use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RETAINED_RUNS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiRunKind {
    Chat,
    Edit,
    Agent,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiRunStatus {
    Queued,
    Running,
    Cancelling,
    Completed,
    Blocked,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AiRun {
    pub run_id: String,
    pub kind: AiRunKind,
    pub status: AiRunStatus,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Default)]
pub struct AiRunRegistry {
    state: Arc<Mutex<HashMap<String, AiRun>>>,
    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    capability_grants: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    sequence: Arc<AtomicU64>,
}

impl AiRunRegistry {
    pub fn begin(&self, run_id: String, kind: AiRunKind) -> Result<AiRun, String> {
        if run_id.trim().is_empty() {
            return Err("AI run ID is required.".to_string());
        }
        let now = now_millis();
        let run = AiRun {
            run_id: run_id.clone(),
            kind,
            status: AiRunStatus::Queued,
            created_at: now,
            updated_at: now,
        };
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if state.contains_key(&run_id) {
            return Err("AI run already exists.".to_string());
        }
        state.insert(run_id, run.clone());
        self.cancellations
            .lock()
            .map_err(|error| error.to_string())?
            .insert(run.run_id.clone(), Arc::new(AtomicBool::new(false)));
        Ok(run)
    }

    pub fn begin_generated(&self, kind: AiRunKind) -> Result<AiRun, String> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.begin(format!("ai-{kind:?}-{}-{sequence}", now_millis()), kind)
    }

    pub fn start(&self, run_id: &str) -> Result<AiRun, String> {
        self.transition(run_id, AiRunStatus::Running)
    }

    pub fn cancel(&self, run_id: &str) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let Some(run) = state.get_mut(run_id) else {
            return Ok(false);
        };
        if matches!(
            run.status,
            AiRunStatus::Completed
                | AiRunStatus::Blocked
                | AiRunStatus::Failed
                | AiRunStatus::Cancelled
        ) {
            return Ok(false);
        }
        run.status = AiRunStatus::Cancelling;
        run.updated_at = now_millis();
        if let Some(token) = self
            .cancellations
            .lock()
            .map_err(|error| error.to_string())?
            .get(run_id)
        {
            token.store(true, Ordering::Release);
        }
        Ok(true)
    }

    pub fn cancellation(&self, run_id: &str) -> Result<Arc<AtomicBool>, String> {
        self.cancellations
            .lock()
            .map_err(|error| error.to_string())?
            .get(run_id)
            .cloned()
            .ok_or_else(|| "AI run cancellation token was not found.".to_string())
    }

    pub fn grant_capabilities(
        &self,
        run_id: &str,
        capabilities: Vec<String>,
    ) -> Result<(), String> {
        const ALLOWED: [&str; 5] = [
            "workspace_read",
            "workspace_write",
            "shell",
            "git",
            "external_tools",
        ];
        let state = self.state.lock().map_err(|error| error.to_string())?;
        let run = state
            .get(run_id)
            .ok_or_else(|| "AI run was not found.".to_string())?;
        if run.kind != AiRunKind::Agent || run.status != AiRunStatus::Queued {
            return Err("Capabilities can only be granted to a queued Agent run.".to_string());
        }
        let capabilities = capabilities
            .into_iter()
            .map(|value| value.trim().to_string())
            .collect::<HashSet<_>>();
        if capabilities.is_empty()
            || capabilities
                .iter()
                .any(|value| !ALLOWED.contains(&value.as_str()))
        {
            return Err("AI capability grant contains an unsupported capability.".to_string());
        }
        drop(state);
        self.capability_grants
            .lock()
            .map_err(|error| error.to_string())?
            .insert(run_id.to_string(), capabilities);
        Ok(())
    }

    pub fn require_capabilities(&self, run_id: &str, required: &[&str]) -> Result<(), String> {
        let grants = self
            .capability_grants
            .lock()
            .map_err(|error| error.to_string())?;
        let granted = grants
            .get(run_id)
            .ok_or_else(|| "Agent capability approval is required before execution.".to_string())?;
        if required.iter().any(|value| !granted.contains(*value)) {
            return Err("Agent capability approval does not cover this execution.".to_string());
        }
        Ok(())
    }

    pub fn finish(&self, run_id: &str, status: AiRunStatus) -> Result<AiRun, String> {
        if !matches!(
            status,
            AiRunStatus::Completed
                | AiRunStatus::Blocked
                | AiRunStatus::Failed
                | AiRunStatus::Cancelled
        ) {
            return Err("AI run must finish in a terminal status.".to_string());
        }
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let run = state
            .get_mut(run_id)
            .ok_or_else(|| "AI run was not found.".to_string())?;
        if matches!(
            run.status,
            AiRunStatus::Completed
                | AiRunStatus::Blocked
                | AiRunStatus::Failed
                | AiRunStatus::Cancelled
        ) {
            if run.status == status {
                return Ok(run.clone());
            }
            return Err("AI run terminal status is immutable.".to_string());
        }
        run.status = status;
        run.updated_at = now_millis();
        let finished = run.clone();
        self.cancellations
            .lock()
            .map_err(|error| error.to_string())?
            .remove(run_id);
        self.capability_grants
            .lock()
            .map_err(|error| error.to_string())?
            .remove(run_id);
        prune_runs(&mut state);
        Ok(finished)
    }

    pub fn get(&self, run_id: &str) -> Result<Option<AiRun>, String> {
        Ok(self
            .state
            .lock()
            .map_err(|error| error.to_string())?
            .get(run_id)
            .cloned())
    }

    fn transition(&self, run_id: &str, status: AiRunStatus) -> Result<AiRun, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let run = state
            .get_mut(run_id)
            .ok_or_else(|| "AI run was not found.".to_string())?;
        if status == AiRunStatus::Running && run.status != AiRunStatus::Queued {
            return Err("AI run cannot start from its current status.".to_string());
        }
        run.status = status;
        run.updated_at = now_millis();
        Ok(run.clone())
    }
}

