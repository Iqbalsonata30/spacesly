//! Domain model for scheduler-owned Task Sessions.
//!
//! A Task Session is created in `queued`, assigned to at most one Worker, and retained after its
//! terminal transition until the execution engine explicitly removes it. Lifecycle mutations are
//! owned by the Scheduler store and exposed here only as read-only projections.

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Current schema version for durable Task Session execution envelopes.
pub const TASK_SESSION_ENVELOPE_VERSION: u32 = 1;

/// Runtime category requested by a versioned Task Session envelope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSessionKind {
    /// Autonomous task execution using an execution contract.
    Agent,
    /// Conversation turn linked to a durable conversation.
    Chat,
    /// Proposed edit generation linked to a workspace document.
    Edit,
}

/// Non-secret immutable references required to reconstruct a future runtime assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSessionEnvelopeV1 {
    /// Workspace whose trusted runtime context must be resolved at dispatch time.
    pub workspace_id: String,
    /// Requested runtime category.
    pub kind: TaskSessionKind,
    /// Optional card or document subject identifier.
    pub subject_id: Option<String>,
    /// Durable conversation identifier when the task belongs to a conversation.
    pub conversation_id: Option<String>,
    /// Durable execution-run identifier when the task executes a contract.
    pub execution_run_id: Option<String>,
    /// Digest of the immutable execution contract or prompt context.
    pub context_digest: String,
    /// Backend-resolvable provider/runtime profile identifier.
    pub runtime_profile_id: String,
    /// Model identifier validated by the backend provider registry.
    pub model: String,
    /// Non-secret connector identifiers requested by this session.
    pub connector_ids: Vec<String>,
    /// Capability names requested by this session; grants remain a separate authority.
    pub requested_capabilities: Vec<String>,
    /// Prompt-template revision used for deterministic audit and retry decisions.
    pub prompt_template_version: String,
    /// Optional workspace/context revision captured when the session was submitted.
    pub context_revision: Option<String>,
    /// Optional rules revision captured when the session was submitted.
    pub rules_revision: Option<String>,
    /// Optional skills revision captured when the session was submitted.
    pub skills_revision: Option<String>,
}

impl TaskSessionEnvelopeV1 {
    /// Validates fields required before the envelope may be persisted.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("workspace_id", self.workspace_id.as_str()),
            ("context_digest", self.context_digest.as_str()),
            ("runtime_profile_id", self.runtime_profile_id.as_str()),
            ("model", self.model.as_str()),
            (
                "prompt_template_version",
                self.prompt_template_version.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(format!("Task Session envelope field '{name}' is required."));
            }
        }
        if self.connector_ids.iter().any(|value| {
            value.trim().is_empty()
                || value != value.trim()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) {
            return Err(
                "Task Session connector IDs must be non-empty canonical values.".to_string(),
            );
        }
        if self
            .requested_capabilities
            .iter()
            .any(|value| value.trim().is_empty() || value != value.trim())
        {
            return Err(
                "Task Session capabilities must be non-empty canonical values.".to_string(),
            );
        }
        let connectors = self
            .connector_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if connectors.len() != self.connector_ids.len() {
            return Err("Task Session connector IDs must be unique.".to_string());
        }
        let external_capabilities = self
            .requested_capabilities
            .iter()
            .filter_map(|capability| capability.strip_prefix("external_tools:"))
            .collect::<HashSet<_>>();
        if self
            .connector_ids
            .iter()
            .any(|connector| !external_capabilities.contains(connector.as_str()))
            || external_capabilities
                .iter()
                .any(|connector| !connectors.contains(connector))
        {
            return Err(
                "Task Session connectors and external tool capabilities must match.".to_string(),
            );
        }
        Ok(())
    }

    /// Validates ownership required by the live Agent runtime.
    ///
    /// This is stricter than persistence validation so old durable envelopes remain readable while
    /// new scheduler-owned Agent submissions explicitly bind conversation, prompt/context revision,
    /// Agent runtime profile revisions, MCP connector context, progress, timeline, and cancellation
    /// to one Task Session assignment.
    pub fn validate_agent_runtime_ownership(&self) -> Result<(), String> {
        self.validate()?;
        if self.kind != TaskSessionKind::Agent {
            return Err(
                "Live Task Session runtime currently accepts Agent sessions only.".to_string(),
            );
        }
        for (name, value) in [
            ("conversation_id", self.conversation_id.as_deref()),
            ("execution_run_id", self.execution_run_id.as_deref()),
            ("context_revision", self.context_revision.as_deref()),
            ("rules_revision", self.rules_revision.as_deref()),
            ("skills_revision", self.skills_revision.as_deref()),
        ] {
            if value.is_none_or(|value| value.trim().is_empty() || value != value.trim()) {
                return Err(format!(
                    "Agent Task Session ownership field '{name}' is required."
                ));
            }
        }
        Ok(())
    }
}

