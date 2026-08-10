//! Domain model for scheduler-owned Task Sessions.
//!
//! A Task Session is created in `queued`, assigned to at most one Worker, and retained after its
//! terminal transition until the execution engine explicitly removes it. Lifecycle mutations are
//! owned by the Scheduler store and exposed here only as read-only projections.

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Current schema version for durable Task Session execution envelopes.
pub const TASK_SESSION_ENVELOPE_VERSION: u32 = 2;
const TASK_SESSION_ENVELOPE_V1: u32 = 1;
const MAX_TASK_CHAT_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_TASK_CHAT_CONTEXT_BYTES: usize = 512 * 1024;
const MAX_TASK_EDIT_CONTENT_BYTES: usize = 256 * 1024;
const MAX_TASK_EDIT_CONTEXT_FILES: usize = 8;
const MAX_TASK_EDIT_CONTEXT_FILE_BYTES: usize = 128 * 1024;
const MAX_TASK_EDIT_COMBINED_BYTES: usize = 512 * 1024;
const MAX_TASK_EDIT_SELECTION_BYTES: usize = 64 * 1024;
const MAX_TASK_EDIT_DIAGNOSTICS: usize = 50;
const MAX_TASK_EDIT_DIAGNOSTIC_BYTES: usize = 2 * 1024;
const MAX_TASK_PROMPT_METADATA_BYTES: usize = 4 * 1024;
const MAX_TASK_EDIT_INSTRUCTION_BYTES: usize = 64 * 1024;
const MAX_TASK_PROMPT_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;

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

/// Immutable Chat input persisted atomically with a Task Session submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskChatInputV2 {
    /// Durable user-message identifier already committed to the owning conversation.
    pub message_id: String,
    /// Durable sequence of the user message within the owning conversation.
    pub message_sequence: u64,
    /// Exact user message sent to the runtime.
    pub message: String,
    /// Immutable workspace/terminal context snapshot.
    pub terminal_context: Option<String>,
    /// Immutable prior-turn and workspace-selection context snapshot.
    pub session_context: Option<String>,
}

/// Immutable selected range supplied to an Edit Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskEditSelectionV2 {
    /// Zero-based selection start line.
    pub start_line: usize,
    /// Zero-based UTF-16 selection start character.
    pub start_character: usize,
    /// Zero-based selection end line.
    pub end_line: usize,
    /// Zero-based UTF-16 selection end character.
    pub end_character: usize,
    /// Exact selected text snapshot.
    pub text: String,
}

/// Immutable context-file snapshot supplied to an Edit Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskEditContextFileV2 {
    /// Workspace-relative context file path.
    pub file_path: String,
    /// Exact context file content snapshot.
    pub content: String,
}

/// Immutable Edit input persisted atomically with a Task Session submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskEditInputV2 {
    /// Workspace-relative target file path.
    pub file_path: String,
    /// User instruction captured with the source snapshot.
    pub instruction: String,
    /// Complete target file content snapshot.
    pub content: String,
    /// Optional editor selection captured atomically with the content.
    pub selection: Option<TaskEditSelectionV2>,
    /// Explicit pinned context file snapshots.
    pub context_files: Vec<TaskEditContextFileV2>,
    /// Redacted diagnostics captured with the edit request.
    pub diagnostics: Vec<String>,
}

/// Kind-specific immutable prompt input for envelope schema V2.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "input", rename_all = "snake_case")]
pub enum TaskSessionInputV2 {
    /// Immutable Chat turn input.
    Chat(TaskChatInputV2),
    /// Immutable Edit proposal input.
    Edit(TaskEditInputV2),
}

/// Envelope schema V2 binding common runtime references to immutable Chat/Edit input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSessionEnvelopeV2 {
    /// Common runtime identity, ownership, revisions, and capability references.
    pub session: TaskSessionEnvelopeV1,
    /// Immutable kind-specific prompt input.
    pub prompt_input: TaskSessionInputV2,
}

