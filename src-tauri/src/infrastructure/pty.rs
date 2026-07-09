use super::shell_env::inject_shell_env;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;

pub type PtyState = Arc<Mutex<PtyRegistry>>;

pub struct PtyRegistry {
    sessions: HashMap<String, PtySession>,
}

pub struct PtySession {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}

impl PtyRegistry {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

pub fn open_pty_terminal(
    state: &PtyState,
    terminal_id: String,
    workdir: Option<String>,
    on_data: Channel<Vec<u8>>,
) -> Result<(), String> {
    if state
        .lock()
        .map_err(|error| error.to_string())?
        .sessions
        .contains_key(&terminal_id)
    {
        return Ok(());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to open PTY: {error}"))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = CommandBuilder::new(&shell);
    command.arg("-l");
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("SHELL", &shell);
    if let Ok(lang) = std::env::var("LANG") {
        command.env("LANG", &lang);
        command.env("LC_ALL", &lang);
    }

    let cwd = workdir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".to_string());
    command.cwd(cwd);

    // Reuse the same login-shell PATH/env resolution used by MCP and shell helpers.
    let mut env_probe = std::process::Command::new("true");
    inject_shell_env(&mut env_probe);
    for (key, value) in env_probe.get_envs() {
        if let (Some(key), Some(value)) = (key.to_str(), value.and_then(|entry| entry.to_str())) {
            command.env(key, value);
        }
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to spawn shell: {error}"))?;
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to get PTY writer: {error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to get PTY reader: {error}"))?;

    state
        .lock()
        .map_err(|error| error.to_string())?
        .sessions
        .insert(
            terminal_id.clone(),
            PtySession {
                writer,
                child,
                master: pair.master,
            },
        );

    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    let _ = on_data.send(buffer[..size].to_vec());
                }
                Err(_) => break,
            }
        }
    });

    Ok(())
}

pub fn write_pty_terminal(
    state: &PtyState,
    terminal_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let mut registry = state.lock().map_err(|error| error.to_string())?;
    let session = registry
        .sessions
        .get_mut(&terminal_id)
        .ok_or_else(|| "Terminal session is not open.".to_string())?;
    session
        .writer
        .write_all(&data)
        .map_err(|error| format!("Failed to write to PTY: {error}"))
}

pub fn resize_pty_terminal(
    state: &PtyState,
    terminal_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let mut registry = state.lock().map_err(|error| error.to_string())?;
    let session = registry
        .sessions
        .get_mut(&terminal_id)
        .ok_or_else(|| "Terminal session is not open.".to_string())?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to resize PTY: {error}"))
}

pub fn close_all_terminals(state: &PtyState) {
    if let Ok(mut registry) = state.lock() {
        for (_, mut session) in registry.sessions.drain() {
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
    }
}
