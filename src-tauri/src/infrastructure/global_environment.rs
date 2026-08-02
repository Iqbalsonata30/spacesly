use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GlobalEnvironmentVariable {
    pub id: String,
    pub key: String,
    pub value: String,
    pub secret: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GlobalEnvironmentVariableInput {
    #[serde(default)]
    pub id: Option<String>,
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    pub secret: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct GlobalEnvironmentVariableView {
    pub id: String,
    pub key: String,
    pub value: String,
    pub secret: bool,
    pub enabled: bool,
    pub value_set: bool,
}

#[derive(Default, Deserialize, Serialize)]
struct GlobalEnvironmentFile {
    #[serde(default)]
    variables: Vec<GlobalEnvironmentVariable>,
}

#[derive(Clone)]
pub struct GlobalEnvironmentStore {
    variables: Arc<Mutex<Vec<GlobalEnvironmentVariable>>>,
}

impl GlobalEnvironmentStore {
    pub fn load() -> Result<Self, String> {
        let variables = load_global_environment_file()?.variables;
        Ok(Self {
            variables: Arc::new(Mutex::new(variables)),
        })
    }

    pub fn global() -> Result<Self, String> {
        static STORE: OnceLock<GlobalEnvironmentStore> = OnceLock::new();
        if let Some(store) = STORE.get() {
            return Ok(store.clone());
        }
        let store = Self::load()?;
        let _ = STORE.set(store.clone());
        Ok(store)
    }

    pub fn list(&self) -> Result<Vec<GlobalEnvironmentVariableView>, String> {
        let mut variables = self
            .variables
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        variables.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(variables.into_iter().map(redacted_view).collect())
    }

    pub fn reveal(&self, id: &str) -> Result<String, String> {
        let variables = self.variables.lock().map_err(|error| error.to_string())?;
        let variable = variables
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| "Global environment variable was not found.".to_string())?;
        Ok(variable.value.clone())
    }

    pub fn save(
        &self,
        input: GlobalEnvironmentVariableInput,
    ) -> Result<GlobalEnvironmentVariableView, String> {
        let key = input.key.trim().to_string();
        validate_environment_key(&key)?;

        let id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(new_environment_id);

        let mut variables = self.variables.lock().map_err(|error| error.to_string())?;
        if variables
            .iter()
            .any(|entry| entry.id != id && entry.key == key)
        {
            return Err(format!(
                "Global environment key '{key}' is already defined."
            ));
        }

        let existing_value = variables
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.value.clone());
        let value = input
            .value
            .unwrap_or_else(|| existing_value.unwrap_or_default());
        if value.is_empty() {
            return Err("Global environment value is required.".to_string());
        }

        let variable = GlobalEnvironmentVariable {
            id: id.clone(),
            key,
            value,
            secret: input.secret,
            enabled: input.enabled,
        };

        match variables.iter().position(|entry| entry.id == id) {
            Some(index) => variables[index] = variable.clone(),
            None => variables.push(variable.clone()),
        }
        variables.sort_by(|left, right| left.key.cmp(&right.key));
        persist_global_environment_file(&variables)?;
        Ok(redacted_view(variable))
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut variables = self.variables.lock().map_err(|error| error.to_string())?;
        let original_len = variables.len();
        variables.retain(|entry| entry.id != id);
        if variables.len() == original_len {
            return Err("Global environment variable was not found.".to_string());
        }
        persist_global_environment_file(&variables)
    }

    pub fn enabled_values(&self) -> Result<HashMap<String, String>, String> {
        let variables = self.variables.lock().map_err(|error| error.to_string())?;
        Ok(variables
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect())
    }

    pub fn secret_values(&self) -> Result<Vec<String>, String> {
        let variables = self.variables.lock().map_err(|error| error.to_string())?;
        Ok(variables
            .iter()
            .filter(|entry| entry.secret)
            .map(|entry| entry.value.clone())
            .filter(|value| value.len() >= 4)
            .collect())
    }
}

