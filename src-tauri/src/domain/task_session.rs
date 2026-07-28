//! Domain model for scheduler-owned Task Sessions.
//!
//! A Task Session is created in `queued`, assigned to at most one Worker, and retained after its
//! terminal transition until the execution engine explicitly removes it. Mutation methods remain
//! crate-private so only the Scheduler can advance lifecycle state.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Stable identifier assigned to one scheduler-managed task session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct TaskSessionId(
    /// Monotonic identifier value assigned by one Scheduler instance.
    pub u64,
);

/// Lifecycle state of a task session owned by the scheduler.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSessionState {
    /// Session is waiting in the Scheduler FIFO queue.
    Queued,
    /// Session is assigned to exactly one Worker.
    Running,
    /// Cancellation is requested and Worker cleanup is still in progress.
    Cancelling,
    /// Mock execution completed successfully.
    Succeeded,
    /// Mock execution returned an error or panicked.
    Failed,
    /// Session was cancelled before dispatch or during execution.
    Cancelled,
}

impl TaskSessionState {
    /// Returns true when the session cannot transition without an explicit retry.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Immutable task data submitted to the execution engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskRequest {
    /// Human-readable task label used for diagnostics and mock execution.
    pub label: String,
    /// Opaque payload reserved for the executor implementation.
    pub payload: String,
}

impl TaskRequest {
    /// Creates a request with an empty opaque payload.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            payload: String::new(),
        }
    }

    /// Creates a request with an explicit opaque executor payload.
    pub fn with_payload(label: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            payload: payload.into(),
        }
    }
}

/// Read-only projection of a scheduler-owned task session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSessionSnapshot {
    /// Stable session identifier.
    pub id: TaskSessionId,
    /// Immutable request assigned to the session.
    pub request: TaskRequest,
    /// Current scheduler-owned lifecycle state.
    pub state: TaskSessionState,
    /// Worker slot currently or most recently assigned to this session.
    pub worker_id: Option<usize>,
    /// Monotonic scheduler dispatch order, used to verify FIFO assignment.
    pub dispatch_sequence: Option<u64>,
    /// Number of execution attempts made by this session.
    pub attempt: u32,
    /// Failure description when the terminal state is `failed`.
    pub error: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: u64,
    /// First execution timestamp in Unix milliseconds.
    pub started_at: Option<u64>,
    /// Terminal timestamp in Unix milliseconds.
    pub completed_at: Option<u64>,
}

pub(crate) struct TaskSession {
    id: TaskSessionId,
    request: TaskRequest,
    state: TaskSessionState,
    worker_id: Option<usize>,
    dispatch_sequence: Option<u64>,
    attempt: u32,
    error: Option<String>,
    created_at: u64,
    started_at: Option<u64>,
    completed_at: Option<u64>,
}

impl TaskSession {
    pub(crate) fn new(id: TaskSessionId, request: TaskRequest) -> Self {
        Self {
            id,
            request,
            state: TaskSessionState::Queued,
            worker_id: None,
            dispatch_sequence: None,
            attempt: 0,
            error: None,
            created_at: now_millis(),
            started_at: None,
            completed_at: None,
        }
    }

    pub(crate) fn state(&self) -> TaskSessionState {
        self.state
    }

    pub(crate) fn request(&self) -> &TaskRequest {
        &self.request
    }

    pub(crate) fn assign(&mut self, worker_id: usize, dispatch_sequence: u64) {
        self.state = TaskSessionState::Running;
        self.worker_id = Some(worker_id);
        self.dispatch_sequence = Some(dispatch_sequence);
        self.attempt = self.attempt.saturating_add(1);
        self.started_at.get_or_insert_with(now_millis);
        self.completed_at = None;
        self.error = None;
    }

    pub(crate) fn request_cancellation(&mut self) {
        self.state = TaskSessionState::Cancelling;
    }

    pub(crate) fn cancel_queued(&mut self) {
        self.state = TaskSessionState::Cancelled;
        self.completed_at = Some(now_millis());
    }

    pub(crate) fn succeed(&mut self) {
        self.state = TaskSessionState::Succeeded;
        self.completed_at = Some(now_millis());
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.state = TaskSessionState::Failed;
        self.error = Some(error);
        self.completed_at = Some(now_millis());
    }

    pub(crate) fn cancel(&mut self) {
        self.state = TaskSessionState::Cancelled;
        self.completed_at = Some(now_millis());
    }

    pub(crate) fn snapshot(&self) -> TaskSessionSnapshot {
        TaskSessionSnapshot {
            id: self.id,
            request: self.request.clone(),
            state: self.state,
            worker_id: self.worker_id,
            dispatch_sequence: self.dispatch_sequence,
            attempt: self.attempt,
            error: self.error.clone(),
            created_at: self.created_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
