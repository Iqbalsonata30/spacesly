//! Domain model for scheduler-owned Task Sessions.
//!
//! A Task Session is created in `queued`, assigned to at most one Worker, and retained after its
//! terminal transition until the execution engine explicitly removes it. Lifecycle mutations are
//! owned by the Scheduler store and exposed here only as read-only projections.

use serde::{Deserialize, Serialize};

/// Stable identifier assigned to one scheduler-managed task session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct TaskSessionId(
    /// Monotonic identifier value assigned by one Scheduler database.
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
    /// Active assignment attempt identifier.
    pub attempt_id: Option<u64>,
    /// Monotonic token that fences stale Worker completions.
    pub fencing_token: u64,
    /// Lease expiry for a currently running assignment, in Unix milliseconds.
    pub lease_expires_at: Option<u64>,
    /// Failure description when the terminal state is `failed`.
    pub error: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: u64,
    /// First execution timestamp in Unix milliseconds.
    pub started_at: Option<u64>,
    /// Terminal timestamp in Unix milliseconds.
    pub completed_at: Option<u64>,
}
