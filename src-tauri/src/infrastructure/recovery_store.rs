use crate::infrastructure::files::{read_file_at_root, LineEnding, TextEncoding, WorkspaceRoot};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECOVERY_CONTENT_BYTES: usize = 1_000_000;
const MAX_RECOVERY_SNAPSHOTS_PER_WORKSPACE: usize = 64;
const RECOVERY_RETENTION_MILLIS: u64 = 30 * 24 * 60 * 60 * 1_000;

#[derive(Clone)]
pub struct RecoveryStore {
    connection: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RecoverySnapshotInput {
    pub path: String,
    pub name: String,
    pub content: String,
    pub persisted_version: String,
    pub root_revision: u64,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    pub revision: u64,
    pub scroll_top: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoverySnapshot {
    pub workspace_id: String,
    pub path: String,
    pub name: String,
    pub content: String,
    pub persisted_content: String,
    pub persisted_version: String,
    pub current_version: Option<String>,
    pub disk_status: RecoveryDiskStatus,
    pub root_revision: u64,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    pub revision: u64,
    pub scroll_top: u64,
    pub updated_at: u64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDiskStatus {
    Unchanged,
    Changed,
    Missing,
}

impl RecoveryStore {
    pub fn open() -> Result<Self, String> {
        let path = database_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create recovery data directory: {error}"))?;
            secure_data_directory(parent)?;
        }
        Self::open_at(path)
    }

    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create recovery data directory: {error}"))?;
        }
        let connection = Connection::open(&path)
            .map_err(|error| format!("Failed to open recovery database: {error}"))?;
        secure_database_file(&path)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS recovery_snapshots (
                   workspace_id TEXT NOT NULL,
                   root_path TEXT NOT NULL,
                   path TEXT NOT NULL,
                   name TEXT NOT NULL,
                   content TEXT NOT NULL,
                   persisted_version TEXT NOT NULL,
                   root_revision INTEGER NOT NULL,
                   encoding TEXT NOT NULL,
                   line_ending TEXT NOT NULL,
                   revision INTEGER NOT NULL,
                   scroll_top INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL,
                   PRIMARY KEY (workspace_id, root_path, path)
                 );
                 CREATE INDEX IF NOT EXISTS idx_recovery_snapshots_updated_at
                   ON recovery_snapshots(updated_at);",
            )
            .map_err(|error| format!("Failed to initialize recovery database: {error}"))?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.prune_expired()?;
        Ok(store)
    }

    pub fn sync_workspace(
        &self,
        root: &WorkspaceRoot,
        workspace_id: String,
        snapshots: Vec<RecoverySnapshotInput>,
    ) -> Result<(), String> {
        if snapshots.len() > MAX_RECOVERY_SNAPSHOTS_PER_WORKSPACE {
            return Err(format!(
                "Too many recovery snapshots ({}), limit is {}.",
                snapshots.len(),
                MAX_RECOVERY_SNAPSHOTS_PER_WORKSPACE
            ));
        }
        for snapshot in &snapshots {
            validate_snapshot(snapshot)?;
        }

        let now = now_millis()?;
        let root_path = root.path(&workspace_id)?.to_string_lossy().to_string();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start recovery transaction: {error}"))?;
        transaction
            .execute(
                "DELETE FROM recovery_snapshots WHERE workspace_id = ?1 AND root_path = ?2",
                params![workspace_id, root_path],
            )
            .map_err(|error| format!("Failed to clear old recovery snapshots: {error}"))?;
        for snapshot in snapshots {
            transaction
                .execute(
                    "INSERT INTO recovery_snapshots
                       (workspace_id, root_path, path, name, content, persisted_version, root_revision,
                        encoding, line_ending, revision, scroll_top, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        workspace_id,
                        root_path,
                        snapshot.path,
                        snapshot.name,
                        snapshot.content,
                        snapshot.persisted_version,
                        snapshot.root_revision,
                        encoding_name(snapshot.encoding),
                        line_ending_name(snapshot.line_ending),
                        snapshot.revision,
                        snapshot.scroll_top,
                        now
                    ],
                )
                .map_err(|error| format!("Failed to save recovery snapshot: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit recovery snapshots: {error}"))?;
        drop(connection);
        self.prune_expired()?;
        Ok(())
    }

    pub fn list_workspace(
        &self,
        root: &WorkspaceRoot,
        workspace_id: String,
    ) -> Result<Vec<RecoverySnapshot>, String> {
        let root_path = root.path(&workspace_id)?.to_string_lossy().to_string();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT workspace_id, path, name, content, persisted_version,
                        encoding, line_ending, revision, scroll_top, updated_at
                   FROM recovery_snapshots
                  WHERE workspace_id = ?1 AND root_path = ?2
                  ORDER BY updated_at DESC, path ASC
                  LIMIT ?3",
            )
            .map_err(|error| format!("Failed to query recovery snapshots: {error}"))?;
        let rows = statement
            .query_map(
                params![
                    workspace_id,
                    root_path,
                    MAX_RECOVERY_SNAPSHOTS_PER_WORKSPACE
                ],
                |row| {
                    Ok(StoredRecoverySnapshot {
                        workspace_id: row.get(0)?,
                        path: row.get(1)?,
                        name: row.get(2)?,
                        content: row.get(3)?,
                        persisted_version: row.get(4)?,
                        encoding: row.get(5)?,
                        line_ending: row.get(6)?,
                        revision: row.get(7)?,
                        scroll_top: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                },
            )
            .map_err(|error| format!("Failed to load recovery snapshots: {error}"))?;

        let stored = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode recovery snapshots: {error}"))?;
        drop(statement);
        drop(connection);

        stored
            .into_iter()
            .map(|snapshot| snapshot.into_recovery_snapshot(root))
            .collect()
    }

    pub fn delete_snapshot(
        &self,
        root: &WorkspaceRoot,
        workspace_id: String,
        path: String,
    ) -> Result<(), String> {
        let root_path = root.path(&workspace_id)?.to_string_lossy().to_string();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM recovery_snapshots
                  WHERE workspace_id = ?1 AND root_path = ?2 AND path = ?3",
                params![workspace_id, root_path, path],
            )
            .map_err(|error| format!("Failed to delete recovery snapshot: {error}"))?;
        Ok(())
    }

    fn prune_expired(&self) -> Result<(), String> {
        let cutoff = now_millis()?.saturating_sub(RECOVERY_RETENTION_MILLIS);
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM recovery_snapshots WHERE updated_at < ?1",
                params![cutoff],
            )
            .map_err(|error| format!("Failed to prune recovery snapshots: {error}"))?;
        Ok(())
    }
}

