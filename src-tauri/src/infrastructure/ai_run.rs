use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RETAINED_RUNS: usize = 256;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
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
        Ok(true)
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
        run.status = status;
        run.updated_at = now_millis();
        let finished = run.clone();
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
}
