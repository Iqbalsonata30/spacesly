use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RULES_BYTES: usize = 32 * 1024;
const MAX_SKILLS_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Non-secret durable configuration used to reconstruct one scheduler-owned Agent runtime.
pub struct AgentRuntimeProfile {
    pub id: String,
    pub runtime: String,
    pub model: String,
    pub opencode_command: String,
    pub opencode_workdir: Option<String>,
    pub agent_rules: String,
    pub agent_skills: String,
    pub temperature: f32,
    pub connector_ids: Vec<String>,
    pub prompt_template_version: String,
    pub rules_revision: String,
    pub skills_revision: String,
}

impl AgentRuntimeProfile {
    /// Validates the profile before it can be stored or used by the Agent runtime resolver.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("id", self.id.as_str()),
            ("model", self.model.as_str()),
            ("opencode_command", self.opencode_command.as_str()),
            (
                "prompt_template_version",
                self.prompt_template_version.as_str(),
            ),
            ("rules_revision", self.rules_revision.as_str()),
            ("skills_revision", self.skills_revision.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("Agent runtime profile field '{name}' is required."));
            }
            if value != value.trim() {
                return Err(format!(
                    "Agent runtime profile field '{name}' must not contain surrounding whitespace."
                ));
            }
        }
        if !self
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("Agent runtime profile ID must be a canonical literal.".to_string());
        }
        if self.runtime != "opencode" {
            return Err("Scheduler Agent profiles must use the OpenCode runtime.".to_string());
        }
        if self.opencode_workdir.as_deref().is_some_and(|path| {
            path.trim().is_empty()
                || path != path.trim()
                || (!PathBuf::from(path).is_absolute() && !path.starts_with("~/"))
        }) {
            return Err(
                "Agent runtime profile working directory must be an absolute or home-relative path without surrounding whitespace."
                    .to_string(),
            );
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err("Agent runtime profile temperature must be between 0 and 2.".to_string());
        }
        if self.agent_rules.len() > MAX_RULES_BYTES {
            return Err(format!(
                "Agent runtime profile Rules exceed the {} KiB limit.",
                MAX_RULES_BYTES / 1024
            ));
        }
        if self.agent_skills.len() > MAX_SKILLS_BYTES {
            return Err(format!(
                "Agent runtime profile Skills exceed the {} KiB limit.",
                MAX_SKILLS_BYTES / 1024
            ));
        }
        let connectors = self.connector_ids.iter().collect::<HashSet<_>>();
        if connectors.len() != self.connector_ids.len()
            || self.connector_ids.iter().any(|connector| {
                connector.trim().is_empty()
                    || connector != connector.trim()
                    || !connector
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
        {
            return Err("Agent runtime profile connector IDs must be unique literals.".to_string());
        }
        Ok(())
    }

    /// Verifies that persisted revision claims match the profile content.
    pub fn validate_content_revisions(&self) -> Result<(), String> {
        if self.rules_revision != content_revision(&self.agent_rules)
            || self.skills_revision != content_revision(&self.agent_skills)
        {
            return Err(
                "Agent runtime profile rule or skill revision did not match its content."
                    .to_string(),
            );
        }
        Ok(())
    }
}

pub(crate) fn content_revision(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

#[derive(Clone)]
/// SQLite-backed owner of durable Agent runtime profiles.
///
/// The store owns only non-secret runtime metadata. Secret provider keys and MCP environments remain
/// owned by `AppSecretsStore` and are resolved at assignment time.
pub struct RuntimeProfileStore {
    connection: Arc<Mutex<Connection>>,
}

impl RuntimeProfileStore {
    /// Opens the default runtime profile database under the application data directory.
    pub fn open() -> Result<Self, String> {
        Self::open_at(runtime_profile_database_path()?)
    }

    /// Opens a runtime profile database at an explicit path, primarily for tests.
    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create runtime profile directory: {error}"))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("Failed to open runtime profile database: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS agent_runtime_profiles (
                   profile_id TEXT PRIMARY KEY,
                   profile_json TEXT NOT NULL,
                   updated_at INTEGER NOT NULL
                 );",
            )
            .map_err(|error| format!("Failed to initialize runtime profiles: {error}"))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Inserts or replaces one validated Agent runtime profile.
    pub fn save(&self, profile: &AgentRuntimeProfile) -> Result<AgentRuntimeProfile, String> {
        if profile.id.starts_with("prompt-") || profile.id.starts_with("agent-") {
            return self.save_immutable(profile);
        }
        profile.validate()?;
        let encoded = serde_json::to_string(profile)
            .map_err(|error| format!("Failed to encode Agent runtime profile: {error}"))?;
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO agent_runtime_profiles (profile_id, profile_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(profile_id) DO UPDATE SET
                   profile_json = excluded.profile_json,
                   updated_at = excluded.updated_at",
                params![profile.id, encoded, now_millis()?],
            )
            .map_err(|error| format!("Failed to save Agent runtime profile: {error}"))?;
        Ok(profile.clone())
    }

    /// Inserts a content-addressed profile or verifies that an existing value is identical.
    pub fn save_immutable(
        &self,
        profile: &AgentRuntimeProfile,
    ) -> Result<AgentRuntimeProfile, String> {
        profile.validate()?;
        profile.validate_content_revisions()?;
        let encoded = serde_json::to_string(profile)
            .map_err(|error| format!("Failed to encode Agent runtime profile: {error}"))?;
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let existing = connection
            .query_row(
                "SELECT profile_json FROM agent_runtime_profiles WHERE profile_id = ?1",
                params![profile.id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to load Agent runtime profile: {error}"))?;
        if let Some(existing) = existing {
            if existing != encoded {
                return Err(format!(
                    "Agent runtime profile '{}' is immutable and already has different content.",
                    profile.id
                ));
            }
            return Ok(profile.clone());
        }
        connection
            .execute(
                "INSERT INTO agent_runtime_profiles (profile_id, profile_json, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![profile.id, encoded, now_millis()?],
            )
            .map_err(|error| format!("Failed to save immutable Agent runtime profile: {error}"))?;
        Ok(profile.clone())
    }

    /// Loads one Agent runtime profile by identifier.
    pub fn get(&self, profile_id: &str) -> Result<Option<AgentRuntimeProfile>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let encoded = connection
            .query_row(
                "SELECT profile_json FROM agent_runtime_profiles WHERE profile_id = ?1",
                params![profile_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to load Agent runtime profile: {error}"))?;
        encoded
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|error| format!("Failed to decode Agent runtime profile: {error}"))
            })
            .transpose()
    }

    /// Lists all stored Agent runtime profiles in stable identifier order.
    pub fn list(&self) -> Result<Vec<AgentRuntimeProfile>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare("SELECT profile_json FROM agent_runtime_profiles ORDER BY profile_id")
            .map_err(|error| format!("Failed to prepare Agent runtime profiles: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Failed to query Agent runtime profiles: {error}"))?;
        rows.map(|row| {
            let encoded =
                row.map_err(|error| format!("Failed to decode Agent runtime row: {error}"))?;
            serde_json::from_str(&encoded)
                .map_err(|error| format!("Failed to decode Agent runtime profile: {error}"))
        })
        .collect()
    }
}