#[derive(Debug)]
struct StoredRecoverySnapshot {
    workspace_id: String,
    path: String,
    name: String,
    content: String,
    persisted_version: String,
    encoding: String,
    line_ending: String,
    revision: u64,
    scroll_top: u64,
    updated_at: u64,
}

impl StoredRecoverySnapshot {
    fn into_recovery_snapshot(self, root: &WorkspaceRoot) -> Result<RecoverySnapshot, String> {
        let disk = read_file_at_root(root, self.workspace_id.clone(), self.path.clone());
        let (persisted_content, current_version, disk_status, root_revision, encoding, line_ending) =
            match disk {
                Ok(snapshot) => {
                    let disk_status = if snapshot.version == self.persisted_version {
                        RecoveryDiskStatus::Unchanged
                    } else {
                        RecoveryDiskStatus::Changed
                    };
                    (
                        snapshot.content,
                        Some(snapshot.version),
                        disk_status,
                        snapshot.root_revision,
                        snapshot.encoding,
                        snapshot.line_ending,
                    )
                }
                Err(_) => (
                    String::new(),
                    None,
                    RecoveryDiskStatus::Missing,
                    root.revision(&self.workspace_id)?,
                    parse_encoding(&self.encoding)?,
                    parse_line_ending(&self.line_ending)?,
                ),
            };

        Ok(RecoverySnapshot {
            workspace_id: self.workspace_id,
            path: self.path,
            name: self.name,
            content: self.content,
            persisted_content,
            persisted_version: current_version
                .as_ref()
                .map(|_| self.persisted_version)
                .unwrap_or_default(),
            current_version,
            disk_status,
            root_revision,
            encoding,
            line_ending,
            revision: self.revision,
            scroll_top: self.scroll_top,
            updated_at: self.updated_at,
        })
    }
}

