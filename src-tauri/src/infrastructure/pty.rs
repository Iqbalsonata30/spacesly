use super::files::WorkspaceRoot;
use super::shell_env::inject_shell_env;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

const PTY_OUTPUT_BATCH_BYTES: usize = 32 * 1024;
const PTY_OUTPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(25);

pub type PtyState = Arc<Mutex<PtyRegistry>>;

pub struct PtyRegistry {
    sessions: HashMap<String, Arc<Mutex<PtySession>>>,
}

pub struct PtySession {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    child_pid: Option<u32>,
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
    workspace_root: &WorkspaceRoot,
    terminal_id: String,
    workdir: Option<String>,
    on_data: Channel<Vec<u8>>,
) -> Result<(), String> {
    prune_dead_session(state, &terminal_id)?;
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
        .or_else(|| {
            workspace_root
                .path()
                .ok()
                .map(|path| path.to_string_lossy().to_string())
        })
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
    let child_pid = child.process_id();
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
            Arc::new(Mutex::new(PtySession {
                writer,
                child,
                master: pair.master,
                child_pid,
            })),
        );

    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut batch = Vec::with_capacity(PTY_OUTPUT_BATCH_BYTES);
        let mut last_flush = Instant::now();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if !batch.is_empty() {
                        let _ = on_data.send(std::mem::take(&mut batch));
                    }
                    break;
                }
                Ok(size) => {
                    batch.extend_from_slice(&buffer[..size]);
                    if should_flush_pty_batch(batch.len(), last_flush.elapsed()) {
                        if on_data.send(std::mem::take(&mut batch)).is_err() {
                            break;
                        }
                        last_flush = Instant::now();
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(())
}

pub fn close_pty_terminal(state: &PtyState, terminal_id: String) -> Result<(), String> {
    let session = state
        .lock()
        .map_err(|error| error.to_string())?
        .sessions
        .remove(&terminal_id);
    match session {
        Some(session) => {
            close_session_handle(session);
            Ok(())
        }
        None => Ok(()),
    }
}

pub fn pty_current_directory(
    state: &PtyState,
    terminal_id: String,
) -> Result<Option<String>, String> {
    let session = session_handle(state, &terminal_id)?;
    let pid = session.lock().map_err(|error| error.to_string())?.child_pid;
    let Some(pid) = pid else {
        return Ok(None);
    };

    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd"))
            .map(|path| Some(path.to_string_lossy().to_string()))
            .map_err(|error| format!("Failed to read terminal cwd: {error}"))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Ok(None)
    }
}

pub fn write_pty_terminal(
    state: &PtyState,
    terminal_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let session = session_handle(state, &terminal_id)?;
    let mut session = session.lock().map_err(|error| error.to_string())?;
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
    let session = session_handle(state, &terminal_id)?;
    let session = session.lock().map_err(|error| error.to_string())?;
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
    let sessions = state.lock().map(|mut registry| {
        registry
            .sessions
            .drain()
            .map(|(_, session)| session)
            .collect::<Vec<_>>()
    });
    if let Ok(sessions) = sessions {
        for session in sessions {
            close_session_handle(session);
        }
    }
}

fn session_handle(state: &PtyState, terminal_id: &str) -> Result<Arc<Mutex<PtySession>>, String> {
    prune_dead_session(state, terminal_id)?;
    state
        .lock()
        .map_err(|error| error.to_string())?
        .sessions
        .get(terminal_id)
        .cloned()
        .ok_or_else(|| "Terminal session is not open.".to_string())
}

fn prune_dead_session(state: &PtyState, terminal_id: &str) -> Result<(), String> {
    let session = state
        .lock()
        .map_err(|error| error.to_string())?
        .sessions
        .get(terminal_id)
        .cloned();
    let Some(session) = session else {
        return Ok(());
    };

    let should_remove = match session
        .lock()
        .map_err(|error| error.to_string())?
        .child
        .try_wait()
    {
        Ok(Some(_)) | Err(_) => true,
        Ok(None) => false,
    };

    if should_remove {
        let removed = {
            let mut registry = state.lock().map_err(|error| error.to_string())?;
            if registry
                .sessions
                .get(terminal_id)
                .is_some_and(|current| Arc::ptr_eq(current, &session))
            {
                registry.sessions.remove(terminal_id)
            } else {
                None
            }
        };
        if let Some(removed) = removed {
            if Arc::ptr_eq(&removed, &session) {
                close_session_handle(removed);
            }
        }
    }

    Ok(())
}

fn close_session_handle(session: Arc<Mutex<PtySession>>) {
    if let Ok(mut session) = session.lock() {
        let _ = session.child.kill();
        let _ = session.child.wait();
    }
}

fn should_flush_pty_batch(size: usize, elapsed: Duration) -> bool {
    size >= PTY_OUTPUT_BATCH_BYTES || elapsed >= PTY_OUTPUT_FLUSH_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::{should_flush_pty_batch, PTY_OUTPUT_BATCH_BYTES, PTY_OUTPUT_FLUSH_INTERVAL};
    use std::time::Duration;

    #[test]
    fn output_batch_flushes_by_size_or_time() {
        assert!(!should_flush_pty_batch(
            PTY_OUTPUT_BATCH_BYTES - 1,
            Duration::from_millis(1)
        ));
        assert!(should_flush_pty_batch(
            PTY_OUTPUT_BATCH_BYTES,
            Duration::from_millis(1)
        ));
        assert!(should_flush_pty_batch(1, PTY_OUTPUT_FLUSH_INTERVAL));
    }
}
