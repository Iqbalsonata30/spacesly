use super::shell_env::inject_shell_env;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct ShellCommandRequest {
    pub command: String,
    pub workdir: Option<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShellCommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ShellCompletionRequest {
    pub input: String,
    pub workdir: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShellCompletionResult {
    pub replacement: Option<String>,
    pub suggestions: Vec<String>,
}

pub fn run_shell_command(request: ShellCommandRequest) -> Result<ShellCommandResult, String> {
    let command_text = request.command.trim();
    if command_text.is_empty() {
        return Err("Command is required.".to_string());
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let wrapped_command = format!(
        "{{\n{command_text}\n}}\nstatus=$?\nprintf '\\n__SPACESLY_EXIT__:%s\\n__SPACESLY_CWD__:%s\\n' \"$status\" \"$PWD\"\nexit $status"
    );
    let mut command = Command::new(shell);
    inject_shell_env(&mut command);
    command
        .args(["-lc", &wrapped_command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(workdir) = request
        .workdir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.current_dir(workdir);
    } else if let Some(home) = std::env::var("HOME").ok().filter(|value| !value.is_empty()) {
        command.current_dir(home);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start shell command: {error}"))?;
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(30).clamp(1, 300));
    let started = Instant::now();

    loop {
        if child
            .try_wait()
            .map_err(|error| format!("Failed to poll shell command: {error}"))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .map_err(|error| format!("Failed to read shell command output: {error}"))?;
            let (stdout, exit_code, cwd) =
                parse_shell_stdout(&String::from_utf8_lossy(&output.stdout));
            return Ok(ShellCommandResult {
                exit_code: exit_code.or_else(|| output.status.code()),
                stdout,
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                timed_out: false,
                cwd,
            });
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| format!("Failed to stop shell command after timeout: {error}"))?;
            let (stdout, exit_code, cwd) =
                parse_shell_stdout(&String::from_utf8_lossy(&output.stdout));
            return Ok(ShellCommandResult {
                exit_code: exit_code.or_else(|| output.status.code()),
                stdout,
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                timed_out: true,
                cwd,
            });
        }

        sleep(Duration::from_millis(40));
    }
}

pub fn complete_shell_input(
    request: ShellCompletionRequest,
) -> Result<ShellCompletionResult, String> {
    let token = current_token(&request.input);
    let workdir = request
        .workdir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let suggestions = if token.contains('/') || token.starts_with('.') || token.starts_with('~') {
        path_suggestions(token, &workdir)
    } else {
        command_suggestions(token, &workdir)
    };
    let replacement = common_prefix(&suggestions).filter(|prefix| prefix.len() > token.len());

    Ok(ShellCompletionResult {
        replacement,
        suggestions: suggestions.into_iter().take(24).collect(),
    })
}

fn current_token(input: &str) -> &str {
    input
        .rsplit_once(|character: char| character.is_whitespace())
        .map(|(_, token)| token)
        .unwrap_or(input)
}

fn path_suggestions(token: &str, workdir: &Path) -> Vec<String> {
    let (display_dir, partial) = token.rsplit_once('/').unwrap_or(("", token));
    let base_dir = if display_dir.is_empty() {
        workdir.to_path_buf()
    } else if display_dir == "~" {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workdir.to_path_buf())
    } else if let Some(rest) = display_dir.strip_prefix("~/") {
        std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|_| workdir.join(rest))
    } else {
        let path = PathBuf::from(display_dir);
        if path.is_absolute() {
            path
        } else {
            workdir.join(path)
        }
    };
    let prefix = if display_dir.is_empty() {
        "".to_string()
    } else {
        format!("{display_dir}/")
    };

    read_dir_names(&base_dir)
        .into_iter()
        .filter(|name| name.starts_with(partial))
        .map(|name| {
            let full_path = base_dir.join(&name);
            let suffix = if full_path.is_dir() { "/" } else { "" };
            format!("{prefix}{name}{suffix}")
        })
        .collect()
}

fn command_suggestions(token: &str, workdir: &Path) -> Vec<String> {
    let mut suggestions = BTreeSet::new();
    for command in [
        "cd", "clear", "ls", "pwd", "mkdir", "touch", "rm", "cp", "mv", "grep", "rg", "git", "bun",
        "npm", "cargo",
    ] {
        if command.starts_with(token) {
            suggestions.insert(command.to_string());
        }
    }

    for name in read_dir_names(workdir) {
        if name.starts_with(token) {
            suggestions.insert(name);
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in read_dir_names(&dir) {
                if name.starts_with(token) {
                    suggestions.insert(name);
                }
            }
        }
    }

    suggestions.into_iter().collect()
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn common_prefix(values: &[String]) -> Option<String> {
    let first = values.first()?.clone();
    let mut prefix = first.as_str();

    for value in values.iter().skip(1) {
        let shared_len = prefix
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(prefix.len()))
            .take_while(|&index| value.starts_with(&prefix[..index]))
            .last()
            .unwrap_or(0);
        prefix = &prefix[..shared_len];
    }

    if prefix.is_empty() {
        None
    } else {
        Some(prefix.to_string())
    }
}

fn parse_shell_stdout(stdout: &str) -> (String, Option<i32>, Option<String>) {
    let mut output_lines = Vec::new();
    let mut exit_code = None;
    let mut cwd = None;

    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("__SPACESLY_EXIT__:") {
            exit_code = value.trim().parse::<i32>().ok();
            continue;
        }

        if let Some(value) = line.strip_prefix("__SPACESLY_CWD__:") {
            let value = value.trim();
            if !value.is_empty() {
                cwd = Some(value.to_string());
            }
            continue;
        }

        output_lines.push(line);
    }

    (output_lines.join("\n"), exit_code, cwd)
}