/// Versioned durable Task Session execution envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskSessionEnvelope {
    /// Initial non-secret execution-envelope schema.
    V1(TaskSessionEnvelopeV1),
}

#[derive(Deserialize, Serialize)]
struct TaskSessionEnvelopeWire {
    schema_version: u32,
    session: TaskSessionEnvelopeV1,
}

impl Serialize for TaskSessionEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::V1(session) => TaskSessionEnvelopeWire {
                schema_version: TASK_SESSION_ENVELOPE_VERSION,
                session: session.clone(),
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TaskSessionEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TaskSessionEnvelopeWire::deserialize(deserializer)?;
        match wire.schema_version {
            TASK_SESSION_ENVELOPE_VERSION => Ok(Self::V1(wire.session)),
            version => Err(D::Error::custom(format!(
                "Unsupported Task Session envelope schema version {version}."
            ))),
        }
    }
}

impl TaskSessionEnvelope {
    /// Returns the numeric schema version persisted with this envelope.
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::V1(_) => TASK_SESSION_ENVELOPE_VERSION,
        }
    }

    /// Validates the selected envelope version.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::V1(envelope) => envelope.validate(),
        }
    }
}

/// Semantic progress projection owned by one Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskProgress {
    /// Stable semantic phase name, such as `queued`, `executing`, or `verifying`.
    pub phase: String,
    /// Completed work units in the current phase.
    pub completed: u64,
    /// Optional total work units; `None` represents indeterminate progress.
    pub total: Option<u64>,
}

impl TaskProgress {
    /// Validates semantic progress before it is persisted.
    pub fn validate(&self) -> Result<(), String> {
        if self.phase.trim().is_empty() {
            return Err("Task progress phase is required.".to_string());
        }
        if self.total.is_some_and(|total| self.completed > total) {
            return Err("Task progress cannot exceed its total.".to_string());
        }
        Ok(())
    }
}

/// Broad category of an append-only Task Session event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSessionEventKind {
    /// Scheduler or runtime lifecycle transition.
    Lifecycle,
    /// Human-readable activity suitable for timeline projection.
    Activity,
    /// Semantic progress update.
    Progress,
    /// Runtime/provider event that does not invoke a tool.
    Runtime,
    /// Tool-call lifecycle event.
    Tool,
}

/// Event payload submitted by a Scheduler or assignment-local event sink.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskSessionEventInput {
    /// Broad event category.
    pub kind: TaskSessionEventKind,
    /// Structured, redacted event payload.
    pub payload: Value,
    /// Optional progress projection updated atomically with this event.
    pub progress: Option<TaskProgress>,
}