pub fn inject_global_environment(command: &mut Command) {
    if let Ok(store) = GlobalEnvironmentStore::global() {
        if let Ok(values) = store.enabled_values() {
            command.envs(values);
        }
    }
}

#[allow(dead_code)]
pub fn inject_global_environment_pty(command: &mut CommandBuilder) {
    if let Ok(store) = GlobalEnvironmentStore::global() {
        if let Ok(values) = store.enabled_values() {
            for (key, value) in values {
                command.env(key, value);
            }
        }
    }
}

pub fn redact_global_environment_values(message: &str) -> String {
    let Ok(store) = GlobalEnvironmentStore::global() else {
        return message.to_string();
    };
    let Ok(values) = store.secret_values() else {
        return message.to_string();
    };
    redact_values(message, &values)
}

fn redacted_view(variable: GlobalEnvironmentVariable) -> GlobalEnvironmentVariableView {
    GlobalEnvironmentVariableView {
        id: variable.id,
        key: variable.key,
        value: if variable.secret {
            String::new()
        } else {
            variable.value.clone()
        },
        secret: variable.secret,
        enabled: variable.enabled,
        value_set: !variable.value.is_empty(),
    }
}

fn validate_environment_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("Global environment key is required.".to_string());
    }
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err("Global environment key is required.".to_string());
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err("Environment key must start with a letter or underscore.".to_string());
    }
    if !chars.all(|character| character == '_' || character.is_ascii_alphanumeric()) {
        return Err(
            "Environment key may only contain letters, numbers, and underscores.".to_string(),
        );
    }
    Ok(())
}

fn redact_values(message: &str, values: &[String]) -> String {
    let mut redacted = message.to_string();
    let mut seen = HashSet::new();
    for value in values {
        if seen.insert(value) {
            redacted = redacted.replace(value, "[REDACTED]");
        }
    }
    redacted
}

fn new_environment_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("global-env-{millis}")
}

fn load_global_environment_file() -> Result<GlobalEnvironmentFile, String> {
    let path = global_environment_path()?;
    if !path.exists() {
        return Ok(GlobalEnvironmentFile::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read global environment: {error}"))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("Failed to parse global environment: {error}"))
}

fn persist_global_environment_file(variables: &[GlobalEnvironmentVariable]) -> Result<(), String> {
    let path = global_environment_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create global environment directory: {error}"))?;
        set_private_dir_permissions(parent)?;
    }
    let payload = serde_json::to_string_pretty(&GlobalEnvironmentFile {
        variables: variables.to_vec(),
    })
    .map_err(|error| format!("Failed to encode global environment: {error}"))?;
    fs::write(&path, payload)
        .map_err(|error| format!("Failed to write global environment: {error}"))?;
    set_private_file_permissions(&path)
}

fn global_environment_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| "Cannot resolve a config directory for global environment.".to_string())?;
    Ok(base.join("spacesly").join("global_environment.json"))
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Failed to set global environment directory permissions: {error}"))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to set global environment file permissions: {error}"))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_environment_keys() {
        assert!(validate_environment_key("GITHUB_TOKEN").is_ok());
        assert!(validate_environment_key("_JAVA_HOME").is_ok());
        assert!(validate_environment_key("").is_err());
        assert!(validate_environment_key("1TOKEN").is_err());
        assert!(validate_environment_key("BAD-KEY").is_err());
    }

    #[test]
    fn redacts_secret_values_only_when_long_enough() {
        let redacted = redact_values(
            "token super-secret-value and tiny abc",
            &["super-secret-value".to_string(), "abc".to_string()],
        );
        assert!(!redacted.contains("super-secret-value"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("abc"));
    }

    #[test]
    fn secret_views_do_not_expose_values() {
        let view = redacted_view(GlobalEnvironmentVariable {
            id: "env-1".to_string(),
            key: "CUSTOM_API_KEY".to_string(),
            value: "secret".to_string(),
            secret: true,
            enabled: true,
        });
        assert!(view.value.is_empty());
        assert!(view.value_set);
    }
}
