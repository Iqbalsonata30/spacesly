use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionRun {
    pub run_id: String,
    pub contract: Value,
    pub status: String,
    pub current_step_ids: Vec<String>,
    pub step_runs: BTreeMap<String, StepRun>,
    pub started_at: String,
    pub completed_at: Option<String>,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StepRun {
    pub step_id: String,
    pub status: String,
    pub attempt: u32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub summary: Option<String>,
    #[serde(default)]
    pub lease_owner: Option<String>,
    #[serde(default)]
    pub lease_expires_at: Option<u64>,
}