/// Durable append-only Task Session event record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskSessionEvent {
    /// Store-generated event identifier.
    pub id: u64,
    /// Owning Task Session.
    pub session_id: TaskSessionId,
    /// Assignment attempt associated with the event, when applicable.
    pub attempt_id: Option<u64>,
    /// Fencing token associated with the assignment, or zero for Scheduler lifecycle events.
    pub fencing_token: u64,
    /// Monotonic sequence within one Task Session.
    pub sequence: u64,
    /// Broad event category.
    pub kind: TaskSessionEventKind,
    /// Structured, redacted event payload.
    pub payload: Value,
    /// Progress projection carried by this event, when applicable.
    pub progress: Option<TaskProgress>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: u64,
}

/// Durable capability authority granted independently from an execution request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskCapabilityGrant {
    /// Capability name enforced at the final pre-side-effect boundary.
    pub capability: String,
    /// Auditable authority that approved this grant.
    pub grant_source: String,
    /// Grant timestamp in Unix milliseconds.
    pub granted_at: u64,
}

/// Best-effort wake-up carrying the latest committed journal cursor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSessionUpdate {
    /// Task Session whose durable projection advanced.
    pub session_id: TaskSessionId,
    /// Latest sequence known committed when this notification was emitted.
    pub latest_sequence: u64,
}

/// Bounded durable journal page returned to replay consumers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskSessionEventPage {
    /// Ordered events with sequence greater than the requested cursor.
    pub events: Vec<TaskSessionEvent>,
    /// Last sequence included in this page, or the requested cursor for an empty page.
    pub next_cursor: u64,
    /// True when another bounded page remains available.
    pub has_more: bool,
}

/// Current lifecycle status for one tool call within a Task Session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskToolStatus {
    /// Tool call has started but no completion event has been observed.
    Running,
    /// Tool call completed successfully.
    Succeeded,
    /// Tool call completed with a runtime-reported failure.
    Failed,
}

/// Durable read projection for one tool call, derived from the Task Session event journal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskToolCallState {
    /// Runtime-generated tool call identifier scoped to one Task Session attempt.
    pub tool_call_id: String,
    /// Display name of the invoked tool.
    pub tool_name: String,
    /// Latest known tool call lifecycle status.
    pub status: TaskToolStatus,
    /// Risk level reported by the runtime, if available.
    pub risk: Option<String>,
    /// Digest of redacted tool arguments, if available.
    pub arguments_digest: Option<String>,
    /// Redacted display context for timeline/UI rendering.
    pub display_context: Option<Value>,
    /// Assignment attempt that emitted the latest state.
    pub attempt_id: Option<u64>,
    /// Fencing token that authorized the latest state.
    pub fencing_token: u64,
    /// Event sequence that first introduced this tool call.
    pub started_sequence: u64,
    /// Event sequence that completed this tool call, if observed.
    pub completed_sequence: Option<u64>,
    /// Timestamp of the latest observed tool event in Unix milliseconds.
    pub updated_at: u64,
}

/// Durable read projection of all tool calls owned by one Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskToolState {
    /// Owning Task Session.
    pub session_id: TaskSessionId,
    /// Tool calls ordered by first observed event sequence.
    pub calls: Vec<TaskToolCallState>,
}

