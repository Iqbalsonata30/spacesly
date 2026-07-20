use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AppSecrets {
    #[serde(default)]
    pub jira_api_token: String,
    #[serde(default)]
    pub jira_personal_access_token: String,
    #[serde(default)]
    pub jira_password: String,
    #[serde(default)]
    pub ai_api_keys: HashMap<String, String>,
    #[serde(default)]
    pub mcp_env: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone)]
pub struct AppSecretsStore {
    secrets: Arc<Mutex<AppSecrets>>,
}

impl AppSecretsStore {
    pub fn load() -> Result<Self, String> {
        Ok(Self {
            secrets: Arc::new(Mutex::new(load_app_secrets()?)),
        })
    }

    pub fn replace(&self, secrets: AppSecrets) -> Result<(), String> {
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        *current = secrets;
        Ok(())
    }

    pub fn ai_api_key(&self, provider_id: &str) -> Result<String, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok(secrets
            .ai_api_keys
            .get(provider_id)
            .cloned()
            .unwrap_or_default())
    }

    pub fn mcp_environment(&self, server_name: &str) -> Result<HashMap<String, String>, String> {
        let server_id = server_name.strip_prefix("spacesly-").unwrap_or(server_name);
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok(secrets.mcp_env.get(server_id).cloned().unwrap_or_default())
    }
}

pub fn load_app_secrets() -> Result<AppSecrets, String> {
    let path = secrets_path()?;
    if !path.exists() {
        return Ok(AppSecrets::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read app secrets: {error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("Failed to parse app secrets: {error}"))
}

pub fn save_app_secrets(secrets: AppSecrets) -> Result<(), String> {
    let path = secrets_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create secrets directory: {error}"))?;
        set_private_dir_permissions(parent)?;
    }

    let payload = serde_json::to_string_pretty(&secrets)
        .map_err(|error| format!("Failed to encode app secrets: {error}"))?;
    fs::write(&path, payload).map_err(|error| format!("Failed to write app secrets: {error}"))?;
    set_private_file_permissions(&path)?;
    Ok(())
}

fn secrets_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| "Cannot resolve a config directory for app secrets.".to_string())?;
    Ok(base.join("spacesly").join("secrets.json"))
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Failed to set secrets directory permissions: {error}"))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to set secrets file permissions: {error}"))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_resolves_provider_and_mcp_secrets_without_exposing_the_full_payload() {
        let store = AppSecretsStore {
            secrets: Arc::new(Mutex::new(AppSecrets {
                ai_api_keys: HashMap::from([("openai".to_string(), "token".to_string())]),
                mcp_env: HashMap::from([(
                    "jira".to_string(),
                    HashMap::from([("JIRA_TOKEN".to_string(), "secret".to_string())]),
                )]),
                ..AppSecrets::default()
            })),
        };

        assert_eq!(store.ai_api_key("openai").unwrap(), "token");
        assert_eq!(
            store.mcp_environment("spacesly-jira").unwrap()["JIRA_TOKEN"],
            "secret"
        );
        assert!(store.ai_api_key("missing").unwrap().is_empty());
    }
}
