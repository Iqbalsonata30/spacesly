use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const FORMAT_TIMEOUT: Duration = Duration::from_secs(8);

pub fn format_code(formatter: String, source: String) -> Result<String, String> {
    match formatter.as_str() {
        "rustfmt" => format_with_command("rustfmt", &["--emit", "stdout"], source),
        "gofmt" => format_with_command("gofmt", &[], source),
        _ => Err(format!("Unsupported formatter: {formatter}")),
    }
}

fn format_with_command(command: &'static str, args: &'static [&'static str], source: String) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_formatter(command, args, source);
        let _ = tx.send(result);
    });

    rx.recv_timeout(FORMAT_TIMEOUT)
        .map_err(|_| format!("{command} timed out after {} seconds.", FORMAT_TIMEOUT.as_secs()))?
}

fn run_formatter(command: &str, args: &[&str], source: String) -> Result<String, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to start {command}: {error}"))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(source.as_bytes())
            .map_err(|error| format!("Failed to send source to {command}: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to wait for {command}: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{command} exited with status {}.", output.status)
        } else {
            stderr
        });
    }

    String::from_utf8(output.stdout).map_err(|_| format!("{command} returned non-UTF-8 output."))
}