impl TaskSessionEnvelopeV2 {
    /// Validates kind-specific immutable prompt ownership before persistence or execution.
    pub fn validate(&self) -> Result<(), String> {
        self.session.validate()?;
        if [
            self.session.workspace_id.as_str(),
            self.session.context_digest.as_str(),
            self.session.runtime_profile_id.as_str(),
            self.session.model.as_str(),
            self.session.prompt_template_version.as_str(),
        ]
        .iter()
        .any(|value| value.len() > MAX_TASK_PROMPT_METADATA_BYTES)
        {
            return Err("Prompt Task Session metadata exceeds its size limit.".to_string());
        }
        for (name, value) in [
            ("context_revision", self.session.context_revision.as_deref()),
            ("rules_revision", self.session.rules_revision.as_deref()),
            ("skills_revision", self.session.skills_revision.as_deref()),
        ] {
            if value.is_none_or(|value| value.trim().is_empty() || value != value.trim()) {
                return Err(format!(
                    "Prompt Task Session ownership field '{name}' is required."
                ));
            }
        }
        if !self.session.connector_ids.is_empty()
            || self
                .session
                .requested_capabilities
                .iter()
                .any(|capability| capability.starts_with("external_tools:"))
        {
            return Err(
                "Chat/Edit Task Sessions do not enable MCP connectors or external tools."
                    .to_string(),
            );
        }
        match (&self.session.kind, &self.prompt_input) {
            (TaskSessionKind::Chat, TaskSessionInputV2::Chat(input)) => {
                if self
                    .session
                    .conversation_id
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty() || value != value.trim())
                    || input.message_id.trim().is_empty()
                    || input.message_id != input.message_id.trim()
                    || input.message.trim().is_empty()
                    || input.message != input.message.trim()
                {
                    return Err(
                        "Chat Task Session requires conversation, message ID, and message input."
                            .to_string(),
                    );
                }
                if input.message.len() > MAX_TASK_CHAT_MESSAGE_BYTES
                    || input.message_id.len() > MAX_TASK_PROMPT_METADATA_BYTES
                    || input
                        .terminal_context
                        .as_ref()
                        .is_some_and(|context| context.len() > MAX_TASK_CHAT_CONTEXT_BYTES)
                    || input
                        .session_context
                        .as_ref()
                        .is_some_and(|context| context.len() > MAX_TASK_CHAT_CONTEXT_BYTES)
                {
                    return Err("Chat Task Session input exceeds its size limit.".to_string());
                }
            }
            (TaskSessionKind::Edit, TaskSessionInputV2::Edit(input)) => {
                if input.file_path.trim().is_empty()
                    || input.file_path != input.file_path.trim()
                    || input.instruction.trim().is_empty()
                    || input.instruction != input.instruction.trim()
                {
                    return Err(
                        "Edit Task Session requires a canonical file path and instruction."
                            .to_string(),
                    );
                }
                if input.content.len() > MAX_TASK_EDIT_CONTENT_BYTES
                    || input.file_path.len() > MAX_TASK_PROMPT_METADATA_BYTES
                    || input.instruction.len() > MAX_TASK_EDIT_INSTRUCTION_BYTES
                    || input.context_files.len() > MAX_TASK_EDIT_CONTEXT_FILES
                    || input.selection.as_ref().is_some_and(|selection| {
                        selection.text.len() > MAX_TASK_EDIT_SELECTION_BYTES
                    })
                    || input.diagnostics.len() > MAX_TASK_EDIT_DIAGNOSTICS
                    || input
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.len() > MAX_TASK_EDIT_DIAGNOSTIC_BYTES)
                {
                    return Err("Edit Task Session input exceeds its size limit.".to_string());
                }
                let mut paths = HashSet::new();
                let mut combined_bytes = input.content.len();
                for file in &input.context_files {
                    if file.file_path.trim().is_empty()
                        || file.file_path != file.file_path.trim()
                        || file.file_path == input.file_path
                        || file.content.len() > MAX_TASK_EDIT_CONTEXT_FILE_BYTES
                        || !paths.insert(file.file_path.as_str())
                    {
                        return Err("Edit Task Session context file is invalid.".to_string());
                    }
                    combined_bytes = combined_bytes.saturating_add(file.content.len());
                }
                if combined_bytes > MAX_TASK_EDIT_COMBINED_BYTES {
                    return Err(
                        "Edit Task Session combined content exceeds its size limit.".to_string()
                    );
                }
            }
            _ => {
                return Err(
                    "Task Session kind does not match its immutable prompt input.".to_string(),
                );
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|error| format!("Failed to encode prompt Task Session envelope: {error}"))?;
        if encoded.len() > MAX_TASK_PROMPT_ENVELOPE_BYTES {
            return Err("Prompt Task Session envelope exceeds its size limit.".to_string());
        }
        Ok(())
    }
}

/// Versioned durable Task Session execution envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskSessionEnvelope {
    /// Initial non-secret execution-envelope schema.
    V1(TaskSessionEnvelopeV1),
    /// Immutable kind-specific Chat/Edit prompt-input schema.
    V2(TaskSessionEnvelopeV2),
}

