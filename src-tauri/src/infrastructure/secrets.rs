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
    #[serde(default)]
    pub mcp_connectors: HashMap<String, McpConnectorProfile>,
    #[serde(default)]
    pub jira_profile: Option<JiraConnectionProfile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpConnectorProfile {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JiraConnectionProfile {
    pub base_url: String,
    pub auth_mode: String,
    pub username: String,
    pub command: String,
    pub args: Vec<String>,
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

    pub fn redacted_snapshot(&self) -> Result<AppSecrets, String> {
        let mut secrets = self
            .secrets
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        secrets.ai_api_keys.clear();
        secrets.mcp_env.clear();
        secrets.jira_api_token.clear();
        secrets.jira_personal_access_token.clear();
        secrets.jira_password.clear();
        Ok(secrets)
    }

    pub fn ai_provider_statuses(&self) -> Result<HashMap<String, bool>, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok(secrets
            .ai_api_keys
            .iter()
            .map(|(provider_id, value)| (provider_id.clone(), !value.trim().is_empty()))
            .collect())
    }

    pub fn mcp_environment_statuses(&self) -> Result<HashMap<String, Vec<String>>, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok(secrets
            .mcp_env
            .iter()
            .map(|(server_id, values)| {
                let mut keys = values.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                (server_id.clone(), keys)
            })
            .collect())
    }

    pub fn save_mcp_environment(
        &self,
        server_id: &str,
        command: String,
        args: Vec<String>,
        environment: HashMap<String, String>,
    ) -> Result<(), String> {
        let server_id = server_id.trim();
        if server_id.is_empty() {
            return Err("MCP server ID is required.".to_string());
        }
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        let mut next = current.clone();
        if command.trim().is_empty() {
            return Err("MCP connector command is required.".to_string());
        }
        next.mcp_connectors.insert(
            server_id.to_string(),
            McpConnectorProfile {
                command: command.trim().to_string(),
                args,
            },
        );
        if !environment.is_empty() {
            let values = next.mcp_env.entry(server_id.to_string()).or_default();
            values.extend(
                environment
                    .into_iter()
                    .filter(|(_, value)| !value.trim().is_empty()),
            );
        }
        save_app_secrets(next.clone())?;
        *current = next;
        Ok(())
    }

    pub fn save_ai_api_key(&self, provider_id: &str, api_key: Option<&str>) -> Result<(), String> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            return Err("AI provider ID is required.".to_string());
        }
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        let mut next = current.clone();
        match api_key.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                next.ai_api_keys
                    .insert(provider_id.to_string(), value.to_string());
            }
            None => {
                next.ai_api_keys.remove(provider_id);
            }
        }
        save_app_secrets(next.clone())?;
        *current = next;
        Ok(())
    }

    pub fn save_from_renderer(&self, mut incoming: AppSecrets) -> Result<(), String> {
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        incoming.mcp_connectors = current.mcp_connectors.clone();
        incoming.jira_profile = current.jira_profile.clone();
        if incoming.jira_api_token.is_empty() {
            incoming.jira_api_token = current.jira_api_token.clone();
        }
        if incoming.jira_personal_access_token.is_empty() {
            incoming.jira_personal_access_token = current.jira_personal_access_token.clone();
        }
        if incoming.jira_password.is_empty() {
            incoming.jira_password = current.jira_password.clone();
        }
        if incoming.ai_api_keys.is_empty() {
            incoming.ai_api_keys = current.ai_api_keys.clone();
        } else {
            incoming.ai_api_keys = current
                .ai_api_keys
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .chain(incoming.ai_api_keys)
                .collect();
        }
        if incoming.mcp_env.is_empty() {
            incoming.mcp_env = current.mcp_env.clone();
        } else {
            incoming.mcp_env = current
                .mcp_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .chain(incoming.mcp_env)
                .collect();
        }
        save_app_secrets(incoming.clone())?;
        *current = incoming;
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

    pub fn mcp_connector(&self, server_id: &str) -> Result<McpConnectorProfile, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        secrets
            .mcp_connectors
            .get(server_id)
            .cloned()
            .ok_or_else(|| format!("MCP connector profile '{server_id}' was not found."))
    }

    pub fn jira_credentials(&self) -> Result<(String, String, String), String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok((
            secrets.jira_api_token.clone(),
            secrets.jira_personal_access_token.clone(),
            secrets.jira_password.clone(),
        ))
    }

    pub fn jira_secret_statuses(&self) -> Result<HashMap<String, bool>, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        Ok(HashMap::from([
            (
                "api_token".to_string(),
                !secrets.jira_api_token.trim().is_empty(),
            ),
            (
                "personal_access_token".to_string(),
                !secrets.jira_personal_access_token.trim().is_empty(),
            ),
            (
                "password".to_string(),
                !secrets.jira_password.trim().is_empty(),
            ),
        ]))
    }

    pub fn save_jira_secret(&self, secret_type: &str, value: Option<&str>) -> Result<(), String> {
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        let mut next = current.clone();
        let target = match secret_type {
            "api_token" => &mut next.jira_api_token,
            "personal_access_token" => &mut next.jira_personal_access_token,
            "password" => &mut next.jira_password,
            _ => return Err("Unknown Jira secret type.".to_string()),
        };
        *target = value.unwrap_or_default().trim().to_string();
        save_app_secrets(next.clone())?;
        *current = next;
        Ok(())
    }

    pub fn save_jira_profile(&self, profile: JiraConnectionProfile) -> Result<(), String> {
        let url = reqwest::Url::parse(profile.base_url.trim())
            .map_err(|_| "Jira URL is invalid.".to_string())?;
        if url.scheme() != "https" {
            return Err("Jira URL must use HTTPS.".to_string());
        }
        if !matches!(profile.auth_mode.as_str(), "api_token" | "pat" | "password") {
            return Err("Jira auth mode is invalid.".to_string());
        }
        let mut current = self.secrets.lock().map_err(|error| error.to_string())?;
        let mut next = current.clone();
        next.jira_profile = Some(profile);
        save_app_secrets(next.clone())?;
        *current = next;
        Ok(())
    }

    pub fn jira_profile(&self) -> Result<JiraConnectionProfile, String> {
        let secrets = self.secrets.lock().map_err(|error| error.to_string())?;
        secrets
            .jira_profile
            .clone()
            .ok_or_else(|| "Jira connection profile was not found.".to_string())
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
                jira_api_token: "jira-token".to_string(),
                ..AppSecrets::default()
            })),
        };

        assert_eq!(store.ai_api_key("openai").unwrap(), "token");
        assert_eq!(
            store.mcp_environment("spacesly-jira").unwrap()["JIRA_TOKEN"],
            "secret"
        );
        assert!(store.ai_api_key("missing").unwrap().is_empty());
        assert!(store.redacted_snapshot().unwrap().ai_api_keys.is_empty());
        assert_eq!(store.ai_provider_statuses().unwrap()["openai"], true);
        assert_eq!(
            store.mcp_environment_statuses().unwrap()["jira"],
            vec!["JIRA_TOKEN"]
        );
        assert!(store.redacted_snapshot().unwrap().mcp_env.is_empty());
        assert!(store.redacted_snapshot().unwrap().jira_api_token.is_empty());
        assert!(store.jira_secret_statuses().unwrap()["api_token"]);
    }
}