impl TaskToolState {
    /// Projects current tool state from the append-only event journal.
    pub fn from_events(session_id: TaskSessionId, events: &[TaskSessionEvent]) -> Self {
        let mut calls = BTreeMap::<String, TaskToolCallState>::new();
        for event in events.iter().filter(|event| {
            event.session_id == session_id && event.kind == TaskSessionEventKind::Tool
        }) {
            let event_type = event.payload.get("type").and_then(Value::as_str);
            let Some(tool_call_id) = event.payload.get("tool_call_id").and_then(Value::as_str)
            else {
                continue;
            };
            let tool_name = event
                .payload
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let risk = event
                .payload
                .get("risk")
                .and_then(Value::as_str)
                .map(str::to_string);
            let arguments_digest = event
                .payload
                .get("arguments_digest")
                .and_then(Value::as_str)
                .map(str::to_string);
            let display_context = event.payload.get("display_context").cloned();

            let entry =
                calls
                    .entry(tool_call_id.to_string())
                    .or_insert_with(|| TaskToolCallState {
                        tool_call_id: tool_call_id.to_string(),
                        tool_name: tool_name.clone(),
                        status: TaskToolStatus::Running,
                        risk: risk.clone(),
                        arguments_digest: arguments_digest.clone(),
                        display_context: display_context.clone(),
                        attempt_id: event.attempt_id,
                        fencing_token: event.fencing_token,
                        started_sequence: event.sequence,
                        completed_sequence: None,
                        updated_at: event.created_at,
                    });
            entry.tool_name = tool_name;
            entry.risk = risk;
            entry.arguments_digest = arguments_digest;
            entry.display_context = display_context;
            entry.attempt_id = event.attempt_id;
            entry.fencing_token = event.fencing_token;
            entry.updated_at = event.created_at;
            if event_type == Some("tool_completed") {
                entry.status = if event
                    .payload
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    TaskToolStatus::Succeeded
                } else {
                    TaskToolStatus::Failed
                };
                entry.completed_sequence = Some(event.sequence);
            }
        }
        let mut calls = calls.into_values().collect::<Vec<_>>();
        calls.sort_by_key(|call| call.started_sequence);
        Self { session_id, calls }
    }
}

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
    /// Execution could not continue without operator action.
    Blocked,
    /// Session was cancelled before dispatch or during execution.
    Cancelled,
}