#[derive(Deserialize, Serialize)]
struct TaskSessionEnvelopeWireV1 {
    schema_version: u32,
    session: TaskSessionEnvelopeV1,
}

#[derive(Deserialize, Serialize)]
struct TaskSessionEnvelopeWireV2 {
    schema_version: u32,
    session: TaskSessionEnvelopeV2,
}

impl Serialize for TaskSessionEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::V1(session) => TaskSessionEnvelopeWireV1 {
                schema_version: TASK_SESSION_ENVELOPE_V1,
                session: session.clone(),
            }
            .serialize(serializer),
            Self::V2(session) => TaskSessionEnvelopeWireV2 {
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
        let value = Value::deserialize(deserializer)?;
        let version = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| D::Error::custom("Task Session envelope schema version is required."))?;
        match version {
            1 => serde_json::from_value::<TaskSessionEnvelopeWireV1>(value)
                .map(|wire| Self::V1(wire.session))
                .map_err(D::Error::custom),
            2 => serde_json::from_value::<TaskSessionEnvelopeWireV2>(value)
                .map(|wire| Self::V2(wire.session))
                .map_err(D::Error::custom),
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
            Self::V1(_) => TASK_SESSION_ENVELOPE_V1,
            Self::V2(_) => TASK_SESSION_ENVELOPE_VERSION,
        }
    }

    /// Returns common runtime references regardless of envelope schema version.
    pub fn session(&self) -> &TaskSessionEnvelopeV1 {
        match self {
            Self::V1(session) => session,
            Self::V2(envelope) => &envelope.session,
        }
    }

    /// Validates the selected envelope version.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::V1(envelope) => envelope.validate(),
            Self::V2(envelope) => envelope.validate(),
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

/// Bounded developer-oriented execution trace projected from the authoritative Task Session journal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskExecutionTracePage {
    pub schema_version: u32,
    pub trace_id: String,
    pub task_session_id: TaskSessionId,
    pub subject_id: Option<String>,
    pub execution_run_id: Option<String>,
    pub runtime_profile_id: Option<String>,
    pub model: Option<String>,
    pub opencode_session_id: Option<String>,
    pub coverage: String,
    pub unknown_fields: Vec<String>,
    pub entries: Vec<TaskExecutionTraceEntry>,
    pub next_cursor: u64,
    pub has_more: bool,
}

/// Safe, allowlisted metadata for one durable execution-trace fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskExecutionTraceEntry {
    pub sequence: u64,
    pub attempt_id: Option<u64>,
    pub assignment_attempt: Option<u32>,
    pub fencing_token: u64,
    pub event_type: String,
    pub created_at: u64,
    pub state: Option<String>,
    pub stage: Option<String>,
    pub duration_us: Option<u64>,
    pub outcome: Option<String>,
    pub worker_id: Option<u64>,
    pub runtime_id: Option<String>,
    pub opencode_session_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_success: Option<bool>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub recovery: Option<String>,
    pub approval_operation: Option<String>,
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

/// Read projection for one MCP connector requested by a Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskMcpConnectorContext {
    /// Non-secret connector identifier from the Task Session envelope.
    pub connector_id: String,
    /// Capability required to use this connector through fenced MCP authority.
    pub capability: String,
    /// Whether the connector was requested by the immutable envelope.
    pub requested: bool,
    /// Whether the requested capability has a durable grant.
    pub granted: bool,
}

/// Durable read projection of MCP context owned by one Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskMcpContext {
    /// Owning Task Session.
    pub session_id: TaskSessionId,
    /// Workspace bound to the MCP context.
    pub workspace_id: Option<String>,
    /// Runtime profile whose resolver may materialize connector command/environment snapshots.
    pub runtime_profile_id: Option<String>,
    /// Current active assignment attempt, if the session is running.
    pub active_attempt_id: Option<u64>,
    /// Current session fencing token.
    pub fencing_token: u64,
    /// Connector contexts ordered by connector identifier.
    pub connectors: Vec<TaskMcpConnectorContext>,
}