fn validate_snapshot(snapshot: &RecoverySnapshotInput) -> Result<(), String> {
    if snapshot.path.trim().is_empty()
        || snapshot.path.starts_with('/')
        || snapshot
            .path
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err("Recovery snapshot path is invalid.".to_string());
    }
    if snapshot.content.as_bytes().len() > MAX_RECOVERY_CONTENT_BYTES {
        return Err(format!(
            "Recovery snapshot for {} is too large ({} bytes, limit {}).",
            snapshot.path,
            snapshot.content.as_bytes().len(),
            MAX_RECOVERY_CONTENT_BYTES
        ));
    }
    Ok(())
}

fn encoding_name(encoding: TextEncoding) -> &'static str {
    match encoding {
        TextEncoding::Utf8 => "utf8",
        TextEncoding::Utf8Bom => "utf8-bom",
        TextEncoding::Utf16le => "utf16le",
        TextEncoding::Utf16be => "utf16be",
    }
}

fn line_ending_name(line_ending: LineEnding) -> &'static str {
    match line_ending {
        LineEnding::Lf => "lf",
        LineEnding::Crlf => "crlf",
    }
}

fn parse_encoding(value: &str) -> Result<TextEncoding, String> {
    match value {
        "utf8" => Ok(TextEncoding::Utf8),
        "utf8-bom" => Ok(TextEncoding::Utf8Bom),
        "utf16le" => Ok(TextEncoding::Utf16le),
        "utf16be" => Ok(TextEncoding::Utf16be),
        other => Err(format!("Unknown recovery encoding: {other}")),
    }
}

fn parse_line_ending(value: &str) -> Result<LineEnding, String> {
    match value {
        "lf" => Ok(LineEnding::Lf),
        "crlf" => Ok(LineEnding::Crlf),
        other => Err(format!("Unknown recovery line ending: {other}")),
    }
}

fn database_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
        .ok_or_else(|| "Cannot resolve application data directory.".to_string())?;
    Ok(base.join("spacesly").join("recovery.db"))
}

fn now_millis() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before Unix epoch: {error}"))?
        .as_millis() as u64)
}

#[cfg(unix)]
fn secure_data_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Failed to secure recovery data directory: {error}"))
}

#[cfg(not(unix))]
fn secure_data_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn secure_database_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to secure recovery database: {error}"))
}

#[cfg(not(unix))]
fn secure_database_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_store_round_trips_dirty_snapshots() {
        let path = std::env::temp_dir().join(format!(
            "spacesly-recovery-{}.db",
            now_millis().expect("time")
        ));
        let store = RecoveryStore::open_at(path.clone()).expect("store opens");
        let root = WorkspaceRoot::home().expect("workspace root");

        store
            .sync_workspace(
                &root,
                "workspace-personal".to_string(),
                vec![RecoverySnapshotInput {
                    path: "src/main.rs".to_string(),
                    name: "main.rs".to_string(),
                    content: "dirty".to_string(),
                    persisted_version: "v1".to_string(),
                    root_revision: 1,
                    encoding: TextEncoding::Utf8,
                    line_ending: LineEnding::Lf,
                    revision: 4,
                    scroll_top: 12,
                }],
            )
            .expect("snapshot saved");

        let connection = Connection::open(&path).expect("database readable");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM recovery_snapshots", [], |row| {
                row.get(0)
            })
            .expect("count snapshots");
        assert_eq!(count, 1);

        store
            .sync_workspace(&root, "workspace-personal".to_string(), Vec::new())
            .expect("workspace snapshots cleared");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM recovery_snapshots", [], |row| {
                row.get(0)
            })
            .expect("count snapshots after clear");
        assert_eq!(count, 0);

        let _ = fs::remove_file(path);
    }
}
