use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::errors::{OcpError, OcpResult};

pub const DEFAULT_MAX_AUDIT_BYTES: u64 = 2 * 1024 * 1024;
const AUDIT_FILE: &str = "audit.ndjson";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub event: String,
    pub tool: Option<String>,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: Option<String>,
    pub latency_ms: u64,
    /// Correlation ID linking this entry to a specific operation (preflight, save, delete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Which Tauri command or subsystem produced this entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

pub struct AuditLog {
    dir: PathBuf,
    max_bytes: u64,
}

impl AuditLog {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_limit(dir, DEFAULT_MAX_AUDIT_BYTES)
    }

    pub fn with_limit(dir: PathBuf, max_bytes: u64) -> Self {
        Self { dir, max_bytes }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(AUDIT_FILE)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn record(
        &self,
        event: &str,
        tool: Option<&str>,
        target: Option<&str>,
        outcome: &str,
        detail: Option<&str>,
        latency_ms: u64,
    ) -> OcpResult<()> {
        self.record_full(event, tool, target, outcome, detail, latency_ms, None, None)
    }

    /// Record an audit entry with an explicit correlation ID and actor label.
    pub fn record_with_context(
        &self,
        event: &str,
        outcome: &str,
        detail: Option<&str>,
        latency_ms: u64,
        correlation_id: &str,
        actor: &str,
    ) -> OcpResult<()> {
        self.record_full(
            event,
            None,
            None,
            outcome,
            detail,
            latency_ms,
            Some(correlation_id),
            Some(actor),
        )
    }

    pub fn record_with_context_best_effort(
        &self,
        event: &str,
        outcome: &str,
        detail: Option<&str>,
        latency_ms: u64,
        correlation_id: &str,
        actor: &str,
    ) {
        if let Err(err) =
            self.record_with_context(event, outcome, detail, latency_ms, correlation_id, actor)
        {
            eprintln!("OCP audit record failed: {err}");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_full(
        &self,
        event: &str,
        tool: Option<&str>,
        target: Option<&str>,
        outcome: &str,
        detail: Option<&str>,
        latency_ms: u64,
        correlation_id: Option<&str>,
        actor: Option<&str>,
    ) -> OcpResult<()> {
        if let Some(parent) = self.dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                OcpError::config(
                    "audit_dir",
                    format!("Failed to create the OCP audit directory: {error}"),
                )
            })?;
        }
        fs::create_dir_all(&self.dir).map_err(|error| {
            OcpError::config(
                "audit_dir",
                format!("Failed to create the OCP audit directory: {error}"),
            )
        })?;
        set_private_dir_permissions(&self.dir)?;
        self.rotate_if_needed()?;
        let entry = AuditEntry {
            timestamp: Utc::now().to_rfc3339(),
            event: event.to_string(),
            tool: tool.map(str::to_string),
            target: target.map(str::to_string),
            outcome: outcome.to_string(),
            detail: detail.map(str::to_string),
            latency_ms,
            correlation_id: correlation_id.map(str::to_string),
            actor: actor.map(str::to_string),
        };
        let mut line = serde_json::to_string(&entry).map_err(|error| {
            OcpError::internal(format!("Failed to encode audit entry: {error}"))
        })?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())
            .map_err(|error| {
                OcpError::internal(format!("Failed to open the OCP audit log: {error}"))
            })?;
        set_private_file_permissions(&self.path())?;
        file.write_all(line.as_bytes()).map_err(|error| {
            OcpError::internal(format!("Failed to write the OCP audit log: {error}"))
        })?;
        file.flush().map_err(|error| {
            OcpError::internal(format!("Failed to flush the OCP audit log: {error}"))
        })
    }

    pub fn record_best_effort(
        &self,
        event: &str,
        tool: Option<&str>,
        target: Option<&str>,
        outcome: &str,
        detail: Option<&str>,
        latency_ms: u64,
    ) {
        if let Err(error) = self.record(event, tool, target, outcome, detail, latency_ms) {
            eprintln!("OCP audit record failed: {error}");
        }
    }

    pub fn entries(&self, limit: usize) -> Vec<AuditEntry> {
        let content = match fs::read_to_string(self.path()) {
            Ok(content) => content,
            Err(_) => return Vec::new(),
        };
        let mut entries: Vec<AuditEntry> = content
            .lines()
            .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
            .collect();
        entries.reverse();
        entries.truncate(limit);
        entries
    }

    fn rotate_if_needed(&self) -> OcpResult<()> {
        let path = self.path();
        let size = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(_) => return Ok(()),
        };
        if size <= self.max_bytes {
            return Ok(());
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        let lines = content.lines().collect::<Vec<_>>();
        let mut kept = Vec::new();
        let mut kept_bytes = 0usize;
        for line in lines.into_iter().rev() {
            let line_bytes = line.len() + 1;
            if kept_bytes + line_bytes > self.max_bytes as usize {
                break;
            }
            kept.push(line);
            kept_bytes += line_bytes;
        }
        kept.reverse();
        let mut payload = kept.join("\n");
        if !payload.is_empty() {
            payload.push('\n');
        }
        fs::write(&path, payload).map_err(|error| {
            OcpError::internal(format!("Failed to rotate the OCP audit log: {error}"))
        })?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> OcpResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        OcpError::config(
            "audit_perms",
            format!("Failed to set directory permissions: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> OcpResult<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> OcpResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        OcpError::config(
            "audit_perms",
            format!("Failed to set file permissions: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> OcpResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn audit_records_and_reads_back_entries_newest_first() {
        let dir = tempdir().unwrap();
        let log = AuditLog::new(dir.path().to_path_buf());
        log.record("preflight_finished", None, None, "passed", None, 12)
            .unwrap();
        log.record(
            "tool_finished",
            Some("ocp_get_pods"),
            Some("team-a"),
            "success",
            Some("ok"),
            3,
        )
        .unwrap();
        let entries = log.entries(10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tool.as_deref(), Some("ocp_get_pods"));
        assert_eq!(entries[0].outcome, "success");
        assert_eq!(entries[1].event, "preflight_finished");
        assert!(entries[0].timestamp.contains('T'));
    }

    #[test]
    fn entries_are_limited_to_the_requested_count() {
        let dir = tempdir().unwrap();
        let log = AuditLog::new(dir.path().to_path_buf());
        for index in 0..5 {
            log.record("event", None, None, "ok", None, index).unwrap();
        }
        let entries = log.entries(2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].latency_ms, 4);
    }

    #[test]
    fn audit_log_rotates_when_over_the_byte_cap() {
        let dir = tempdir().unwrap();
        let log = AuditLog::with_limit(dir.path().to_path_buf(), 256);
        for index in 0..40 {
            log.record(
                "event",
                Some("ocp_get_pods"),
                Some("team-a"),
                "ok",
                Some(&"x".repeat(40)),
                index,
            )
            .unwrap();
        }
        let metadata = fs::metadata(log.path()).unwrap();
        assert!(metadata.len() <= 256 + 512);
        let entries = log.entries(1000);
        assert!(entries.len() < 40);
    }
}
