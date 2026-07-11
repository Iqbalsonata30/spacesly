use super::shell_env::inject_shell_env;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MIN_TIMEOUT_SECONDS: u64 = 1;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const MAX_CAPTURE_HEAD_BYTES: usize = 1_048_576;
const MAX_CAPTURE_TAIL_BYTES: usize = 8_192;
const DRAIN_BUFFER_SIZE: usize = 8192;

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

struct CapturedStream {
    head: Vec<u8>,
    tail: Vec<u8>,
    truncated: bool,
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
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture shell stdout.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture shell stderr.".to_string())?;
    let stdout_reader = thread::spawn(move || capture_stream(stdout));
    let stderr_reader = thread::spawn(move || capture_stream(stderr));

    let timeout = Duration::from_secs(
        request
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
            .clamp(MIN_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS),
    );
    let timed_out = match child
        .wait_timeout(timeout)
        .map_err(|error| format!("Failed to wait for shell command: {error}"))?
    {
        Some(_) => false,
        None => {
            let _ = child.kill();
            true
        }
    };

    let status = child
        .wait()
        .map_err(|error| format!("Failed to read shell command output: {error}"))?;
    let stdout_capture = stdout_reader
        .join()
        .map_err(|_| "Failed to collect shell stdout.".to_string())?;
    let stderr_capture = stderr_reader
        .join()
        .map_err(|_| "Failed to collect shell stderr.".to_string())?;

    let (stdout, exit_code, cwd) = parse_shell_stdout(&render_stream(stdout_capture, "stdout"));

    Ok(ShellCommandResult {
        exit_code: exit_code.or_else(|| status.code()),
        stdout,
        stderr: render_stream(stderr_capture, "stderr"),
        timed_out,
        cwd,
    })
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

fn capture_stream<R: Read>(mut reader: R) -> CapturedStream {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; DRAIN_BUFFER_SIZE];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => {
                if !truncated {
                    let remaining = MAX_CAPTURE_HEAD_BYTES.saturating_sub(head.len());
                    if remaining > 0 {
                        let to_copy = remaining.min(size);
                        head.extend_from_slice(&buffer[..to_copy]);
                        if to_copy < size {
                            truncated = true;
                            push_tail(&mut tail, &buffer[to_copy..size]);
                        }
                        continue;
                    }
                    truncated = true;
                }

                push_tail(&mut tail, &buffer[..size]);
            }
            Err(_) => break,
        }
    }

    CapturedStream {
        head,
        tail,
        truncated,
    }
}

fn render_stream(capture: CapturedStream, stream_name: &str) -> String {
    let mut text = String::from_utf8_lossy(&capture.head).to_string();

    if capture.truncated {
        text.push_str(&format!(
            "\n[spacesly] {stream_name} truncated after {MAX_CAPTURE_HEAD_BYTES} bytes; showing last {MAX_CAPTURE_TAIL_BYTES} bytes"
        ));
        if !capture.tail.is_empty() {
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&capture.tail));
        }
    } else if !capture.tail.is_empty() {
        text.push_str(&String::from_utf8_lossy(&capture.tail));
    }

    text
}

fn push_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    if chunk.is_empty() {
        return;
    }

    if chunk.len() >= MAX_CAPTURE_TAIL_BYTES {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - MAX_CAPTURE_TAIL_BYTES..]);
        return;
    }

    let overflow = tail.len() + chunk.len();
    if overflow > MAX_CAPTURE_TAIL_BYTES {
        let drain = overflow - MAX_CAPTURE_TAIL_BYTES;
        tail.drain(0..drain);
    }
    tail.extend_from_slice(chunk);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_stream_truncates_at_limit() {
        let input = vec![b'a'; MAX_CAPTURE_HEAD_BYTES + MAX_CAPTURE_TAIL_BYTES + 32];
        let capture = capture_stream(std::io::Cursor::new(input));

        assert_eq!(capture.head.len(), MAX_CAPTURE_HEAD_BYTES);
        assert_eq!(capture.tail.len(), MAX_CAPTURE_TAIL_BYTES);
        assert!(capture.truncated);
    }

    #[test]
    fn run_shell_command_times_out_and_kills_child() {
        let result = run_shell_command(ShellCommandRequest {
            command: "sleep 2; printf done".to_string(),
            workdir: None,
            timeout_seconds: Some(1),
        })
        .expect("shell command should complete with timeout");

        assert!(result.timed_out);
    }
}
