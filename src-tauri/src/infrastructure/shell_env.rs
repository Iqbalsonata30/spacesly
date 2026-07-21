use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Injects the user's login shell environment into a spawned command.
pub fn inject_shell_env(command: &mut Command) {
    let env = shell_env();
    command.env_clear();
    command.envs(
        env.vars
            .iter()
            .filter(|(key, _)| is_safe_runtime_env_key(key)),
    );
}

fn is_safe_runtime_env_key(key: &str) -> bool {
    let normalized = key.to_ascii_uppercase();
    matches!(
        normalized.as_str(),
        "PATH"
            | "HOME"
            | "USERPROFILE"
            | "TEMP"
            | "TMP"
            | "TMPDIR"
            | "LANG"
            | "LC_ALL"
            | "LC_CTYPE"
            | "LC_COLLATE"
            | "XDG_CONFIG_HOME"
            | "XDG_DATA_HOME"
            | "XDG_CACHE_HOME"
            | "SSL_CERT_FILE"
            | "SSL_CERT_DIR"
    )
}

fn shell_env() -> &'static ShellEnv {
    static ENV: OnceLock<ShellEnv> = OnceLock::new();
    ENV.get_or_init(resolve_shell_env)
}

struct ShellEnv {
    vars: HashMap<String, String>,
}

fn resolve_shell_env() -> ShellEnv {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let delimiter = "__SPACESLY_ENV__";
    let output = login_shell_command(
        &shell,
        &format!("echo {delimiter}; /usr/bin/env; echo {delimiter}"),
    )
    .output()
    .ok();

    let vars = output
        .as_ref()
        .and_then(|output| parse_env_output(&String::from_utf8_lossy(&output.stdout), delimiter))
        .unwrap_or_default();

    ShellEnv { vars }
}

fn login_shell_command(shell: &str, script: &str) -> Command {
    let shell_name = Path::new(shell)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut command = Command::new(shell);

    if shell_name == "fish" {
        command.args(["--login", "--interactive", "-c", script]);
    } else {
        command.args(["-lic", script]);
    }

    command.stderr(Stdio::null());
    command
}

fn parse_env_output(stdout: &str, delimiter: &str) -> Option<HashMap<String, String>> {
    let mut parts = stdout.split(delimiter);
    let _before = parts.next();
    let env_section = parts.next()?;
    let mut vars = HashMap::new();

    for line in env_section.lines() {
        let Some(separator) = line.find('=') else {
            continue;
        };
        let key = &line[..separator];
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            continue;
        }
        vars.insert(key.to_string(), line[separator + 1..].to_string());
    }

    Some(vars)
}

#[cfg(test)]
mod tests {
    use super::is_safe_runtime_env_key;

    #[test]
    fn shell_environment_allowlist_excludes_credentials() {
        assert!(is_safe_runtime_env_key("PATH"));
        assert!(is_safe_runtime_env_key("lc_all"));
        assert!(!is_safe_runtime_env_key("AWS_ACCESS_KEY_ID"));
        assert!(!is_safe_runtime_env_key("OPENAI_API_KEY"));
        assert!(!is_safe_runtime_env_key("SSH_AUTH_SOCK"));
    }
}
