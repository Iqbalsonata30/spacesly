use crate::domain::entity::Workspace;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedWorkspace {
    pub saved_at: u64,
    pub size_bytes: u64,
    pub workspace: Workspace,
    pub deleted_card_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CachePayload<'a> {
    saved_at: u64,
    workspace: &'a Workspace,
    deleted_card_ids: &'a [String],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCachePayload {
    saved_at: u64,
    workspace: Workspace,
    #[serde(default)]
    deleted_card_ids: Vec<String>,
}

pub fn load_cached_workspace() -> Result<Option<CachedWorkspace>, String> {
    let path = cache_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read workspace cache: {error}"))?;
    let payload: StoredCachePayload = serde_json::from_str(&raw)
        .map_err(|error| format!("Failed to parse workspace cache: {error}"))?;

    Ok(Some(CachedWorkspace {
        saved_at: payload.saved_at,
        size_bytes: raw.len() as u64,
        workspace: payload.workspace,
        deleted_card_ids: payload.deleted_card_ids,
    }))
}

pub fn save_cached_workspace(
    workspace: Workspace,
    deleted_card_ids: Vec<String>,
) -> Result<CachedWorkspace, String> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create workspace cache directory: {error}"))?;
    }

    let saved_at = now_millis()?;
    let payload = CachePayload {
        saved_at,
        workspace: &workspace,
        deleted_card_ids: &deleted_card_ids,
    };
    let raw = serde_json::to_string(&payload)
        .map_err(|error| format!("Failed to encode workspace cache: {error}"))?;

    fs::write(&path, &raw).map_err(|error| format!("Failed to write workspace cache: {error}"))?;

    Ok(CachedWorkspace {
        saved_at,
        size_bytes: raw.len() as u64,
        workspace,
        deleted_card_ids,
    })
}

fn cache_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("workspace-cache.json"))
}

fn config_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".config")))
        .ok_or_else(|| "Cannot resolve a config directory for workspace cache.".to_string())?;
    Ok(base.join("spacesly"))
}

fn now_millis() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before Unix epoch: {error}"))?
        .as_millis() as u64)
}