impl TaskSessionState {
    /// Returns true when the session cannot transition without an explicit retry.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Blocked | Self::Cancelled
        )
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

    /// Serializes a validated, versioned execution envelope into the durable request payload.
    pub fn from_envelope(
        label: impl Into<String>,
        envelope: &TaskSessionEnvelope,
    ) -> Result<Self, String> {
        envelope.validate()?;
        let payload = serde_json::to_string(envelope)
            .map_err(|error| format!("Failed to serialize Task Session envelope: {error}"))?;
        Ok(Self {
            label: label.into(),
            payload,
        })
    }

    /// Decodes the versioned envelope, returning `None` for backward-compatible mock requests.
    pub fn envelope(&self) -> Result<Option<TaskSessionEnvelope>, String> {
        if self.payload.trim().is_empty() {
            return Ok(None);
        }
        let envelope = serde_json::from_str::<TaskSessionEnvelope>(&self.payload)
            .map_err(|error| format!("Failed to decode Task Session envelope: {error}"))?;
        envelope.validate()?;
        Ok(Some(envelope))
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
    /// Latest semantic progress projection, if the session has reported progress.
    pub progress: Option<TaskProgress>,
    /// Sequence of the latest event included in this projection; zero means no events exist.
    pub last_event_sequence: u64,
    /// Failure description when the terminal state is `failed`.
    pub error: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: u64,
    /// First execution timestamp in Unix milliseconds.
    pub started_at: Option<u64>,
    /// Terminal timestamp in Unix milliseconds.
    pub completed_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_envelope_round_trips_without_secrets() {
        let envelope = TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: Some("card-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "sha256:contract".to_string(),
            runtime_profile_id: "openai".to_string(),
            model: "gpt-5".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec![
                "workspace_read".to_string(),
                "external_tools:jira".to_string(),
            ],
            prompt_template_version: "agent-v1".to_string(),
            context_revision: Some("context-7".to_string()),
            rules_revision: Some("rules-2".to_string()),
            skills_revision: Some("skills-3".to_string()),
        });
        let request = TaskRequest::from_envelope("agent task", &envelope).expect("serialized");
        let encoded = serde_json::to_value(&envelope).expect("encoded");
        assert_eq!(encoded["schema_version"], serde_json::json!(1));
        assert_eq!(request.envelope().expect("decoded"), Some(envelope));
        assert!(!request.payload.contains("api_key"));
        assert!(!request.payload.contains("secret"));
    }

    #[test]
    fn envelope_validation_rejects_missing_runtime_identity() {
        let envelope = TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: None,
            context_digest: "digest".to_string(),
            runtime_profile_id: String::new(),
            model: "model".to_string(),
            connector_ids: Vec::new(),
            requested_capabilities: Vec::new(),
            prompt_template_version: "v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        });
        assert!(TaskRequest::from_envelope("invalid", &envelope).is_err());
    }

    #[test]
    fn empty_legacy_payload_remains_backward_compatible() {
        assert_eq!(
            TaskRequest::new("legacy").envelope().expect("decoded"),
            None
        );
    }

    #[test]
    fn envelope_rejects_non_canonical_capability_names() {
        let envelope = TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: None,
            context_digest: "digest".to_string(),
            runtime_profile_id: "profile".to_string(),
            model: "model".to_string(),
            connector_ids: Vec::new(),
            requested_capabilities: vec![" shell ".to_string()],
            prompt_template_version: "v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        };
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn envelope_rejects_connector_glob_characters() {
        let envelope = TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: None,
            context_digest: "digest".to_string(),
            runtime_profile_id: "profile".to_string(),
            model: "model".to_string(),
            connector_ids: vec!["jira*".to_string()],
            requested_capabilities: vec!["external_tools:jira*".to_string()],
            prompt_template_version: "v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        };
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn agent_runtime_ownership_requires_conversation_and_revisions() {
        let mut envelope = TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "digest".to_string(),
            runtime_profile_id: "profile".to_string(),
            model: "model".to_string(),
            connector_ids: Vec::new(),
            requested_capabilities: Vec::new(),
            prompt_template_version: "v1".to_string(),
            context_revision: Some("context-1".to_string()),
            rules_revision: Some("rules-1".to_string()),
            skills_revision: Some("skills-1".to_string()),
        };
        assert!(envelope.validate_agent_runtime_ownership().is_ok());

        envelope.conversation_id = None;
        assert!(envelope.validate_agent_runtime_ownership().is_err());
    }

    #[test]
    fn tool_state_projects_started_and_completed_events_for_one_session() {
        let session = TaskSessionId(7);
        let other_session = TaskSessionId(8);
        let events = vec![
            TaskSessionEvent {
                id: 1,
                session_id: session,
                attempt_id: Some(11),
                fencing_token: 1,
                sequence: 1,
                kind: TaskSessionEventKind::Tool,
                payload: serde_json::json!({
                    "type": "tool_started",
                    "tool_call_id": "tool-1",
                    "tool_name": "jira_search",
                    "risk": "low",
                    "arguments_digest": "abc",
                    "display_context": { "query": "ABC-1" }
                }),
                progress: None,
                created_at: 100,
            },
            TaskSessionEvent {
                id: 2,
                session_id: other_session,
                attempt_id: Some(12),
                fencing_token: 1,
                sequence: 1,
                kind: TaskSessionEventKind::Tool,
                payload: serde_json::json!({
                    "type": "tool_completed",
                    "tool_call_id": "tool-1",
                    "tool_name": "jira_search",
                    "success": false
                }),
                progress: None,
                created_at: 101,
            },
            TaskSessionEvent {
                id: 3,
                session_id: session,
                attempt_id: Some(11),
                fencing_token: 1,
                sequence: 2,
                kind: TaskSessionEventKind::Tool,
                payload: serde_json::json!({
                    "type": "tool_completed",
                    "tool_call_id": "tool-1",
                    "tool_name": "jira_search",
                    "success": true,
                    "risk": "low",
                    "arguments_digest": "abc",
                    "display_context": { "query": "ABC-1" }
                }),
                progress: None,
                created_at: 110,
            },
        ];

        let state = TaskToolState::from_events(session, &events);
        assert_eq!(state.session_id, session);
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.calls[0].status, TaskToolStatus::Succeeded);
        assert_eq!(state.calls[0].started_sequence, 1);
        assert_eq!(state.calls[0].completed_sequence, Some(2));
        assert_eq!(state.calls[0].updated_at, 110);
    }
}