fn runtime_profile_database_path() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path)
            .join("spacesly")
            .join("runtime-profiles.db"));
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not configured.".to_string())?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("spacesly")
        .join("runtime-profiles.db"))
}

fn now_millis() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "Timestamp exceeds u64 milliseconds.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn runtime_profiles_are_validated_and_survive_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("profiles.db");
        let store = RuntimeProfileStore::open_at(path.clone()).expect("store opens");
        let profile = test_profile();
        store.save(&profile).expect("profile saved");
        drop(store);

        let reopened = RuntimeProfileStore::open_at(path).expect("store reopens");
        assert_eq!(
            reopened.get("agent-default").expect("profile read"),
            Some(profile)
        );
        assert_eq!(reopened.list().expect("profiles listed").len(), 1);
    }

    #[test]
    fn runtime_profiles_reject_non_literal_connector_ids() {
        let mut profile = test_profile();
        profile.connector_ids = vec!["jira*".to_string()];
        assert!(profile.validate().is_err());
    }

    #[test]
    fn content_addressed_profiles_are_idempotent_and_immutable() {
        let directory = tempdir().expect("temp directory");
        let store = RuntimeProfileStore::open_at(directory.path().join("profiles.db"))
            .expect("store opens");
        let mut profile = test_profile();
        profile.id = "agent-content-addressed".to_string();
        profile.rules_revision = content_revision(&profile.agent_rules);
        profile.skills_revision = content_revision(&profile.agent_skills);

        store.save(&profile).expect("profile saved");
        store.save(&profile).expect("identical save is idempotent");

        let mut conflicting = profile.clone();
        conflicting.model = "openai/gpt-5.1".to_string();
        assert!(store.save(&conflicting).is_err());
        assert_eq!(store.get(&profile.id).expect("profile read"), Some(profile));
    }

    #[test]
    fn immutable_profiles_reject_forged_content_revisions() {
        let directory = tempdir().expect("temp directory");
        let store = RuntimeProfileStore::open_at(directory.path().join("profiles.db"))
            .expect("store opens");
        let mut profile = test_profile();
        profile.id = "prompt-forged".to_string();
        profile.rules_revision = "sha256:forged".to_string();

        assert!(store.save_immutable(&profile).is_err());
    }

    #[test]
    fn runtime_profiles_reject_oversized_governance_content() {
        let mut profile = test_profile();
        profile.agent_rules = "x".repeat(MAX_RULES_BYTES + 1);
        assert!(profile.validate().is_err());

        let mut profile = test_profile();
        profile.agent_skills = "x".repeat(MAX_SKILLS_BYTES + 1);
        assert!(profile.validate().is_err());
    }

    fn test_profile() -> AgentRuntimeProfile {
        let agent_rules = "Use evidence.".to_string();
        let agent_skills = "Verify changes.".to_string();
        AgentRuntimeProfile {
            id: "agent-default".to_string(),
            runtime: "opencode".to_string(),
            model: "openai/gpt-5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_workdir: None,
            rules_revision: content_revision(&agent_rules),
            skills_revision: content_revision(&agent_skills),
            agent_rules,
            agent_skills,
            temperature: 0.2,
            connector_ids: vec!["jira".to_string()],
            prompt_template_version: "agent-v1".to_string(),
        }
    }
}