impl TaskMcpContext {
    /// Projects MCP connector ownership from an optional envelope and explicit capability grants.
    pub fn from_parts(
        session_id: TaskSessionId,
        envelope: Option<&TaskSessionEnvelopeV1>,
        grants: &[TaskCapabilityGrant],
        active_attempt_id: Option<u64>,
        fencing_token: u64,
    ) -> Self {
        let granted = grants
            .iter()
            .map(|grant| grant.capability.as_str())
            .collect::<HashSet<_>>();
        let mut connectors = envelope
            .map(|envelope| {
                envelope
                    .connector_ids
                    .iter()
                    .map(|connector_id| {
                        let capability = format!("external_tools:{connector_id}");
                        TaskMcpConnectorContext {
                            connector_id: connector_id.clone(),
                            requested: true,
                            granted: granted.contains(capability.as_str()),
                            capability,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        connectors.sort_by(|left, right| left.connector_id.cmp(&right.connector_id));
        Self {
            session_id,
            workspace_id: envelope.map(|envelope| envelope.workspace_id.clone()),
            runtime_profile_id: envelope.map(|envelope| envelope.runtime_profile_id.clone()),
            active_attempt_id,
            fencing_token,
            connectors,
        }
    }
}

impl TaskToolState {
    /// Projects current tool state from the append-only event journal.
    ///
    /// Tool calls are identified by attempt, fencing token, and runtime call ID so a retry cannot
    /// overwrite an earlier attempt that reused the same runtime-generated identifier.
    pub fn from_events(session_id: TaskSessionId, events: &[TaskSessionEvent]) -> Self {
        let mut calls = BTreeMap::<(Option<u64>, u64, String), TaskToolCallState>::new();
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

            let identity = (
                event.attempt_id,
                event.fencing_token,
                tool_call_id.to_string(),
            );
            let entry = calls.entry(identity).or_insert_with(|| TaskToolCallState {
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
    /// Agent execution finished and its authoritative projections are being committed.
    Committing,
    /// Mock execution completed successfully.
    Succeeded,
    /// Mock execution returned an error or panicked.
    Failed,
    /// Execution could not continue without operator action.
    Blocked,
    /// Session was cancelled before dispatch or during execution.
    Cancelled,
}

/// Structured outcome produced by an Agent runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentTaskResult {
    pub summary: String,
    pub evidence: Vec<String>,
    pub details: Vec<String>,
    pub next: Vec<String>,
    pub completion_status: AgentTaskCompletionStatus,
    pub blocked_reason: Option<String>,
    #[serde(default)]
    pub objective_results: Vec<AgentTaskObjectiveResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentTaskObjectiveResult {
    pub objective_id: String,
    pub completion_status: AgentTaskCompletionStatus,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

/// Authoritative assistant response produced by a Chat Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatTaskResult {
    pub conversation_id: String,
    pub message: String,
}

/// Authoritative replacement proposal produced by an Edit Task Session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EditTaskResult {
    pub file_path: String,
    pub summary: String,
    pub content: String,
}

/// Authoritative terminal disposition reported by an Agent runtime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTaskCompletionStatus {
    Completed,
    Blocked,
}

/// Durable structured output returned by a task executor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "result", rename_all = "snake_case")]
pub enum TaskExecutionOutput {
    None,
    Agent(AgentTaskResult),
    Chat(ChatTaskResult),
    Edit(EditTaskResult),
}

/// Query projection for a staged or finalized authoritative task result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSessionResult {
    pub session_id: TaskSessionId,
    pub output: TaskExecutionOutput,
    pub terminal_state: TaskSessionState,
    pub projection_error: Option<String>,
    pub projected_at: Option<u64>,
    pub finalized_at: Option<u64>,
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
    /// Current assignment attempt, or the terminal assignment after completion.
    pub attempt_id: Option<u64>,
    /// Monotonic token that fences stale Worker completions.
    pub fencing_token: u64,
    /// Durable OpenCode conversation identity owned by this Task Session.
    pub opencode_session_id: Option<String>,
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
    fn prompt_envelope_v2_round_trips_immutable_chat_input() {
        let prompt_input = TaskSessionInputV2::Chat(TaskChatInputV2 {
            message_id: "message-1".to_string(),
            message_sequence: 1,
            message: "hello".to_string(),
            terminal_context: Some("workspace".to_string()),
            session_context: Some("prior turns".to_string()),
        });
        let envelope = TaskSessionEnvelope::V2(TaskSessionEnvelopeV2 {
            session: TaskSessionEnvelopeV1 {
                workspace_id: "workspace-personal".to_string(),
                kind: TaskSessionKind::Chat,
                subject_id: None,
                conversation_id: Some("conversation-1".to_string()),
                execution_run_id: None,
                context_digest: "sha256:prompt".to_string(),
                runtime_profile_id: "profile-1".to_string(),
                model: "openai/gpt-5".to_string(),
                connector_ids: Vec::new(),
                requested_capabilities: Vec::new(),
                prompt_template_version: "prompt-v2".to_string(),
                context_revision: Some("1".to_string()),
                rules_revision: Some("rules-v1".to_string()),
                skills_revision: Some("skills-v1".to_string()),
            },
            prompt_input,
        });
        let request = TaskRequest::from_envelope("chat", &envelope).expect("serialized");
        let encoded = serde_json::to_value(&envelope).expect("encoded");
        assert_eq!(encoded["schema_version"], serde_json::json!(2));
        assert_eq!(request.envelope().expect("decoded"), Some(envelope));
    }

    #[test]
    fn prompt_envelope_v2_rejects_oversized_input_before_enqueue() {
        let envelope = TaskSessionEnvelope::V2(TaskSessionEnvelopeV2 {
            session: TaskSessionEnvelopeV1 {
                workspace_id: "workspace-personal".to_string(),
                kind: TaskSessionKind::Edit,
                subject_id: None,
                conversation_id: None,
                execution_run_id: None,
                context_digest: "sha256:prompt".to_string(),
                runtime_profile_id: "profile-1".to_string(),
                model: "openai/gpt-5".to_string(),
                connector_ids: Vec::new(),
                requested_capabilities: Vec::new(),
                prompt_template_version: "prompt-v2".to_string(),
                context_revision: Some("1".to_string()),
                rules_revision: Some("rules-v1".to_string()),
                skills_revision: Some("skills-v1".to_string()),
            },
            prompt_input: TaskSessionInputV2::Edit(TaskEditInputV2 {
                file_path: "src/main.rs".to_string(),
                instruction: "update".to_string(),
                content: "x".repeat(MAX_TASK_EDIT_CONTENT_BYTES + 1),
                selection: None,
                context_files: Vec::new(),
                diagnostics: Vec::new(),
            }),
        });
        assert!(TaskRequest::from_envelope("oversized", &envelope).is_err());
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

    #[test]
    fn tool_state_keeps_reused_call_ids_isolated_between_attempts() {
        let session = TaskSessionId(7);
        let events = vec![
            TaskSessionEvent {
                id: 1,
                session_id: session,
                attempt_id: Some(11),
                fencing_token: 1,
                sequence: 1,
                kind: TaskSessionEventKind::Tool,
                payload: serde_json::json!({
                    "type": "tool_completed",
                    "tool_call_id": "tool-1",
                    "tool_name": "jira_search",
                    "success": true
                }),
                progress: None,
                created_at: 100,
            },
            TaskSessionEvent {
                id: 2,
                session_id: session,
                attempt_id: Some(12),
                fencing_token: 2,
                sequence: 2,
                kind: TaskSessionEventKind::Tool,
                payload: serde_json::json!({
                    "type": "tool_started",
                    "tool_call_id": "tool-1",
                    "tool_name": "jira_search"
                }),
                progress: None,
                created_at: 110,
            },
        ];

        let state = TaskToolState::from_events(session, &events);
        assert_eq!(state.calls.len(), 2);
        assert_eq!(state.calls[0].attempt_id, Some(11));
        assert_eq!(state.calls[0].status, TaskToolStatus::Succeeded);
        assert_eq!(state.calls[1].attempt_id, Some(12));
        assert_eq!(state.calls[1].status, TaskToolStatus::Running);
    }

    #[test]
    fn mcp_context_projects_connectors_and_grants() {
        let envelope = TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "digest".to_string(),
            runtime_profile_id: "profile-1".to_string(),
            model: "openai/gpt-5".to_string(),
            connector_ids: vec!["jira".to_string(), "github".to_string()],
            requested_capabilities: vec![
                "external_tools:jira".to_string(),
                "external_tools:github".to_string(),
            ],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: Some("context-1".to_string()),
            rules_revision: Some("rules-1".to_string()),
            skills_revision: Some("skills-1".to_string()),
        };
        let context = TaskMcpContext::from_parts(
            TaskSessionId(9),
            Some(&envelope),
            &[TaskCapabilityGrant {
                capability: "external_tools:jira".to_string(),
                grant_source: "test".to_string(),
                granted_at: 1,
            }],
            Some(77),
            2,
        );

        assert_eq!(context.workspace_id.as_deref(), Some("workspace-personal"));
        assert_eq!(context.runtime_profile_id.as_deref(), Some("profile-1"));
        assert_eq!(context.active_attempt_id, Some(77));
        assert_eq!(context.fencing_token, 2);
        assert_eq!(context.connectors.len(), 2);
        assert_eq!(context.connectors[0].connector_id, "github");
        assert!(!context.connectors[0].granted);
        assert_eq!(context.connectors[1].connector_id, "jira");
        assert!(context.connectors[1].granted);
    }
}