fn prune_runs(state: &mut HashMap<String, AiRun>) {
    if state.len() <= MAX_RETAINED_RUNS {
        return;
    }
    let mut terminal = state
        .values()
        .filter(|run| {
            matches!(
                run.status,
                AiRunStatus::Completed
                    | AiRunStatus::Blocked
                    | AiRunStatus::Failed
                    | AiRunStatus::Cancelled
            )
        })
        .map(|run| (run.updated_at, run.run_id.clone()))
        .collect::<Vec<_>>();
    terminal.sort_unstable();
    for (_, run_id) in terminal.into_iter().take(state.len() - MAX_RETAINED_RUNS) {
        state.remove(&run_id);
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{AiRunKind, AiRunRegistry, AiRunStatus};

    #[test]
    fn run_lifecycle_is_backend_owned_and_terminal() {
        let registry = AiRunRegistry::default();
        let queued = registry
            .begin("run-1".to_string(), AiRunKind::Chat)
            .unwrap();
        assert_eq!(queued.status, AiRunStatus::Queued);
        assert_eq!(
            registry.start("run-1").unwrap().status,
            AiRunStatus::Running
        );
        assert!(registry.cancel("run-1").unwrap());
        assert!(registry.start("run-1").is_err());
        assert_eq!(
            registry
                .finish("run-1", AiRunStatus::Cancelled)
                .unwrap()
                .status,
            AiRunStatus::Cancelled
        );
        assert!(!registry.cancel("run-1").unwrap());
    }

    #[test]
    fn duplicate_run_ids_are_rejected() {
        let registry = AiRunRegistry::default();
        registry
            .begin("run-1".to_string(), AiRunKind::Agent)
            .unwrap();
        assert!(registry
            .begin("run-1".to_string(), AiRunKind::Edit)
            .is_err());
    }

    #[test]
    fn agent_execution_requires_explicit_capability_grants() {
        let registry = AiRunRegistry::default();
        registry
            .begin("agent-1".to_string(), AiRunKind::Agent)
            .unwrap();
        assert!(registry
            .require_capabilities("agent-1", &["workspace_write"])
            .is_err());

        registry
            .grant_capabilities(
                "agent-1",
                vec!["workspace_read".to_string(), "workspace_write".to_string()],
            )
            .unwrap();

        registry
            .require_capabilities("agent-1", &["workspace_read", "workspace_write"])
            .unwrap();
        assert!(registry
            .require_capabilities("agent-1", &["shell"])
            .is_err());
    }
}
