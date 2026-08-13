//! SQLite persistence for the concurrent execution Scheduler.
//!
//! The store owns queue ordering, assignment attempts, leases, and fencing tokens. Every lifecycle
//! mutation is transactional so the Scheduler can keep only process-local Worker handles and
//! cancellation tokens in memory.

#[cfg(test)]
use crate::domain::execution_manifest::ExecutionModelConfiguration;
use crate::domain::execution_manifest::{
    ExecutionManifest, ExecutionManifestDraft, EXECUTION_MANIFEST_SCHEMA_VERSION,
};
use crate::domain::governance::GovernanceResolutionRecord;
use crate::domain::resource_idempotency::{ResourceMutationEvidence, ResourceOperationIdentity};
use crate::domain::subtask_authority::{
    DormantSubtaskFence, PreparedSubtaskContract, SchedulerPreparedSubtask,
};
#[cfg(test)]
use crate::domain::task_session::TaskMcpConnectorContext;
use crate::domain::task_session::{
    AgentTaskCompletionStatus, AgentTaskObjectiveCheckpoint, AgentTaskObjectiveToolReceipt,
    TaskCapabilityGrant, TaskExecutionOutput, TaskExecutionTraceEntry, TaskExecutionTracePage,
    TaskMcpContext, TaskProgress, TaskRequest, TaskSessionEnvelope, TaskSessionEvent,
    TaskSessionEventInput, TaskSessionEventKind, TaskSessionEventPage, TaskSessionId,
    TaskSessionInputV2, TaskSessionKind, TaskSessionResult, TaskSessionSnapshot, TaskSessionState,
    TaskToolState,
};
use rusqlite::{
    params, Connection, ErrorCode, OpenFlags, OptionalExtension, Row, Transaction,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SUBTASK_AUTHORITY_MAX_LEASE: Duration = Duration::from_secs(30);
pub(crate) const RECOVERY_REQUIRES_RETRY_FRESH: &str =
    "[recovery_requires_retry_fresh] Spacesly cannot safely resume this Agent task because no durable OpenCode session identity was recorded. Use Retry Fresh explicitly.";
pub(crate) const RECOVERY_REQUIRES_MUTATION_RECONCILIATION: &str =
    "[recovery_requires_mutation_reconciliation] Spacesly stopped recovery because an external mutation has an uncertain outcome. Reconcile the retained mutation fence before continuing.";

fn stable_manifest_identity_matches(
    first: &ExecutionManifestDraft,
    current: &ExecutionManifestDraft,
) -> bool {
    first.kind == current.kind
        && first.workspace_id == current.workspace_id
        && first.subject_id == current.subject_id
        && first.conversation_id == current.conversation_id
        && first.execution_run_id == current.execution_run_id
        && first.runtime == current.runtime
        && first.runtime_profile_id == current.runtime_profile_id
        && first.model == current.model
        && first.prompt_template_version == current.prompt_template_version
        && first.rules_digest == current.rules_digest
        && first.skills_catalog_revision == current.skills_catalog_revision
}

/// SQLite-backed authority for Scheduler queue and Task Session lifecycle state.
#[derive(Clone)]
pub struct SchedulerStore {
    connection: Arc<Mutex<Connection>>,
    instance_id: Arc<str>,
    database_path: Option<Arc<PathBuf>>,
    #[cfg(test)]
    resolution_failures: Arc<AtomicUsize>,
}

/// Non-secret identity checked by a subprocess immediately before forwarding an external call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalAssignmentAuthority {
    pub scheduler_database: PathBuf,
    pub scheduler_instance_id: String,
    pub session_id: TaskSessionId,
    pub attempt_id: u64,
    pub attempt: u32,
    pub owner_id: u64,
    pub fencing_token: u64,
    pub capability: String,
    pub connector_id: String,
    pub connector_binding_digest: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtask_authority: Option<SubtaskToolAuthority>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceMutationState {
    Reserved,
    Succeeded,
    Failed,
    Uncertain,
    Superseded,
}

impl ResourceMutationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Superseded => "superseded",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "uncertain" => Ok(Self::Uncertain),
            "superseded" => Ok(Self::Superseded),
            _ => Err(format!("Unknown resource mutation state '{value}'.")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceMutationRecord {
    pub mutation_id: u64,
    pub operation_key: String,
    pub identity: ResourceOperationIdentity,
    pub connector_id: String,
    pub tool_name: String,
    pub state: ResourceMutationState,
    pub session_id: TaskSessionId,
    pub attempt_id: u64,
    pub attempt: u32,
    pub fencing_token: u64,
    pub evidence: Option<ResourceMutationEvidence>,
    pub failure_kind: Option<String>,
    pub failure_code: Option<String>,
    pub revision: u64,
    pub reserved_at: u64,
    pub resolved_at: Option<u64>,
    pub superseded_at: Option<u64>,
    pub supersede_reason: Option<String>,
    pub checkpoint_objective_id: Option<String>,
    pub checkpoint_tool_call_id: Option<String>,
    pub checkpoint_recorded_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceMutationReservation {
    Reserved(ResourceMutationRecord),
    Blocked(ResourceMutationRecord),
}

#[derive(Clone, Debug)]
pub enum ResourceMutationResolution {
    Succeeded(ResourceMutationEvidence),
    Failed {
        evidence: Option<ResourceMutationEvidence>,
        kind: String,
        code: String,
    },
    Uncertain {
        evidence: Option<ResourceMutationEvidence>,
        kind: String,
        code: String,
    },
}

/// Non-secret authority used by the assignment-local workspace tool MCP server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskToolAuthority {
    pub scheduler_database: PathBuf,
    pub scheduler_instance_id: String,
    pub session_id: TaskSessionId,
    pub attempt_id: u64,
    pub attempt: u32,
    pub owner_id: u64,
    pub fencing_token: u64,
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_repository_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_branch: Option<String>,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtask_authority: Option<SubtaskToolAuthority>,
}

/// Scheduler-minted authority for one isolated subtask.
///
/// The current application has no constructor for `SubtaskDispatchPermit`, so this descriptor
/// cannot reach a tool process until the scheduler dispatch path is implemented deliberately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtaskToolAuthority {
    pub scheduler_database: PathBuf,
    pub scheduler_instance_id: String,
    pub session_id: TaskSessionId,
    pub parent_attempt_id: u64,
    pub parent_attempt: u32,
    pub parent_owner_id: u64,
    pub parent_fencing_token: u64,
    pub subtask_id: u64,
    pub subtask_attempt_id: u64,
    pub subtask_attempt: u32,
    pub subtask_fencing_token: u64,
    pub authority_id: u64,
    pub authority_fencing_token: u64,
    pub objective_id: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub allowed_connector_tools: BTreeMap<String, Vec<String>>,
    pub lease_expires_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubtaskToolRisk {
    Read,
    Mutation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskToolAdmission {
    pub authority_id: u64,
    pub tool_calls_used: u32,
    pub mutation_calls_used: u32,
    pub max_tool_calls: u32,
    pub max_mutation_calls: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum SubtaskAuthorityOutcome {
    Completed,
    Cancelled,
    Failed,
}

impl SubtaskAuthorityOutcome {
    fn state(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled | Self::Failed => "revoked",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskAuthorityStatus {
    pub authority_id: u64,
    pub state: String,
    pub terminal_reason: Option<String>,
    pub lease_expires_at: Option<u64>,
    pub tool_calls_used: u32,
    pub mutation_calls_used: u32,
    pub completed_at: Option<u64>,
}

/// Unforgeable module-private capability required to activate dormant subtask authority.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SubtaskDispatchPermit(());

/// Identity required for a Worker to renew or finish one assignment attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AssignmentFence {
    pub(crate) session_id: TaskSessionId,
    pub(crate) attempt_id: u64,
    pub(crate) attempt: u32,
    pub(crate) owner_id: u64,
    pub(crate) fencing_token: u64,
}

/// Durable assignment returned after the FIFO head is atomically claimed.
#[derive(Clone, Debug)]
pub(crate) struct DurableAssignment {
    pub(crate) fence: AssignmentFence,
    pub(crate) request: TaskRequest,
    pub(crate) grants: Vec<TaskCapabilityGrant>,
}

/// Result of one scheduler claim transaction, including every session whose
/// durable state changed while expired assignments were reconciled.
pub(crate) struct ClaimOutcome {
    pub(crate) assignment: Option<DurableAssignment>,
    pub(crate) changed_session_ids: Vec<TaskSessionId>,
}

/// Terminal outcome accepted by a fenced assignment completion.
pub(crate) enum DurableOutcome {
    Succeeded(TaskExecutionOutput),
    Failed(String),
    Blocked(String),
    Cancelled,
}

/// Result of requesting cancellation from the durable lifecycle store.
pub(crate) struct CancelResult {
    pub(crate) changed: bool,
    pub(crate) snapshot: TaskSessionSnapshot,
}

/// Result of attempting to finish an assignment with a fencing token.
pub(crate) enum FinishResult {
    Applied,
    Stale,
}

#[derive(Default)]
struct TaskOwnership {
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    subject_id: Option<String>,
    execution_run_id: Option<String>,
}

/// Durable scheduler outbox entry awaiting projection into executions.db.
#[derive(Clone, Debug)]
pub(crate) struct StagedChatHead {
    pub(crate) message_id: String,
    pub(crate) message_sequence: u64,
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
pub(crate) struct StagedCompletion {
    pub(crate) projection_id: String,
    pub(crate) session_id: TaskSessionId,
    pub(crate) attempt_id: u64,
    pub(crate) fencing_token: u64,
    pub(crate) workspace_id: String,
    pub(crate) conversation_id: String,
    pub(crate) execution_run_id: String,
    pub(crate) output: TaskExecutionOutput,
    pub(crate) terminal_state: TaskSessionState,
    pub(crate) chat_head: Option<StagedChatHead>,
}

impl SchedulerStore {
    /// Opens the default persistent Scheduler database in the Spacesly data directory.
    pub fn open() -> Result<Self, String> {
        Self::open_at(database_path()?)
    }

    /// Opens a query-only connection, bootstrapping or migrating the schema only when required.
    pub fn open_query() -> Result<Self, String> {
        Self::open_query_at(database_path()?)
    }

    fn open_query_at(path: PathBuf) -> Result<Self, String> {
        match Self::open_read_only_at(path.clone()) {
            Ok(store) => Ok(store),
            Err(error)
                if !path.exists()
                    || error.contains("no such table")
                    || error.contains("no such column") =>
            {
                drop(Self::open_at(path.clone())?);
                Self::open_read_only_at(path)
            }
            Err(error) => Err(error),
        }
    }

    /// Opens or creates a persistent Scheduler database at an explicit path.
    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create scheduler data directory: {error}"))?;
        }
        let connection = Connection::open(&path)
            .map_err(|error| format!("Failed to open scheduler database: {error}"))?;
        let canonical_path = fs::canonicalize(&path)
            .map_err(|error| format!("Failed to resolve scheduler database path: {error}"))?;
        Self::initialize(connection, Some(canonical_path))
    }

    fn open_read_only_at(path: PathBuf) -> Result<Self, String> {
        let canonical_path = fs::canonicalize(&path)
            .map_err(|error| format!("Failed to resolve Task Session query database: {error}"))?;
        let connection =
            Connection::open_with_flags(&canonical_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| format!("Failed to open Task Session query database: {error}"))?;
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .map_err(|error| format!("Failed to configure Task Session query timeout: {error}"))?;
        connection
            .prepare(SESSION_SELECT_ALL)
            .map_err(|error| format!("Task Session query schema is not ready: {error}"))?;
        connection
            .prepare(
                "SELECT event_id, session_id, attempt_id, fencing_token, sequence,
                        event_kind, payload_json, progress_json, created_at
                   FROM scheduler_task_events LIMIT 1",
            )
            .map_err(|error| format!("Task Session event schema is not ready: {error}"))?;
        connection
            .prepare("SELECT mutation_id FROM scheduler_resource_mutations LIMIT 1")
            .map_err(|error| format!("Resource mutation query schema is not ready: {error}"))?;
        let instance_id = connection
            .query_row(
                "SELECT instance_id FROM scheduler_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to read scheduler instance ID: {error}"))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            instance_id: Arc::from(instance_id),
            database_path: Some(Arc::new(canonical_path)),
            #[cfg(test)]
            resolution_failures: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Opens an isolated in-memory Scheduler database.
    pub fn open_in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("Failed to open in-memory scheduler database: {error}"))?;
        Self::initialize(connection, None)
    }

    fn initialize(
        mut connection: Connection,
        database_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .map_err(|error| format!("Failed to configure scheduler database timeout: {error}"))?;
        execute_batch_with_busy_retry(
            &connection,
            "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA wal_autocheckpoint = 100;
                  CREATE TABLE IF NOT EXISTS scheduler_metadata (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    next_enqueue_sequence INTEGER NOT NULL,
                    next_dispatch_sequence INTEGER NOT NULL,
                    instance_id TEXT
                 );
                 INSERT OR IGNORE INTO scheduler_metadata
                   (singleton, next_enqueue_sequence, next_dispatch_sequence)
                 VALUES (1, 1, 1);
                 CREATE TABLE IF NOT EXISTS scheduler_owners (
                   owner_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   started_at INTEGER NOT NULL,
                   last_seen_at INTEGER NOT NULL
                 );
                  CREATE TABLE IF NOT EXISTS scheduler_task_sessions (
                   session_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   enqueue_sequence INTEGER NOT NULL UNIQUE,
                    label TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    workspace_id TEXT,
                    conversation_id TEXT,
                    subject_id TEXT,
                    execution_run_id TEXT,
                    state TEXT NOT NULL,
                   worker_id INTEGER,
                   dispatch_sequence INTEGER,
                   attempt_count INTEGER NOT NULL DEFAULT 0,
                   active_attempt_id INTEGER,
                   fencing_token INTEGER NOT NULL DEFAULT 0,
                   opencode_session_id TEXT,
                   lease_expires_at INTEGER,
                   progress_phase TEXT,
                   progress_completed INTEGER,
                   progress_total INTEGER,
                   next_event_sequence INTEGER NOT NULL DEFAULT 1,
                   error TEXT,
                   created_at INTEGER NOT NULL,
                   started_at INTEGER,
                   completed_at INTEGER
                 );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_sessions_fifo
                    ON scheduler_task_sessions(state, enqueue_sequence);
                  CREATE TABLE IF NOT EXISTS scheduler_task_grants (
                    session_id INTEGER NOT NULL,
                    capability TEXT NOT NULL,
                    grant_source TEXT NOT NULL,
                    granted_at INTEGER NOT NULL,
                    PRIMARY KEY(session_id, capability),
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE
                  );
                  CREATE TABLE IF NOT EXISTS scheduler_task_governance (
                    session_id INTEGER PRIMARY KEY,
                    resolution_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE
                  );
                  CREATE TABLE IF NOT EXISTS scheduler_task_execution_manifests (
                    attempt_id INTEGER PRIMARY KEY,
                    session_id INTEGER NOT NULL,
                    manifest_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    UNIQUE(session_id, attempt_id),
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE,
                    FOREIGN KEY(attempt_id) REFERENCES scheduler_task_attempts(attempt_id)
                      ON DELETE CASCADE
                  );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_execution_manifests_session
                    ON scheduler_task_execution_manifests(session_id, attempt_id DESC);
                  CREATE TABLE IF NOT EXISTS scheduler_task_attempts (
                   attempt_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   session_id INTEGER NOT NULL,
                   attempt_number INTEGER NOT NULL,
                   fencing_token INTEGER NOT NULL,
                   dispatch_sequence INTEGER NOT NULL UNIQUE,
                   owner_id INTEGER NOT NULL,
                   worker_id INTEGER NOT NULL,
                   state TEXT NOT NULL,
                   lease_expires_at INTEGER,
                   started_at INTEGER NOT NULL,
                   completed_at INTEGER,
                   error TEXT,
                   UNIQUE(session_id, attempt_number),
                   UNIQUE(session_id, fencing_token),
                   FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                     ON DELETE CASCADE
                 );
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduler_one_running_attempt
                   ON scheduler_task_attempts(session_id) WHERE state = 'running';
                 CREATE INDEX IF NOT EXISTS idx_scheduler_attempt_expiry
                   ON scheduler_task_attempts(state, lease_expires_at);
                 CREATE INDEX IF NOT EXISTS idx_scheduler_attempt_owner
                   ON scheduler_task_attempts(owner_id, state);
                  CREATE TABLE IF NOT EXISTS scheduler_prepared_subtasks (
                    subtask_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id INTEGER NOT NULL,
                    contract_id TEXT NOT NULL,
                    objective_id TEXT NOT NULL,
                    contract_json TEXT NOT NULL,
                    state TEXT NOT NULL CHECK (state = 'prepared'),
                    execution_enabled INTEGER NOT NULL DEFAULT 0 CHECK (execution_enabled = 0),
                    created_from_attempt_id INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    UNIQUE(session_id, contract_id),
                    UNIQUE(session_id, objective_id),
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE,
                    FOREIGN KEY(created_from_attempt_id) REFERENCES scheduler_task_attempts(attempt_id)
                      ON DELETE CASCADE
                  );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_prepared_subtasks_session
                    ON scheduler_prepared_subtasks(session_id, subtask_id);
                  CREATE TABLE IF NOT EXISTS scheduler_subtask_attempts (
                    subtask_attempt_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    subtask_id INTEGER NOT NULL,
                    attempt_number INTEGER NOT NULL,
                    fencing_token INTEGER NOT NULL,
                    state TEXT NOT NULL CHECK (state = 'dormant'),
                    wall_clock_seconds INTEGER NOT NULL,
                    max_tool_calls INTEGER NOT NULL,
                    max_mutation_calls INTEGER NOT NULL,
                    tool_calls_used INTEGER NOT NULL DEFAULT 0,
                    mutation_calls_used INTEGER NOT NULL DEFAULT 0,
                    authority_active INTEGER NOT NULL DEFAULT 0 CHECK (authority_active = 0),
                    created_at INTEGER NOT NULL,
                    UNIQUE(subtask_id, attempt_number),
                    UNIQUE(subtask_id, fencing_token),
                    FOREIGN KEY(subtask_id) REFERENCES scheduler_prepared_subtasks(subtask_id)
                      ON DELETE CASCADE
                  );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_subtask_attempts_subtask
                    ON scheduler_subtask_attempts(subtask_id, attempt_number);
                  CREATE TABLE IF NOT EXISTS scheduler_subtask_authorities (
                    authority_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    subtask_attempt_id INTEGER NOT NULL UNIQUE,
                    parent_attempt_id INTEGER NOT NULL,
                    parent_fencing_token INTEGER NOT NULL,
                    authority_fencing_token INTEGER NOT NULL,
                    state TEXT NOT NULL CHECK (state IN ('active', 'revoked', 'completed')),
                    lease_expires_at INTEGER NOT NULL,
                    tool_calls_used INTEGER NOT NULL DEFAULT 0 CHECK (tool_calls_used >= 0),
                    mutation_calls_used INTEGER NOT NULL DEFAULT 0 CHECK (mutation_calls_used >= 0),
                    activated_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    terminal_reason TEXT,
                    FOREIGN KEY(subtask_attempt_id) REFERENCES scheduler_subtask_attempts(subtask_attempt_id)
                      ON DELETE CASCADE,
                    FOREIGN KEY(parent_attempt_id) REFERENCES scheduler_task_attempts(attempt_id)
                      ON DELETE CASCADE
                  );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_subtask_authorities_active
                    ON scheduler_subtask_authorities(state, lease_expires_at);
                  CREATE TABLE IF NOT EXISTS scheduler_task_events (
                   event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   session_id INTEGER NOT NULL,
                   attempt_id INTEGER,
                   fencing_token INTEGER NOT NULL DEFAULT 0,
                    sequence INTEGER NOT NULL,
                    event_kind TEXT NOT NULL,
                    event_type TEXT,
                    payload_json TEXT NOT NULL,
                    progress_json TEXT,
                    created_at INTEGER NOT NULL,
                   UNIQUE(session_id, sequence),
                   FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                     ON DELETE CASCADE
                 );
                   CREATE INDEX IF NOT EXISTS idx_scheduler_events_cursor
                     ON scheduler_task_events(session_id, sequence);
                   CREATE TABLE IF NOT EXISTS scheduler_task_completions (
                    session_id INTEGER PRIMARY KEY,
                    projection_id TEXT NOT NULL UNIQUE,
                    attempt_id INTEGER NOT NULL,
                    fencing_token INTEGER NOT NULL,
                    workspace_id TEXT NOT NULL,
                    conversation_id TEXT NOT NULL,
                    execution_run_id TEXT NOT NULL,
                    terminal_state TEXT NOT NULL,
                    output_json TEXT NOT NULL,
                     projection_error TEXT,
                     projection_attempt_count INTEGER NOT NULL DEFAULT 0,
                     next_projection_at INTEGER NOT NULL DEFAULT 0,
                    staged_at INTEGER NOT NULL,
                    projected_at INTEGER,
                    finalized_at INTEGER,
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE
                  );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_completion_pending
                    ON scheduler_task_completions(projected_at, finalized_at, staged_at);
                  CREATE TABLE IF NOT EXISTS scheduler_task_objective_checkpoints (
                    session_id INTEGER NOT NULL,
                    objective_id TEXT NOT NULL,
                    evidence_json TEXT NOT NULL,
                    tool_receipts_json TEXT NOT NULL DEFAULT '[]',
                    source_attempt_id INTEGER NOT NULL,
                    source_fencing_token INTEGER NOT NULL,
                    recorded_at INTEGER NOT NULL,
                    PRIMARY KEY(session_id, objective_id),
                    FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                      ON DELETE CASCADE,
                    FOREIGN KEY(source_attempt_id) REFERENCES scheduler_task_attempts(attempt_id)
                      ON DELETE CASCADE
                  );
                   CREATE INDEX IF NOT EXISTS idx_scheduler_objective_checkpoints_session
                     ON scheduler_task_objective_checkpoints(session_id, recorded_at);
                   CREATE TABLE IF NOT EXISTS scheduler_resource_mutations (
                     mutation_id INTEGER PRIMARY KEY AUTOINCREMENT,
                     operation_key TEXT NOT NULL,
                     identity_json TEXT NOT NULL,
                     connector_id TEXT NOT NULL,
                     tool_name TEXT NOT NULL,
                     state TEXT NOT NULL CHECK (
                       state IN ('reserved', 'succeeded', 'failed', 'uncertain', 'superseded')
                     ),
                     session_id INTEGER NOT NULL,
                     attempt_id INTEGER NOT NULL,
                     attempt_number INTEGER NOT NULL,
                     fencing_token INTEGER NOT NULL,
                     evidence_json TEXT,
                     failure_kind TEXT,
                     failure_code TEXT,
                     revision INTEGER NOT NULL DEFAULT 1,
                     reserved_at INTEGER NOT NULL,
                      resolved_at INTEGER,
                      superseded_at INTEGER,
                      supersede_reason TEXT,
                      checkpoint_objective_id TEXT,
                      checkpoint_tool_call_id TEXT,
                      checkpoint_recorded_at INTEGER,
                     FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                       ON DELETE CASCADE,
                     FOREIGN KEY(attempt_id) REFERENCES scheduler_task_attempts(attempt_id)
                       ON DELETE CASCADE
                   );
                   CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduler_resource_mutation_active_key
                     ON scheduler_resource_mutations(operation_key)
                     WHERE state IN ('reserved', 'succeeded', 'uncertain');
                   CREATE INDEX IF NOT EXISTS idx_scheduler_resource_mutations_session
                     ON scheduler_resource_mutations(session_id, mutation_id);",
        )
        .map_err(|error| format!("Failed to initialize scheduler database: {error}"))?;
        // Serialize non-destructive migrations across concurrently starting Scheduler processes.
        let migration = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler migration: {error}"))?;
        ensure_column(
            &migration,
            "scheduler_metadata",
            "instance_id",
            "instance_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_metadata",
            "event_type_backfill_version",
            "event_type_backfill_version INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "opencode_session_id",
            "opencode_session_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "workspace_id",
            "workspace_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "conversation_id",
            "conversation_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "subject_id",
            "subject_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "execution_run_id",
            "execution_run_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "progress_phase",
            "progress_phase TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "progress_completed",
            "progress_completed INTEGER",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "progress_total",
            "progress_total INTEGER",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_sessions",
            "next_event_sequence",
            "next_event_sequence INTEGER NOT NULL DEFAULT 1",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_events",
            "progress_json",
            "progress_json TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_events",
            "event_type",
            "event_type TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_completions",
            "projection_attempt_count",
            "projection_attempt_count INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_completions",
            "next_projection_at",
            "next_projection_at INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &migration,
            "scheduler_task_objective_checkpoints",
            "tool_receipts_json",
            "tool_receipts_json TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column(
            &migration,
            "scheduler_resource_mutations",
            "checkpoint_objective_id",
            "checkpoint_objective_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_resource_mutations",
            "checkpoint_tool_call_id",
            "checkpoint_tool_call_id TEXT",
        )?;
        ensure_column(
            &migration,
            "scheduler_resource_mutations",
            "checkpoint_recorded_at",
            "checkpoint_recorded_at INTEGER",
        )?;
        ensure_column(
            &migration,
            "scheduler_subtask_authorities",
            "completed_at",
            "completed_at INTEGER",
        )?;
        ensure_column(
            &migration,
            "scheduler_subtask_authorities",
            "terminal_reason",
            "terminal_reason TEXT",
        )?;
        migration
            .execute_batch(
                "DROP INDEX IF EXISTS idx_scheduler_active_conversation;
                 DROP INDEX IF EXISTS idx_scheduler_active_subject;
                 CREATE UNIQUE INDEX idx_scheduler_active_conversation
                   ON scheduler_task_sessions(workspace_id, conversation_id)
                  WHERE conversation_id IS NOT NULL AND state IN ('running', 'cancelling', 'committing');
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduler_active_subject
                   ON scheduler_task_sessions(workspace_id, subject_id)
                  WHERE subject_id IS NOT NULL AND state IN ('running', 'cancelling', 'committing');
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduler_active_execution_run
                   ON scheduler_task_sessions(workspace_id, execution_run_id)
                  WHERE execution_run_id IS NOT NULL AND state IN ('running', 'cancelling', 'committing');
                 DROP INDEX IF EXISTS idx_scheduler_events_trace_v3;
                 DROP INDEX IF EXISTS idx_scheduler_events_trace_v4;
                 DROP INDEX IF EXISTS idx_scheduler_events_trace_v5;
                 CREATE INDEX IF NOT EXISTS idx_scheduler_events_trace_v6
                   ON scheduler_task_events(session_id, sequence)
                  WHERE event_type IN (
                    'lifecycle', 'tool_started', 'tool_completed',
                    'execution_trace_stage', 'usage_updated', 'opencode_session',
                    'approval_requested', 'runtime_recovery_decision',
                    'objective_checkpointed',
                    'capability_repair_decision', 'connector_session_recovered',
                    'subtask_contracts_prepared'
                  );
                 CREATE INDEX IF NOT EXISTS idx_scheduler_events_tool_state
                   ON scheduler_task_events(session_id, sequence)
                  WHERE event_type IN ('tool_started', 'tool_completed');
                 DROP TRIGGER IF EXISTS scheduler_task_events_classify_type;
                 CREATE TRIGGER scheduler_task_events_classify_type
                 AFTER INSERT ON scheduler_task_events
                 WHEN NEW.event_type IS NULL
                 BEGIN
                   UPDATE scheduler_task_events
                      SET event_type = CASE
                        WHEN NEW.event_kind = 'lifecycle' THEN 'lifecycle'
                        WHEN NEW.event_kind = 'tool' AND json_valid(NEW.payload_json)
                          AND json_extract(NEW.payload_json, '$.type') IN
                              ('tool_started', 'tool_completed')
                          THEN json_extract(NEW.payload_json, '$.type')
                        WHEN NEW.event_kind = 'runtime' AND json_valid(NEW.payload_json)
                          AND json_extract(NEW.payload_json, '$.type') IN (
                            'execution_trace_stage', 'usage_updated', 'opencode_session',
                            'approval_requested', 'runtime_recovery_decision',
                            'objective_checkpointed',
                            'capability_repair_decision', 'connector_session_recovered',
                            'subtask_contracts_prepared'
                          ) THEN json_extract(NEW.payload_json, '$.type')
                        ELSE NULL
                      END
                    WHERE event_id = NEW.event_id;
                 END;",
            )
            .map_err(|error| format!("Failed to create scheduler ownership indexes: {error}"))?;
        let event_type_backfill_version: u32 = migration
            .query_row(
                "SELECT event_type_backfill_version FROM scheduler_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to inspect trace event migration: {error}"))?;
        if event_type_backfill_version < 6 {
            migration
                .execute_batch(
                    "UPDATE scheduler_task_events
                        SET event_type = CASE
                          WHEN event_kind = 'lifecycle' THEN 'lifecycle'
                          WHEN event_kind = 'tool' AND json_valid(payload_json)
                            AND json_extract(payload_json, '$.type') = 'tool_started'
                            THEN 'tool_started'
                          WHEN event_kind = 'tool' AND json_valid(payload_json)
                            AND json_extract(payload_json, '$.type') = 'tool_completed'
                            THEN 'tool_completed'
                          WHEN event_kind = 'runtime' AND json_valid(payload_json)
                            AND json_extract(payload_json, '$.type') IN (
                              'execution_trace_stage', 'usage_updated', 'opencode_session',
                              'approval_requested', 'runtime_recovery_decision',
                              'objective_checkpointed',
                              'capability_repair_decision', 'connector_session_recovered',
                              'subtask_contracts_prepared'
                            ) THEN json_extract(payload_json, '$.type')
                          ELSE NULL
                        END
                      WHERE event_type IS NULL AND (
                        event_kind IN ('lifecycle', 'tool')
                        OR (event_kind = 'runtime' AND json_valid(payload_json)
                          AND json_extract(payload_json, '$.type') IN (
                            'execution_trace_stage', 'usage_updated', 'opencode_session',
                            'approval_requested', 'runtime_recovery_decision',
                            'objective_checkpointed',
                            'capability_repair_decision', 'connector_session_recovered',
                            'subtask_contracts_prepared'
                          ))
                      );
                     UPDATE scheduler_metadata SET event_type_backfill_version = 6
                      WHERE singleton = 1;",
                )
                .map_err(|error| format!("Failed to backfill trace event types: {error}"))?;
        }
        migration
            .execute(
                "UPDATE scheduler_metadata
                    SET instance_id = lower(hex(randomblob(16)))
                  WHERE singleton = 1 AND instance_id IS NULL",
                [],
            )
            .map_err(|error| format!("Failed to initialize scheduler instance ID: {error}"))?;
        migration
            .commit()
            .map_err(|error| format!("Failed to commit scheduler migration: {error}"))?;
        let instance_id = connection
            .query_row(
                "SELECT instance_id FROM scheduler_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to read scheduler instance ID: {error}"))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            instance_id: Arc::from(instance_id),
            database_path: database_path.map(Arc::new),
            #[cfg(test)]
            resolution_failures: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn external_authority(
        &self,
        fence: AssignmentFence,
        capability: &str,
        connector_id: &str,
        connector_binding_digest: &str,
    ) -> Result<ExternalAssignmentAuthority, String> {
        if capability.trim().is_empty() || capability != capability.trim() {
            return Err("External assignment capability must be canonical.".to_string());
        }
        let connector_id = connector_id.trim();
        if connector_id.is_empty()
            || capability != format!("external_tools:{connector_id}")
            || connector_binding_digest.len() != 64
            || !connector_binding_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("External assignment connector binding is invalid.".to_string());
        }
        let database_path = self.database_path.as_ref().ok_or_else(|| {
            "External assignment authority requires a persistent store.".to_string()
        })?;
        Ok(ExternalAssignmentAuthority {
            scheduler_database: database_path.as_ref().clone(),
            scheduler_instance_id: self.instance_id.to_string(),
            session_id: fence.session_id,
            attempt_id: fence.attempt_id,
            attempt: fence.attempt,
            owner_id: fence.owner_id,
            fencing_token: fence.fencing_token,
            capability: capability.to_string(),
            connector_id: connector_id.to_string(),
            connector_binding_digest: connector_binding_digest.to_ascii_lowercase(),
            allowed_tools: Vec::new(),
            subtask_authority: None,
        })
    }

    pub(crate) fn task_tool_authority(
        &self,
        fence: AssignmentFence,
        workspace_id: &str,
        workspace_root: PathBuf,
        capabilities: &[String],
    ) -> Result<TaskToolAuthority, String> {
        let database_path = self.database_path.as_ref().ok_or_else(|| {
            "Task tool authority requires a persistent scheduler store.".to_string()
        })?;
        if workspace_id.trim().is_empty() || workspace_id != workspace_id.trim() {
            return Err("Task tool workspace ID must be canonical.".to_string());
        }
        let workspace_root = workspace_root
            .canonicalize()
            .map_err(|error| format!("Failed to resolve task tool workspace root: {error}"))?;
        if !workspace_root.is_dir() {
            return Err("Task tool workspace root is not a directory.".to_string());
        }
        let mut capabilities = capabilities
            .iter()
            .filter(|capability| {
                matches!(
                    capability.as_str(),
                    "workspace_read" | "workspace_write" | "shell" | "git"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        capabilities.sort();
        capabilities.dedup();
        Ok(TaskToolAuthority {
            scheduler_database: database_path.as_ref().clone(),
            scheduler_instance_id: self.instance_id.to_string(),
            session_id: fence.session_id,
            attempt_id: fence.attempt_id,
            attempt: fence.attempt,
            owner_id: fence.owner_id,
            fencing_token: fence.fencing_token,
            workspace_id: workspace_id.to_string(),
            workspace_root,
            default_repository_root: None,
            bound_branch: None,
            capabilities,
            subtask_authority: None,
        })
    }

    pub(crate) fn task_tool_authority_is_current(
        authority: &TaskToolAuthority,
        capability: &str,
    ) -> Result<bool, String> {
        if !authority
            .capabilities
            .iter()
            .any(|granted| granted == capability)
        {
            return Ok(false);
        }
        let store = Self::open_read_only_at(authority.scheduler_database.clone())?;
        if store.instance_id() != authority.scheduler_instance_id {
            return Ok(false);
        }
        let connection = store.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT 1
                   FROM scheduler_task_sessions sessions
                   JOIN scheduler_task_attempts attempts
                     ON attempts.attempt_id = sessions.active_attempt_id
                   JOIN scheduler_task_grants grants
                     ON grants.session_id = sessions.session_id
                  WHERE sessions.session_id = ?1
                    AND sessions.workspace_id = ?2
                    AND sessions.state = 'running'
                    AND sessions.active_attempt_id = ?3
                    AND sessions.fencing_token = ?4
                    AND sessions.lease_expires_at > ?5
                    AND attempts.session_id = sessions.session_id
                    AND attempts.attempt_number = ?6
                    AND attempts.owner_id = ?7
                    AND attempts.fencing_token = ?4
                    AND attempts.state = 'running'
                    AND attempts.lease_expires_at > ?5
                    AND grants.capability = ?8",
                params![
                    to_i64(authority.session_id.0)?,
                    authority.workspace_id,
                    to_i64(authority.attempt_id)?,
                    to_i64(authority.fencing_token)?,
                    to_i64(now_millis())?,
                    i64::from(authority.attempt),
                    to_i64(authority.owner_id)?,
                    capability,
                ],
                |_| Ok(()),
            )
            .optional()
            .map(|current| current.is_some())
            .map_err(|error| format!("Failed to validate task tool authority: {error}"))
    }

    pub(crate) fn external_authority_is_current(
        authority: &ExternalAssignmentAuthority,
    ) -> Result<bool, String> {
        let store = Self::open_read_only_at(authority.scheduler_database.clone())?;
        if store.instance_id() != authority.scheduler_instance_id {
            return Ok(false);
        }
        let connection = store.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT 1
                   FROM scheduler_task_sessions sessions
                   JOIN scheduler_task_attempts attempts
                     ON attempts.attempt_id = sessions.active_attempt_id
                   JOIN scheduler_task_grants grants
                     ON grants.session_id = sessions.session_id
                  WHERE sessions.session_id = ?1
                    AND sessions.state = 'running'
                    AND sessions.active_attempt_id = ?2
                    AND sessions.fencing_token = ?3
                    AND sessions.lease_expires_at > ?4
                    AND attempts.session_id = sessions.session_id
                    AND attempts.attempt_number = ?5
                    AND attempts.owner_id = ?6
                    AND attempts.fencing_token = ?3
                    AND attempts.state = 'running'
                    AND attempts.lease_expires_at > ?4
                    AND grants.capability = ?7",
                params![
                    to_i64(authority.session_id.0)?,
                    to_i64(authority.attempt_id)?,
                    to_i64(authority.fencing_token)?,
                    to_i64(now_millis())?,
                    i64::from(authority.attempt),
                    to_i64(authority.owner_id)?,
                    authority.capability
                ],
                |_| Ok(()),
            )
            .optional()
            .map(|current| current.is_some())
            .map_err(|error| format!("Failed to validate external assignment authority: {error}"))
    }

    pub fn reserve_external_resource_mutation(
        authority: &ExternalAssignmentAuthority,
        tool_name: &str,
        identity: &ResourceOperationIdentity,
    ) -> Result<ResourceMutationReservation, String> {
        identity.validate()?;
        validate_external_authority_shape(authority)?;
        validate_ledger_token(tool_name, "tool name")?;
        let supported_operation = identity.connector == "openshift_kubernetes"
            && matches!(
                (tool_name, identity.operation.as_str()),
                ("ocp_restart_deployment", "restart_deployment")
                    | ("ocp_scale_deployment", "scale_deployment")
            )
            || (identity.connector == "jira"
                && identity.operation == "add_comment"
                && crate::infrastructure::jira::trusted_jira_comment_tool(tool_name));
        if !supported_operation {
            return Err(
                "Resource mutation ledger does not support this connector operation.".to_string(),
            );
        }
        let store = Self::open_authority_store(authority)?;
        let mut connection = store.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start resource mutation reservation: {error}"))?;
        if !external_authority_is_current_on(&transaction, authority, now_millis())? {
            return Err(
                "Resource mutation assignment authority is stale, expired, or ungranted."
                    .to_string(),
            );
        }
        let mut reconciled_mutation_id = None;
        if let Some(record) = resource_mutation_by_active_key_on(&transaction, &identity.key)? {
            if record.state == ResourceMutationState::Succeeded {
                if identity.connector == "jira" {
                    transaction.commit().map_err(|error| {
                        format!("Failed to commit resource mutation lookup: {error}")
                    })?;
                    return Ok(ResourceMutationReservation::Blocked(record));
                }
                if record.checkpoint_objective_id.is_some() {
                    transaction.commit().map_err(|error| {
                        format!("Failed to commit resource mutation lookup: {error}")
                    })?;
                    return Ok(ResourceMutationReservation::Blocked(record));
                }
                transaction
                    .execute(
                        "UPDATE scheduler_resource_mutations
                            SET state = 'superseded', revision = revision + 1,
                                superseded_at = ?2,
                                supersede_reason = 'automatic_state_reconciliation'
                          WHERE mutation_id = ?1 AND state = 'succeeded'",
                        params![to_i64(record.mutation_id)?, to_i64(now_millis())?],
                    )
                    .map_err(|error| {
                        format!("Failed to begin resource mutation reconciliation: {error}")
                    })?;
                reconciled_mutation_id = Some(record.mutation_id);
            } else {
                transaction.commit().map_err(|error| {
                    format!("Failed to commit resource mutation lookup: {error}")
                })?;
                return Ok(ResourceMutationReservation::Blocked(record));
            }
        }
        let identity_json = serde_json::to_string(identity)
            .map_err(|error| format!("Failed to encode resource operation identity: {error}"))?;
        let now = now_millis();
        transaction
            .execute(
                "INSERT INTO scheduler_resource_mutations
                   (operation_key, identity_json, connector_id, tool_name, state, session_id,
                    attempt_id, attempt_number, fencing_token, revision, reserved_at)
                 VALUES (?1, ?2, ?3, ?4, 'reserved', ?5, ?6, ?7, ?8, 1, ?9)",
                params![
                    identity.key,
                    identity_json,
                    authority.connector_id,
                    tool_name,
                    to_i64(authority.session_id.0)?,
                    to_i64(authority.attempt_id)?,
                    i64::from(authority.attempt),
                    to_i64(authority.fencing_token)?,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to reserve resource mutation: {error}"))?;
        let mutation_id = from_i64(transaction.last_insert_rowid(), "resource mutation ID")?;
        append_event_in_transaction(
            &transaction,
            authority.session_id,
            Some(authority.attempt_id),
            authority.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Runtime,
                payload: json!({
                    "type": "resource_mutation_reserved",
                    "mutation_id": mutation_id,
                    "operation_key": identity.key,
                    "connector_id": authority.connector_id,
                    "tool_name": tool_name,
                    "state": "reserved",
                    "reconciles_mutation_id": reconciled_mutation_id
                }),
                progress: None,
            },
            now,
        )?;
        let record = resource_mutation_on(&transaction, mutation_id)?
            .ok_or_else(|| "Reserved resource mutation could not be reloaded.".to_string())?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit resource mutation reservation: {error}"))?;
        Ok(ResourceMutationReservation::Reserved(record))
    }

    pub fn resolve_external_resource_mutation(
        authority: &ExternalAssignmentAuthority,
        mutation_id: u64,
        resolution: ResourceMutationResolution,
    ) -> Result<ResourceMutationRecord, String> {
        validate_external_authority_shape(authority)?;
        let store = Self::open_authority_store(authority)?;
        let mut connection = store.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start resource mutation resolution: {error}"))?;
        let current = resource_mutation_on(&transaction, mutation_id)?
            .ok_or_else(|| format!("Resource mutation {mutation_id} was not found."))?;
        if current.state != ResourceMutationState::Reserved
            || current.session_id != authority.session_id
            || current.attempt_id != authority.attempt_id
            || current.attempt != authority.attempt
            || current.fencing_token != authority.fencing_token
            || current.connector_id != authority.connector_id
        {
            return Err(
                "Resource mutation resolution did not match its reservation authority.".to_string(),
            );
        }
        let (state, evidence, failure_kind, failure_code) = match resolution {
            ResourceMutationResolution::Succeeded(evidence) => {
                validate_resolution_evidence(&current, &evidence)?;
                (ResourceMutationState::Succeeded, Some(evidence), None, None)
            }
            ResourceMutationResolution::Failed {
                evidence,
                kind,
                code,
            } => {
                validate_failure_identity(&current, evidence.as_ref(), &kind, &code)?;
                (
                    ResourceMutationState::Failed,
                    evidence,
                    Some(kind),
                    Some(code),
                )
            }
            ResourceMutationResolution::Uncertain {
                evidence,
                kind,
                code,
            } => {
                validate_failure_identity(&current, evidence.as_ref(), &kind, &code)?;
                (
                    ResourceMutationState::Uncertain,
                    evidence,
                    Some(kind),
                    Some(code),
                )
            }
        };
        let evidence_json = evidence
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("Failed to encode resource mutation evidence: {error}"))?;
        let now = now_millis();
        let updated = transaction
            .execute(
                "UPDATE scheduler_resource_mutations
                    SET state = ?2, evidence_json = ?3, failure_kind = ?4, failure_code = ?5,
                        revision = revision + 1, resolved_at = ?6
                  WHERE mutation_id = ?1 AND state = 'reserved' AND session_id = ?7
                    AND attempt_id = ?8 AND fencing_token = ?9",
                params![
                    to_i64(mutation_id)?,
                    state.as_str(),
                    evidence_json,
                    failure_kind,
                    failure_code,
                    to_i64(now)?,
                    to_i64(authority.session_id.0)?,
                    to_i64(authority.attempt_id)?,
                    to_i64(authority.fencing_token)?
                ],
            )
            .map_err(|error| format!("Failed to resolve resource mutation: {error}"))?;
        if updated != 1 {
            return Err("Resource mutation resolution lost its reservation fence.".to_string());
        }
        append_event_in_transaction(
            &transaction,
            authority.session_id,
            Some(authority.attempt_id),
            authority.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Runtime,
                payload: json!({
                    "type": "resource_mutation_resolved",
                    "mutation_id": mutation_id,
                    "operation_key": current.operation_key,
                    "state": state.as_str(),
                    "failure_kind": failure_kind,
                    "failure_code": failure_code
                }),
                progress: None,
            },
            now,
        )?;
        let record = resource_mutation_on(&transaction, mutation_id)?
            .ok_or_else(|| "Resolved resource mutation could not be reloaded.".to_string())?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit resource mutation resolution: {error}"))?;
        Ok(record)
    }

    #[allow(dead_code)]
    pub fn resource_mutation(
        &self,
        mutation_id: u64,
    ) -> Result<Option<ResourceMutationRecord>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        resource_mutation_on(&connection, mutation_id)
    }

    pub fn resource_mutations_for_session(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Vec<ResourceMutationRecord>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT mutation_id, operation_key, identity_json, connector_id, tool_name,
                        state, session_id, attempt_id, attempt_number, fencing_token,
                        evidence_json, failure_kind, failure_code, revision, reserved_at,
                        resolved_at, superseded_at, supersede_reason,
                        checkpoint_objective_id, checkpoint_tool_call_id,
                        checkpoint_recorded_at
                   FROM scheduler_resource_mutations
                  WHERE session_id = ?1 ORDER BY mutation_id",
            )
            .map_err(|error| format!("Failed to prepare resource mutation query: {error}"))?;
        let rows = statement
            .query_map(params![to_i64(session_id.0)?], decode_resource_mutation_row)
            .map_err(|error| format!("Failed to query resource mutations: {error}"))?;
        rows.map(|row| {
            let raw =
                row.map_err(|error| format!("Failed to decode resource mutation: {error}"))?;
            decode_resource_mutation(raw)
        })
        .collect()
    }

    pub fn supersede_resource_mutation(
        &self,
        session_id: TaskSessionId,
        mutation_id: u64,
        expected_key: &str,
        expected_revision: u64,
        reason: &str,
    ) -> Result<ResourceMutationRecord, String> {
        validate_operation_key(expected_key)?;
        validate_supersede_reason(reason)?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start resource mutation supersede: {error}"))?;
        let current = resource_mutation_on(&transaction, mutation_id)?
            .ok_or_else(|| format!("Resource mutation {mutation_id} was not found."))?;
        if current.session_id != session_id
            || current.operation_key != expected_key
            || current.revision != expected_revision
            || !matches!(
                current.state,
                ResourceMutationState::Succeeded | ResourceMutationState::Uncertain
            )
        {
            return Err(
                "Resource mutation supersede did not match the retained fence.".to_string(),
            );
        }
        let now = now_millis();
        let updated = transaction
            .execute(
                "UPDATE scheduler_resource_mutations
                    SET state = 'superseded', revision = revision + 1,
                        superseded_at = ?2, supersede_reason = ?3
                  WHERE mutation_id = ?1 AND session_id = ?4 AND operation_key = ?5
                    AND revision = ?6 AND state IN ('succeeded', 'uncertain')",
                params![
                    to_i64(mutation_id)?,
                    to_i64(now)?,
                    reason,
                    to_i64(session_id.0)?,
                    expected_key,
                    to_i64(expected_revision)?
                ],
            )
            .map_err(|error| format!("Failed to supersede resource mutation: {error}"))?;
        if updated != 1 {
            return Err("Resource mutation supersede lost its revision fence.".to_string());
        }
        append_event_in_transaction(
            &transaction,
            session_id,
            Some(current.attempt_id),
            current.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Runtime,
                payload: json!({
                    "type": "resource_mutation_superseded",
                    "mutation_id": mutation_id,
                    "operation_key": expected_key,
                    "previous_state": current.state.as_str(),
                    "reason": reason,
                    "source": "local_operator"
                }),
                progress: None,
            },
            now,
        )?;
        let record = resource_mutation_on(&transaction, mutation_id)?
            .ok_or_else(|| "Superseded resource mutation could not be reloaded.".to_string())?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit resource mutation supersede: {error}"))?;
        Ok(record)
    }

    fn open_authority_store(authority: &ExternalAssignmentAuthority) -> Result<Self, String> {
        let canonical_path = fs::canonicalize(&authority.scheduler_database)
            .map_err(|error| format!("Failed to resolve scheduler authority database: {error}"))?;
        let connection = Connection::open_with_flags(
            &canonical_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("Failed to open scheduler authority database: {error}"))?;
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .map_err(|error| format!("Failed to configure scheduler authority timeout: {error}"))?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| {
                format!("Failed to configure scheduler authority database: {error}")
            })?;
        connection
            .prepare("SELECT mutation_id FROM scheduler_resource_mutations LIMIT 1")
            .map_err(|error| format!("Resource mutation ledger schema is not ready: {error}"))?;
        let instance_id = connection
            .query_row(
                "SELECT instance_id FROM scheduler_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to read scheduler authority instance: {error}"))?;
        if instance_id != authority.scheduler_instance_id {
            return Err(
                "Resource mutation authority belongs to another scheduler instance.".to_string(),
            );
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            instance_id: Arc::from(instance_id),
            database_path: Some(Arc::new(canonical_path)),
            #[cfg(test)]
            resolution_failures: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn open_subtask_authority_store(authority: &SubtaskToolAuthority) -> Result<Self, String> {
        let canonical_path = fs::canonicalize(&authority.scheduler_database)
            .map_err(|error| format!("Failed to resolve subtask authority database: {error}"))?;
        let connection = Connection::open_with_flags(
            &canonical_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("Failed to open subtask authority database: {error}"))?;
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .map_err(|error| format!("Failed to configure subtask authority timeout: {error}"))?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| format!("Failed to configure subtask authority database: {error}"))?;
        connection
            .prepare("SELECT authority_id FROM scheduler_subtask_authorities LIMIT 1")
            .map_err(|error| format!("Subtask authority schema is not ready: {error}"))?;
        let instance_id = connection
            .query_row(
                "SELECT instance_id FROM scheduler_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to read subtask authority instance: {error}"))?;
        if instance_id != authority.scheduler_instance_id {
            return Err("Subtask authority belongs to another scheduler instance.".to_string());
        }
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            instance_id: Arc::from(instance_id),
            database_path: Some(Arc::new(canonical_path)),
            #[cfg(test)]
            resolution_failures: Arc::new(AtomicUsize::new(0)),
        })
    }

    #[cfg(test)]
    fn test_subtask_dispatch_permit() -> SubtaskDispatchPermit {
        SubtaskDispatchPermit(())
    }

    pub(crate) fn register_owner(&self) -> Result<u64, String> {
        let _metric =
            crate::infrastructure::performance::span("scheduler_owner_register", "sqlite_write");
        crate::infrastructure::performance::increment("sqlite_writes_total", "sqlite", 1);
        let now = now_millis();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO scheduler_owners (started_at, last_seen_at) VALUES (?1, ?1)",
                params![to_i64(now)?],
            )
            .map_err(|error| format!("Failed to register scheduler owner: {error}"))?;
        from_i64(connection.last_insert_rowid(), "scheduler owner ID")
    }

    pub(crate) fn enqueue(&self, request: &TaskRequest) -> Result<TaskSessionSnapshot, String> {
        self.enqueue_with_grants_at(request, &[], "", now_millis())
    }

    pub(crate) fn enqueue_with_grants(
        &self,
        request: &TaskRequest,
        capabilities: &[String],
        grant_source: &str,
    ) -> Result<TaskSessionSnapshot, String> {
        self.enqueue_with_grants_at(request, capabilities, grant_source, now_millis())
    }

    #[cfg(test)]
    fn enqueue_at(&self, request: &TaskRequest, now: u64) -> Result<TaskSessionSnapshot, String> {
        self.enqueue_with_grants_at(request, &[], "", now)
    }

    fn enqueue_with_grants_at(
        &self,
        request: &TaskRequest,
        capabilities: &[String],
        grant_source: &str,
        now: u64,
    ) -> Result<TaskSessionSnapshot, String> {
        let _metric = crate::infrastructure::performance::span(
            "task_session_enqueue",
            "sqlite_write_transaction",
        );
        crate::infrastructure::performance::increment("sqlite_writes_total", "sqlite", 1);
        let capabilities = validate_capability_grants(capabilities, grant_source)?;
        let ownership = request_ownership(request);
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler enqueue transaction: {error}"))?;
        let enqueue_sequence = next_sequence(&transaction, "next_enqueue_sequence")?;
        transaction
            .execute(
                "INSERT INTO scheduler_task_sessions
                   (enqueue_sequence, label, payload, workspace_id, conversation_id, subject_id,
                    execution_run_id, state, attempt_count, fencing_token, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', 0, 0, ?8)",
                params![
                    to_i64(enqueue_sequence)?,
                    request.label,
                    request.payload,
                    ownership.workspace_id,
                    ownership.conversation_id,
                    ownership.subject_id,
                    ownership.execution_run_id,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to enqueue task session: {error}"))?;
        let id = TaskSessionId(from_i64(
            transaction.last_insert_rowid(),
            "task session ID",
        )?);
        for capability in capabilities {
            transaction
                .execute(
                    "INSERT INTO scheduler_task_grants
                       (session_id, capability, grant_source, granted_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![to_i64(id.0)?, capability, grant_source, to_i64(now)?],
                )
                .map_err(|error| format!("Failed to persist task capability grant: {error}"))?;
        }
        append_event_in_transaction(
            &transaction,
            id,
            None,
            0,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({ "state": "queued" }),
                progress: Some(TaskProgress {
                    phase: "queued".to_string(),
                    completed: 0,
                    total: None,
                }),
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler enqueue: {error}"))?;
        drop(connection);
        self.get_session(id)?
            .ok_or_else(|| "Enqueued task session was not found.".to_string())
    }

    pub(crate) fn resume_after_approval(
        &self,
        id: TaskSessionId,
        request: &TaskRequest,
        capabilities: &[String],
        grant_source: &str,
    ) -> Result<TaskSessionSnapshot, String> {
        self.resume_session(
            id,
            request,
            capabilities,
            grant_source,
            true,
            "resumed_after_approval",
        )
    }

    pub(crate) fn continue_interrupted_session(
        &self,
        id: TaskSessionId,
        request: &TaskRequest,
        capabilities: &[String],
        grant_source: &str,
    ) -> Result<TaskSessionSnapshot, String> {
        self.resume_session(id, request, capabilities, grant_source, false, "continued")
    }

    fn resume_session(
        &self,
        id: TaskSessionId,
        request: &TaskRequest,
        capabilities: &[String],
        grant_source: &str,
        approval_resume: bool,
        action: &str,
    ) -> Result<TaskSessionSnapshot, String> {
        let _metric = crate::infrastructure::performance::span(
            "task_session_resume",
            "sqlite_write_transaction",
        );
        crate::infrastructure::performance::increment("sqlite_writes_total", "sqlite", 1);
        let now = now_millis();
        let capabilities = validate_capability_grants(capabilities, grant_source)?;
        let ownership = request_ownership(request);
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start Task Session resume: {error}"))?;
        let (state, error, workspace_id, conversation_id, subject_id, opencode_session_id) =
            transaction
                .query_row(
                    "SELECT state, error, workspace_id, conversation_id, subject_id,
                            opencode_session_id
                       FROM scheduler_task_sessions WHERE session_id = ?1",
                    params![to_i64(id.0)?],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| format!("Failed to inspect Task Session resume: {error}"))?
                .ok_or_else(|| format!("Task Session {} was not found.", id.0))?;
        let approval_required = error
            .as_deref()
            .is_some_and(|error| error.contains("[approval_required]"));
        if approval_resume {
            if state != "blocked" || !approval_required {
                return Err(
                    "Only a Task Session blocked on an approval request may be resumed through approval."
                        .to_string(),
                );
            }
        } else if !matches!(state.as_str(), "blocked" | "failed") || approval_required {
            return Err(if approval_required {
                "This Task Session requires structured UI approval before it may continue."
            } else {
                "Only an interrupted blocked or failed Task Session may continue."
            }
            .to_string());
        }
        let opencode_session_id = opencode_session_id.ok_or_else(|| {
            "The interrupted Task Session has no durable OpenCode session identity; use Retry Fresh instead of silently creating a replacement session."
                .to_string()
        })?;
        if workspace_id != ownership.workspace_id
            || conversation_id != ownership.conversation_id
            || subject_id != ownership.subject_id
        {
            return Err(
                "Resumed Task Session ownership does not match the original task.".to_string(),
            );
        }
        let enqueue_sequence = next_sequence(&transaction, "next_enqueue_sequence")?;
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET enqueue_sequence = ?2, label = ?3, payload = ?4,
                        execution_run_id = ?5, state = 'queued', worker_id = NULL,
                        dispatch_sequence = NULL, active_attempt_id = NULL,
                        lease_expires_at = NULL, completed_at = NULL, error = NULL,
                        progress_phase = 'queued', progress_completed = 0,
                        progress_total = NULL
                  WHERE session_id = ?1 AND state = ?6",
                params![
                    to_i64(id.0)?,
                    to_i64(enqueue_sequence)?,
                    request.label,
                    request.payload,
                    ownership.execution_run_id,
                    state,
                ],
            )
            .map_err(|error| format!("Failed to requeue continued Task Session: {error}"))?;
        if updated != 1 {
            return Err(
                "Task Session continuation raced with another lifecycle transition.".to_string(),
            );
        }
        transaction
            .execute(
                "DELETE FROM scheduler_task_grants WHERE session_id = ?1",
                params![to_i64(id.0)?],
            )
            .map_err(|error| format!("Failed to replace resumed Task Session grants: {error}"))?;
        for capability in capabilities {
            transaction
                .execute(
                    "INSERT INTO scheduler_task_grants
                       (session_id, capability, grant_source, granted_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![to_i64(id.0)?, capability, grant_source, to_i64(now)?],
                )
                .map_err(|error| format!("Failed to persist resumed capability grant: {error}"))?;
        }
        transaction
            .execute(
                "DELETE FROM scheduler_task_completions WHERE session_id = ?1",
                params![to_i64(id.0)?],
            )
            .map_err(|error| format!("Failed to clear blocked Task Session result: {error}"))?;
        append_event_in_transaction(
            &transaction,
            id,
            None,
            0,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({
                    "state": "queued",
                    "action": action,
                    "task_session_id": id.0,
                    "opencode_session_id": opencode_session_id,
                }),
                progress: Some(TaskProgress {
                    phase: "queued".to_string(),
                    completed: 0,
                    total: None,
                }),
            },
            now,
        )?;
        let resumed = load_session(&transaction, id)?
            .ok_or_else(|| "Resumed Task Session was not found.".to_string())?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit Task Session continuation: {error}"))?;
        Ok(resumed)
    }

    pub(crate) fn get_session(
        &self,
        id: TaskSessionId,
    ) -> Result<Option<TaskSessionSnapshot>, String> {
        let _metric = crate::infrastructure::performance::span("task_session_get", "sqlite_read");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        let lock_started = Instant::now();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        load_session(&connection, id)
    }

    pub(crate) fn list_sessions(&self) -> Result<Vec<TaskSessionSnapshot>, String> {
        let _metric = crate::infrastructure::performance::span("task_session_list", "sqlite_read");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        let lock_started = Instant::now();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        let mut statement = connection
            .prepare(SESSION_SELECT_ALL)
            .map_err(|error| format!("Failed to prepare scheduler session query: {error}"))?;
        let rows = statement
            .query_map([], stored_session_from_row)
            .map_err(|error| format!("Failed to query scheduler sessions: {error}"))?;
        let sessions = rows
            .map(|row| {
                row.map_err(|error| format!("Failed to decode scheduler session: {error}"))?
                    .into_snapshot()
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::infrastructure::performance::increment(
            "sqlite_rows_read_total",
            "sqlite",
            sessions.len() as u64,
        );
        Ok(sessions)
    }

    pub(crate) fn append_assignment_event(
        &self,
        fence: AssignmentFence,
        input: TaskSessionEventInput,
    ) -> Result<TaskSessionEvent, String> {
        self.append_assignment_event_at(fence, input, now_millis())
    }

    pub(crate) fn record_objective_checkpoint(
        &self,
        fence: AssignmentFence,
        objective_id: &str,
        evidence: &[String],
        tool_receipts: &[AgentTaskObjectiveToolReceipt],
    ) -> Result<TaskSessionEvent, String> {
        if objective_id.trim() != objective_id
            || objective_id.is_empty()
            || objective_id.len() > 128
            || evidence.is_empty()
            || evidence.len() > 12
            || evidence
                .iter()
                .any(|item| item.trim().is_empty() || item.len() > 2_000)
            || tool_receipts.len() > 32
            || tool_receipts.iter().any(|receipt| {
                receipt.tool_call_id.trim().is_empty()
                    || receipt.tool_call_id.len() > 256
                    || receipt.tool_name.trim().is_empty()
                    || receipt.tool_name.len() > 128
                    || receipt.tool_name.contains("..")
                    || receipt.tool_name.starts_with('/')
                    || receipt.tool_name.ends_with('/')
                    || !receipt.tool_name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
                    })
                    || !matches!(
                        receipt.risk.as_str(),
                        "read" | "mutation" | "destructive" | "credential_sensitive" | "unknown"
                    )
                    || receipt.arguments_digest.len() != 64
                    || !receipt
                        .arguments_digest
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    || receipt
                        .resource_operation_key
                        .as_deref()
                        .is_some_and(|key| validate_operation_key(key).is_err())
                    || (matches!(
                        receipt.tool_name.as_str(),
                        "ocp_restart_deployment" | "ocp_scale_deployment"
                    ) && receipt.resource_operation_key.is_none())
                    || (receipt.resource_operation_key.is_some()
                        && !(matches!(
                            receipt.tool_name.as_str(),
                            "ocp_restart_deployment" | "ocp_scale_deployment"
                        ) || crate::infrastructure::jira::trusted_jira_comment_tool(
                            &receipt.tool_name,
                        )))
                    || receipt.external_resource.as_ref().is_some_and(|resource| {
                        match resource.provider.as_str() {
                            "bamboo" => {
                                resource.resource_kind != "build"
                                    || !crate::infrastructure::bamboo::canonical_bamboo_result_key(
                                        &resource.resource_id,
                                    )
                                    || resource.parent_resource_id.is_some()
                                    || resource.state_fingerprint.is_some()
                            }
                            "jira" => {
                                resource.resource_kind != "comment"
                                    || !crate::infrastructure::jira::canonical_jira_comment_id(
                                        &resource.resource_id,
                                    )
                                    || resource.parent_resource_id.as_deref().is_none_or(|value| {
                                        !crate::infrastructure::jira::canonical_jira_issue_key(
                                            value,
                                        )
                                    })
                                    || resource.state_fingerprint.as_deref().is_none_or(|value| {
                                        !crate::infrastructure::jira::canonical_state_fingerprint(
                                            value,
                                        )
                                    })
                            }
                            _ => true,
                        }
                    })
                    || ((crate::infrastructure::bamboo::trusted_bamboo_trigger_tool(
                        &receipt.tool_name,
                    ) || crate::infrastructure::jira::trusted_jira_comment_tool(
                        &receipt.tool_name,
                    )) && receipt.external_resource.is_none())
                    || (receipt.external_resource.is_some()
                        && !receipt.external_resource.as_ref().is_some_and(|resource| {
                            (resource.provider == "bamboo"
                                && crate::infrastructure::bamboo::trusted_bamboo_trigger_tool(
                                    &receipt.tool_name,
                                ))
                                || (resource.provider == "jira"
                                    && crate::infrastructure::jira::trusted_jira_comment_tool(
                                        &receipt.tool_name,
                                    ))
                        }))
                    || (receipt.external_resource.is_some()
                        && receipt.resource_operation_key.is_some()
                        && !receipt.external_resource.as_ref().is_some_and(|resource| {
                            resource.provider == "jira"
                                && crate::infrastructure::jira::trusted_jira_comment_tool(
                                    &receipt.tool_name,
                                )
                        }))
            })
        {
            return Err("Objective checkpoint payload is invalid.".to_string());
        }
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                format!("Failed to start objective checkpoint transaction: {error}")
            })?;
        if !assignment_is_current_on(&transaction, fence, now)? {
            return Err("Objective checkpoint assignment fence is stale.".to_string());
        }
        let evidence_json = serde_json::to_string(evidence)
            .map_err(|error| format!("Failed to encode objective checkpoint: {error}"))?;
        let tool_receipts_json = serde_json::to_string(tool_receipts)
            .map_err(|error| format!("Failed to encode objective tool receipts: {error}"))?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO scheduler_task_objective_checkpoints
                   (session_id, objective_id, evidence_json, tool_receipts_json,
                    source_attempt_id, source_fencing_token, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    to_i64(fence.session_id.0)?,
                    objective_id,
                    evidence_json,
                    tool_receipts_json,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(now)?,
                ],
            )
            .map_err(|error| format!("Failed to persist objective checkpoint: {error}"))?;
        if inserted == 0 {
            let existing = transaction
                .query_row(
                    "SELECT evidence_json, tool_receipts_json, source_attempt_id,
                            source_fencing_token
                       FROM scheduler_task_objective_checkpoints
                      WHERE session_id = ?1 AND objective_id = ?2",
                    params![to_i64(fence.session_id.0)?, objective_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .map_err(|error| {
                    format!("Failed to verify objective checkpoint replay: {error}")
                })?;
            if existing
                != (
                    evidence_json.clone(),
                    tool_receipts_json.clone(),
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?,
                )
            {
                return Err(
                    "Objective checkpoint replay did not match the immutable checkpoint."
                        .to_string(),
                );
            }
        }
        if inserted == 1 {
            for receipt in tool_receipts {
                if let Some(operation_key) = receipt.resource_operation_key.as_deref() {
                    bind_resource_mutation_checkpoint_on(
                        &transaction,
                        fence,
                        objective_id,
                        receipt,
                        operation_key,
                        now,
                    )?;
                }
            }
        }
        let event = append_event_in_transaction(
            &transaction,
            fence.session_id,
            Some(fence.attempt_id),
            fence.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Runtime,
                payload: json!({
                    "type": "objective_checkpointed",
                    "schema_version": 2,
                    "objective_id": objective_id,
                    "evidence": evidence,
                    "tool_receipts": tool_receipts,
                    "tool_receipt_count": tool_receipts.len(),
                    "new_checkpoint": inserted == 1,
                }),
                progress: None,
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit objective checkpoint: {error}"))?;
        Ok(event)
    }

    fn append_assignment_event_at(
        &self,
        fence: AssignmentFence,
        input: TaskSessionEventInput,
        now: u64,
    ) -> Result<TaskSessionEvent, String> {
        let _metric =
            crate::infrastructure::performance::span("journal_append", "sqlite_write_transaction");
        crate::infrastructure::performance::increment("sqlite_writes_total", "sqlite", 1);
        match (&input.kind, &input.progress) {
            (TaskSessionEventKind::Lifecycle, _) => {
                return Err("Task lifecycle events are Scheduler-owned.".to_string());
            }
            (TaskSessionEventKind::Progress, None) => {
                return Err("Task progress events require a progress projection.".to_string());
            }
            (TaskSessionEventKind::Progress, Some(_)) | (_, None) => {}
            (_, Some(_)) => {
                return Err("Only task progress events may update progress.".to_string());
            }
        }
        input
            .progress
            .as_ref()
            .map(TaskProgress::validate)
            .transpose()?;
        let lock_started = Instant::now();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start task event transaction: {error}"))?;
        let valid = assignment_is_current_on(&transaction, fence, now)?;
        if !valid {
            return Err("Task event assignment fence is stale.".to_string());
        }
        let event = append_event_in_transaction(
            &transaction,
            fence.session_id,
            Some(fence.attempt_id),
            fence.fencing_token,
            &input,
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit task event: {error}"))?;
        Ok(event)
    }

    pub(crate) fn events_after(
        &self,
        session_id: TaskSessionId,
        sequence: u64,
    ) -> Result<Vec<TaskSessionEvent>, String> {
        let _metric = crate::infrastructure::performance::span("journal_replay", "sqlite_read");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT event_id, session_id, attempt_id, fencing_token, sequence,
                        event_kind, payload_json, progress_json, created_at
                   FROM scheduler_task_events
                  WHERE session_id = ?1 AND sequence > ?2
                  ORDER BY sequence",
            )
            .map_err(|error| format!("Failed to prepare task event query: {error}"))?;
        let rows = statement
            .query_map(
                params![to_i64(session_id.0)?, to_i64(sequence)?],
                stored_event_from_row,
            )
            .map_err(|error| format!("Failed to query task events: {error}"))?;
        rows.map(|row| {
            row.map_err(|error| format!("Failed to decode task event: {error}"))?
                .into_event()
        })
        .collect()
    }

    pub(crate) fn event_page(
        &self,
        session_id: TaskSessionId,
        sequence: u64,
        limit: usize,
    ) -> Result<TaskSessionEventPage, String> {
        let _metric =
            crate::infrastructure::performance::span("journal_page", "sqlite_read_transaction");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        if !(1..=500).contains(&limit) {
            return Err("Task Session event page limit must be between 1 and 500.".to_string());
        }
        let lock_started = Instant::now();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start task event page transaction: {error}"))?;
        let exists = transaction
            .query_row(
                "SELECT 1 FROM scheduler_task_sessions WHERE session_id = ?1",
                params![to_i64(session_id.0)?],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to validate Task Session event page: {error}"))?
            .is_some();
        if !exists {
            return Err(format!("Task Session {} was not found.", session_id.0));
        }
        let mut statement = transaction
            .prepare(
                "SELECT event_id, session_id, attempt_id, fencing_token, sequence,
                        event_kind, payload_json, progress_json, created_at
                   FROM scheduler_task_events
                  WHERE session_id = ?1 AND sequence > ?2
                  ORDER BY sequence
                  LIMIT ?3",
            )
            .map_err(|error| format!("Failed to prepare task event page: {error}"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(session_id.0)?,
                    to_i64(sequence)?,
                    i64::try_from(limit + 1).map_err(|_| "Task event page limit exceeds i64.")?
                ],
                stored_event_from_row,
            )
            .map_err(|error| format!("Failed to query task event page: {error}"))?;
        let mut events = rows
            .map(|row| {
                row.map_err(|error| format!("Failed to decode task event: {error}"))?
                    .into_event()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = events.len() > limit;
        events.truncate(limit);
        crate::infrastructure::performance::increment(
            "sqlite_rows_read_total",
            "sqlite",
            events.len() as u64,
        );
        let next_cursor = events.last().map_or(sequence, |event| event.sequence);
        drop(statement);
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit task event page transaction: {error}"))?;
        Ok(TaskSessionEventPage {
            events,
            next_cursor,
            has_more,
        })
    }

    pub(crate) fn execution_trace_page(
        &self,
        session_id: TaskSessionId,
        sequence: u64,
        limit: usize,
    ) -> Result<TaskExecutionTracePage, String> {
        let _metric = crate::infrastructure::performance::span(
            "execution_trace_page",
            "sqlite_read_transaction",
        );
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        if !(1..=200).contains(&limit) {
            return Err("Execution trace page limit must be between 1 and 200.".to_string());
        }
        let lock_started = Instant::now();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start execution trace read: {error}"))?;
        let snapshot = load_session(&transaction, session_id)?
            .ok_or_else(|| format!("Task Session {} was not found.", session_id.0))?;
        let envelope = snapshot.request.envelope().ok().flatten();
        let session = envelope.as_ref().map(TaskSessionEnvelope::session);
        let missing_indexed_events = transaction
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM scheduler_task_events
                    WHERE session_id = ?1 AND event_type IS NULL
                      AND event_kind IN ('lifecycle', 'tool')
                 )",
                params![to_i64(session_id.0)?],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("Failed to inspect execution trace coverage: {error}"))?;
        let trace_stage_count: u64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM scheduler_task_events
                  WHERE session_id = ?1 AND event_type = 'execution_trace_stage'",
                params![to_i64(session_id.0)?],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to count execution trace stages: {error}"))?;
        let mut statement = transaction
            .prepare(
                "SELECT events.event_id, events.session_id, events.attempt_id,
                        events.fencing_token, events.sequence, events.event_kind,
                        events.payload_json, events.progress_json, events.created_at,
                        events.event_type, attempts.attempt_number, attempts.worker_id
                   FROM scheduler_task_events events
                   LEFT JOIN scheduler_task_attempts attempts
                     ON attempts.attempt_id = events.attempt_id
                  WHERE events.session_id = ?1 AND events.sequence > ?2
                    AND events.event_type IN (
                      'lifecycle', 'tool_started', 'tool_completed',
                      'execution_trace_stage', 'usage_updated', 'opencode_session',
                      'approval_requested', 'runtime_recovery_decision',
                      'objective_checkpointed',
                      'capability_repair_decision', 'connector_session_recovered',
                      'subtask_contracts_prepared'
                    )
                 ORDER BY sequence
                 LIMIT ?3",
            )
            .map_err(|error| format!("Failed to prepare execution trace page: {error}"))?;
        let rows = statement
            .query_map(
                params![
                    to_i64(session_id.0)?,
                    to_i64(sequence)?,
                    i64::try_from(limit + 1).map_err(|_| "Trace page limit exceeds i64.")?
                ],
                |row| {
                    Ok((
                        stored_event_from_row(row)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                    ))
                },
            )
            .map_err(|error| format!("Failed to query execution trace page: {error}"))?;
        let mut entries = rows
            .map(|row| {
                let (event, event_type, attempt_number, worker_id) = row
                    .map_err(|error| format!("Failed to decode execution trace event: {error}"))?;
                trace_entry(event, event_type, attempt_number, worker_id)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let next_cursor = if has_more {
            entries.last().map_or(sequence, |entry| entry.sequence)
        } else {
            snapshot.last_event_sequence.max(sequence)
        };
        crate::infrastructure::performance::increment(
            "sqlite_rows_read_total",
            "sqlite",
            entries.len() as u64,
        );
        drop(statement);
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit execution trace read: {error}"))?;
        Ok(TaskExecutionTracePage {
            schema_version: 1,
            trace_id: format!("task-session:{}", session_id.0),
            task_session_id: session_id,
            subject_id: session.and_then(|value| value.subject_id.clone()),
            execution_run_id: session.and_then(|value| value.execution_run_id.clone()),
            runtime_profile_id: session.map(|value| value.runtime_profile_id.clone()),
            model: session.map(|value| value.model.clone()),
            opencode_session_id: snapshot.opencode_session_id,
            coverage: if missing_indexed_events
                || snapshot.attempt > 0 && trace_stage_count < u64::from(snapshot.attempt) * 2
            {
                "partial"
            } else {
                "complete"
            }
            .to_string(),
            unknown_fields: vec!["agent_turn_id".to_string(), "mcp_connection_id".to_string()],
            entries,
            next_cursor,
            has_more,
        })
    }

    pub(crate) fn tool_state(&self, session_id: TaskSessionId) -> Result<TaskToolState, String> {
        let _metric = crate::infrastructure::performance::span("tool_state", "sqlite_read");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        let lock_started = Instant::now();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        crate::infrastructure::performance::record_sqlite_lock_wait(lock_started.elapsed());
        let exists = connection
            .query_row(
                "SELECT 1 FROM scheduler_task_sessions WHERE session_id = ?1",
                params![to_i64(session_id.0)?],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to validate Task Session tool state: {error}"))?
            .is_some();
        if !exists {
            return Err(format!("Task Session {} was not found.", session_id.0));
        }
        let mut statement = connection
            .prepare(
                "SELECT event_id, session_id, attempt_id, fencing_token, sequence,
                        event_kind, payload_json, progress_json, created_at
                   FROM scheduler_task_events
                  WHERE session_id = ?1
                    AND event_type IN ('tool_started', 'tool_completed')
                  ORDER BY sequence",
            )
            .map_err(|error| format!("Failed to prepare Task Session tool state: {error}"))?;
        let rows = statement
            .query_map(params![to_i64(session_id.0)?], stored_event_from_row)
            .map_err(|error| format!("Failed to query Task Session tool state: {error}"))?;
        let events = rows
            .map(|row| {
                row.map_err(|error| format!("Failed to decode Task Session tool event: {error}"))?
                    .into_event()
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::infrastructure::performance::increment(
            "sqlite_rows_read_total",
            "sqlite",
            events.len() as u64,
        );
        Ok(TaskToolState::from_events(session_id, &events))
    }

    pub(crate) fn mcp_context(&self, session_id: TaskSessionId) -> Result<TaskMcpContext, String> {
        let session = self
            .get_session(session_id)?
            .ok_or_else(|| format!("Task Session {} was not found.", session_id.0))?;
        let envelope = session.request.envelope()?;
        let envelope = envelope.as_ref().map(TaskSessionEnvelope::session);
        Ok(TaskMcpContext::from_parts(
            session_id,
            envelope,
            &self.capability_grants(session_id)?,
            session.attempt_id,
            session.fencing_token,
        ))
    }

    pub(crate) fn capability_grants(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Vec<TaskCapabilityGrant>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        load_capability_grants(&connection, session_id)
    }

    pub(crate) fn assignment_is_current(&self, fence: AssignmentFence) -> Result<bool, String> {
        self.assignment_is_current_at(fence, now_millis())
    }

    pub(crate) fn assignment_opencode_session(
        &self,
        fence: AssignmentFence,
    ) -> Result<Option<String>, String> {
        let now = now_millis();
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        if !assignment_is_current_on(&connection, fence, now)? {
            return Err("OpenCode session lookup assignment fence is stale.".to_string());
        }
        connection
            .query_row(
                "SELECT opencode_session_id FROM scheduler_task_sessions WHERE session_id = ?1",
                params![to_i64(fence.session_id.0)?],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to load Task Session OpenCode identity: {error}"))
    }

    /// Loads the immutable governance snapshot for one retained Task Session.
    pub(crate) fn governance_resolution(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Option<GovernanceResolutionRecord>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let encoded = connection
            .query_row(
                "SELECT resolution_json FROM scheduler_task_governance WHERE session_id = ?1",
                params![to_i64(session_id.0)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to load Task Session governance: {error}"))?;
        encoded
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|error| format!("Failed to decode Task Session governance: {error}"))
            })
            .transpose()
    }

    /// Persists a governance snapshot exactly once under the active assignment fence.
    pub(crate) fn bind_governance_resolution(
        &self,
        fence: AssignmentFence,
        resolution: &GovernanceResolutionRecord,
    ) -> Result<GovernanceResolutionRecord, String> {
        if resolution.task_session_id != fence.session_id.0 {
            return Err("Governance snapshot Task Session identity does not match.".to_string());
        }
        let encoded = serde_json::to_string(resolution)
            .map_err(|error| format!("Failed to encode Task Session governance: {error}"))?;
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start governance transaction: {error}"))?;
        if !assignment_is_current_on(&transaction, fence, now)? {
            return Err("Governance snapshot assignment fence is stale.".to_string());
        }
        let existing = transaction
            .query_row(
                "SELECT resolution_json FROM scheduler_task_governance WHERE session_id = ?1",
                params![to_i64(fence.session_id.0)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to inspect Task Session governance: {error}"))?;
        if let Some(existing) = existing {
            if existing != encoded {
                return Err(format!(
                    "Task Session {} already owns a different governance snapshot.",
                    fence.session_id.0
                ));
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO scheduler_task_governance
                       (session_id, resolution_json, created_at) VALUES (?1, ?2, ?3)",
                    params![to_i64(fence.session_id.0)?, encoded, to_i64(now)?],
                )
                .map_err(|error| format!("Failed to persist Task Session governance: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit Task Session governance: {error}"))?;
        Ok(resolution.clone())
    }

    /// Persists one immutable, secret-free Execution Manifest for the current assignment attempt.
    pub(crate) fn bind_execution_manifest(
        &self,
        fence: AssignmentFence,
        draft: &ExecutionManifestDraft,
    ) -> Result<ExecutionManifest, String> {
        let _metric = crate::infrastructure::performance::span(
            "execution_manifest_bind",
            "sqlite_write_transaction",
        );
        crate::infrastructure::performance::increment("sqlite_writes_total", "sqlite", 1);
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start Execution Manifest transaction: {error}"))?;
        if !assignment_is_current_on(&transaction, fence, now)? {
            return Err("Execution Manifest assignment fence is stale.".to_string());
        }
        let (worker_id, started_at) = transaction
            .query_row(
                "SELECT attempts.worker_id, attempts.started_at
                   FROM scheduler_task_attempts attempts
                  WHERE attempts.attempt_id = ?1 AND attempts.session_id = ?2
                    AND attempts.attempt_number = ?3 AND attempts.fencing_token = ?4",
                params![
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.session_id.0)?,
                    i64::from(fence.attempt),
                    to_i64(fence.fencing_token)?
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| format!("Failed to resolve Execution Manifest assignment: {error}"))?;
        let manifest = ExecutionManifest {
            schema_version: EXECUTION_MANIFEST_SCHEMA_VERSION,
            task_session_id: fence.session_id,
            assignment_attempt_id: fence.attempt_id,
            assignment_attempt: fence.attempt,
            worker_id: from_i64(worker_id, "worker ID")? as usize,
            fencing_token: fence.fencing_token,
            started_at: from_i64(started_at, "assignment start timestamp")?,
            execution: draft.clone(),
        };
        manifest.validate_for(fence.session_id)?;
        if let Some(first) = transaction
            .query_row(
                "SELECT manifest_json FROM scheduler_task_execution_manifests
                  WHERE session_id = ?1 ORDER BY attempt_id ASC LIMIT 1",
                params![to_i64(fence.session_id.0)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to load Execution Manifest anchor: {error}"))?
        {
            let first = serde_json::from_str::<ExecutionManifest>(&first)
                .map_err(|error| format!("Failed to decode Execution Manifest anchor: {error}"))?;
            if !stable_manifest_identity_matches(&first.execution, draft) {
                return Err(
                    "Execution Manifest stable Task Session identity changed across attempts."
                        .to_string(),
                );
            }
        }
        let encoded = serde_json::to_string(&manifest)
            .map_err(|error| format!("Failed to encode Execution Manifest: {error}"))?;
        let existing = transaction
            .query_row(
                "SELECT manifest_json FROM scheduler_task_execution_manifests WHERE attempt_id = ?1",
                params![to_i64(fence.attempt_id)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to inspect Execution Manifest: {error}"))?;
        match existing {
            Some(existing) if existing != encoded => {
                return Err(
                    "Assignment attempt already owns a different Execution Manifest.".to_string(),
                );
            }
            Some(_) => {}
            None => {
                transaction
                    .execute(
                        "INSERT INTO scheduler_task_execution_manifests
                           (attempt_id, session_id, manifest_json, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            to_i64(fence.attempt_id)?,
                            to_i64(fence.session_id.0)?,
                            encoded,
                            to_i64(now)?
                        ],
                    )
                    .map_err(|error| format!("Failed to persist Execution Manifest: {error}"))?;
            }
        }
        bind_prepared_subtasks_on(
            &transaction,
            fence,
            &draft.task_examination.prepared_subtasks,
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit Execution Manifest: {error}"))?;
        Ok(manifest)
    }

    /// Loads the newest captured Execution Manifest for one Task Session.
    pub(crate) fn latest_execution_manifest(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Option<ExecutionManifest>, String> {
        let _metric =
            crate::infrastructure::performance::span("execution_manifest_get", "sqlite_read");
        crate::infrastructure::performance::increment("sqlite_reads_total", "sqlite", 1);
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let encoded = connection
            .query_row(
                "SELECT manifest_json FROM scheduler_task_execution_manifests
                  WHERE session_id = ?1 ORDER BY attempt_id DESC LIMIT 1",
                params![to_i64(session_id.0)?],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to load Execution Manifest: {error}"))?;
        encoded
            .map(|encoded| {
                let manifest = serde_json::from_str::<ExecutionManifest>(&encoded)
                    .map_err(|error| format!("Failed to decode Execution Manifest: {error}"))?;
                manifest.validate_for(session_id)?;
                Ok(manifest)
            })
            .transpose()
    }

    /// Loads scheduler-owned dormant subtask allocations for one Task Session.
    pub(crate) fn prepared_subtasks_for_session(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Vec<SchedulerPreparedSubtask>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT subtasks.subtask_id, subtasks.objective_id, subtasks.contract_json,
                        subtasks.state, subtasks.created_at,
                        attempts.subtask_attempt_id, attempts.attempt_number,
                        attempts.fencing_token, attempts.state,
                        attempts.wall_clock_seconds, attempts.max_tool_calls,
                        attempts.max_mutation_calls, attempts.tool_calls_used,
                        attempts.mutation_calls_used, attempts.authority_active
                   FROM scheduler_prepared_subtasks subtasks
                   JOIN scheduler_subtask_attempts attempts
                     ON attempts.subtask_id = subtasks.subtask_id
                  WHERE subtasks.session_id = ?1
                  ORDER BY subtasks.subtask_id, attempts.attempt_number",
            )
            .map_err(|error| format!("Failed to prepare subtask allocation query: {error}"))?;
        let rows = statement
            .query_map(params![to_i64(session_id.0)?], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, i64>(14)?,
                ))
            })
            .map_err(|error| format!("Failed to query subtask allocations: {error}"))?;
        let allocations = rows
            .map(|row| {
                let (
                    subtask_id,
                    objective_id,
                    contract_json,
                    state,
                    created_at,
                    subtask_attempt_id,
                    attempt,
                    fencing_token,
                    attempt_state,
                    wall_clock_seconds,
                    max_tool_calls,
                    max_mutation_calls,
                    tool_calls_used,
                    mutation_calls_used,
                    authority_active,
                ) = row.map_err(|error| format!("Failed to decode subtask allocation: {error}"))?;
                let contract = serde_json::from_str::<PreparedSubtaskContract>(&contract_json)
                    .map_err(|error| {
                        format!("Failed to decode prepared subtask contract: {error}")
                    })?;
                if state != "prepared"
                    || attempt_state != "dormant"
                    || objective_id != contract.objective_id
                    || contract.execution_enabled
                    || from_i64(wall_clock_seconds, "subtask wall-clock budget")?
                        != contract.budget.wall_clock_seconds
                    || from_i64(max_tool_calls, "subtask tool-call budget")?
                        != u64::from(contract.budget.max_tool_calls)
                    || from_i64(max_mutation_calls, "subtask mutation-call budget")?
                        != u64::from(contract.budget.max_mutation_calls)
                    || tool_calls_used != 0
                    || mutation_calls_used != 0
                    || authority_active != 0
                {
                    return Err("Stored prepared subtask allocation is inconsistent.".to_string());
                }
                Ok(SchedulerPreparedSubtask {
                    session_id: session_id.0,
                    objective_id,
                    contract,
                    state,
                    fence: DormantSubtaskFence {
                        subtask_id: from_i64(subtask_id, "subtask ID")?,
                        subtask_attempt_id: from_i64(subtask_attempt_id, "subtask attempt ID")?,
                        attempt: u32::try_from(from_i64(attempt, "subtask attempt")?)
                            .map_err(|_| "Subtask attempt exceeds u32.".to_string())?,
                        fencing_token: from_i64(fencing_token, "subtask fencing token")?,
                    },
                    tool_calls_used: u32::try_from(from_i64(
                        tool_calls_used,
                        "subtask tool usage",
                    )?)
                    .map_err(|_| "Subtask tool usage exceeds u32.".to_string())?,
                    mutation_calls_used: u32::try_from(from_i64(
                        mutation_calls_used,
                        "subtask mutation usage",
                    )?)
                    .map_err(|_| "Subtask mutation usage exceeds u32.".to_string())?,
                    authority_active: false,
                    created_at: from_i64(created_at, "subtask creation timestamp")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        drop(statement);
        let prepared_count: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM scheduler_prepared_subtasks WHERE session_id = ?1",
                params![to_i64(session_id.0)?],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to count prepared subtasks: {error}"))?;
        let attempt_count: u64 = connection
            .query_row(
                "SELECT COUNT(*)
                   FROM scheduler_subtask_attempts attempts
                   JOIN scheduler_prepared_subtasks subtasks
                     ON subtasks.subtask_id = attempts.subtask_id
                  WHERE subtasks.session_id = ?1",
                params![to_i64(session_id.0)?],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to count dormant subtask attempts: {error}"))?;
        if prepared_count != allocations.len() as u64 || attempt_count != prepared_count {
            return Err(
                "Stored prepared subtask allocation cardinality is inconsistent.".to_string(),
            );
        }
        Ok(allocations)
    }

    /// Checks an exact dormant fence identity. A match does not activate tool authority.
    pub(crate) fn dormant_subtask_fence_exists(
        &self,
        session_id: TaskSessionId,
        fence: DormantSubtaskFence,
    ) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1
                     FROM scheduler_prepared_subtasks subtasks
                     JOIN scheduler_subtask_attempts attempts
                       ON attempts.subtask_id = subtasks.subtask_id
                    WHERE subtasks.session_id = ?1
                      AND subtasks.subtask_id = ?2
                      AND subtasks.state = 'prepared'
                      AND subtasks.execution_enabled = 0
                      AND attempts.subtask_attempt_id = ?3
                      AND attempts.attempt_number = ?4
                      AND attempts.fencing_token = ?5
                      AND attempts.state = 'dormant'
                      AND attempts.authority_active = 0
                 )",
                params![
                    to_i64(session_id.0)?,
                    to_i64(fence.subtask_id)?,
                    to_i64(fence.subtask_attempt_id)?,
                    i64::from(fence.attempt),
                    to_i64(fence.fencing_token)?
                ],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| value != 0)
            .map_err(|error| format!("Failed to inspect dormant subtask fence: {error}"))
    }

    /// Activates one exact dormant identity under the current parent assignment.
    ///
    /// No production caller can construct `SubtaskDispatchPermit` yet. Keeping that permit
    /// module-private makes the implemented authority and budget checks testable without opening
    /// a multi-agent dispatch path prematurely.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn activate_prepared_subtask(
        &self,
        parent: AssignmentFence,
        dormant: DormantSubtaskFence,
        lease_duration: Duration,
        _permit: &SubtaskDispatchPermit,
    ) -> Result<SubtaskToolAuthority, String> {
        if lease_duration.is_zero() || lease_duration > SUBTASK_AUTHORITY_MAX_LEASE {
            return Err("Subtask authority lease must be between 1 ms and 30 seconds.".to_string());
        }
        let database_path = self.database_path.as_ref().ok_or_else(|| {
            "Subtask authority requires a persistent scheduler store.".to_string()
        })?;
        let now = now_millis();
        let requested_lease_expires_at = now
            .checked_add(duration_millis(lease_duration)?)
            .ok_or_else(|| "Subtask authority lease timestamp overflowed.".to_string())?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start subtask activation transaction: {error}"))?;
        if !assignment_is_current_on(&transaction, parent, now)? {
            return Err("Parent assignment fence is stale, expired, or cancelled.".to_string());
        }
        let parent_lease_expires_at = transaction
            .query_row(
                "SELECT lease_expires_at FROM scheduler_task_attempts WHERE attempt_id = ?1",
                params![to_i64(parent.attempt_id)?],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("Failed to load parent assignment lease: {error}"))?;
        let lease_expires_at = requested_lease_expires_at.min(from_i64(
            parent_lease_expires_at,
            "parent assignment lease",
        )?);
        if lease_expires_at <= now {
            return Err("Parent assignment lease cannot contain a subtask lease.".to_string());
        }
        let (objective_id, contract_json) = transaction
            .query_row(
                "SELECT subtasks.objective_id, subtasks.contract_json
                   FROM scheduler_prepared_subtasks subtasks
                   JOIN scheduler_subtask_attempts attempts
                     ON attempts.subtask_id = subtasks.subtask_id
                  WHERE subtasks.session_id = ?1
                    AND subtasks.subtask_id = ?2
                    AND subtasks.state = 'prepared'
                    AND subtasks.execution_enabled = 0
                    AND attempts.subtask_attempt_id = ?3
                    AND attempts.attempt_number = ?4
                    AND attempts.fencing_token = ?5
                    AND attempts.state = 'dormant'
                    AND attempts.authority_active = 0",
                params![
                    to_i64(parent.session_id.0)?,
                    to_i64(dormant.subtask_id)?,
                    to_i64(dormant.subtask_attempt_id)?,
                    i64::from(dormant.attempt),
                    to_i64(dormant.fencing_token)?
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| format!("Failed to inspect dormant subtask activation: {error}"))?
            .ok_or_else(|| "Dormant subtask fence is stale or incompatible.".to_string())?;
        let contract = serde_json::from_str::<PreparedSubtaskContract>(&contract_json)
            .map_err(|error| format!("Failed to decode subtask activation contract: {error}"))?;
        if contract.schema_version != 2
            || contract.objective_id != objective_id
            || contract.execution_enabled
        {
            return Err("Prepared subtask activation contract is inconsistent.".to_string());
        }
        for capability in &contract.granted_capabilities {
            let still_granted = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM scheduler_task_grants
                        WHERE session_id = ?1 AND capability = ?2
                     )",
                    params![to_i64(parent.session_id.0)?, capability],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| format!("Failed to recheck parent subtask grants: {error}"))?;
            if still_granted == 0 {
                return Err(
                    "Prepared subtask capability is no longer granted to its parent.".to_string(),
                );
            }
        }
        let existing = transaction
            .query_row(
                "SELECT authority_id, parent_attempt_id, parent_fencing_token,
                        authority_fencing_token, state, lease_expires_at,
                        tool_calls_used, mutation_calls_used
                   FROM scheduler_subtask_authorities WHERE subtask_attempt_id = ?1",
                params![to_i64(dormant.subtask_attempt_id)?],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to inspect existing subtask authority: {error}"))?;
        let (authority_id, authority_fencing_token, retained_lease_expires_at) = match existing {
            Some((
                authority_id,
                parent_attempt_id,
                parent_fencing_token,
                authority_fencing_token,
                state,
                existing_lease_expires_at,
                tool_calls_used,
                mutation_calls_used,
            )) if parent_attempt_id == to_i64(parent.attempt_id)?
                && parent_fencing_token == to_i64(parent.fencing_token)?
                && state == "active"
                && existing_lease_expires_at > to_i64(now)? =>
            {
                if tool_calls_used < 0 || mutation_calls_used < 0 {
                    return Err("Stored subtask authority usage is invalid.".to_string());
                }
                (
                    from_i64(authority_id, "subtask authority ID")?,
                    from_i64(authority_fencing_token, "subtask authority fencing token")?,
                    from_i64(existing_lease_expires_at, "subtask authority lease")?,
                )
            }
            Some(_) => {
                return Err(
                    "Subtask authority already exists but is stale or requires recovery."
                        .to_string(),
                )
            }
            None => {
                transaction
                    .execute(
                        "INSERT INTO scheduler_subtask_authorities
                           (subtask_attempt_id, parent_attempt_id, parent_fencing_token,
                            authority_fencing_token, state, lease_expires_at, tool_calls_used,
                            mutation_calls_used, activated_at, updated_at)
                         VALUES (?1, ?2, ?3, 1, 'active', ?4, 0, 0, ?5, ?5)",
                        params![
                            to_i64(dormant.subtask_attempt_id)?,
                            to_i64(parent.attempt_id)?,
                            to_i64(parent.fencing_token)?,
                            to_i64(lease_expires_at)?,
                            to_i64(now)?
                        ],
                    )
                    .map_err(|error| format!("Failed to persist subtask authority: {error}"))?;
                (
                    from_i64(transaction.last_insert_rowid(), "subtask authority ID")?,
                    1,
                    lease_expires_at,
                )
            }
        };
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit subtask activation: {error}"))?;
        Ok(SubtaskToolAuthority {
            scheduler_database: database_path.as_ref().clone(),
            scheduler_instance_id: self.instance_id.to_string(),
            session_id: parent.session_id,
            parent_attempt_id: parent.attempt_id,
            parent_attempt: parent.attempt,
            parent_owner_id: parent.owner_id,
            parent_fencing_token: parent.fencing_token,
            subtask_id: dormant.subtask_id,
            subtask_attempt_id: dormant.subtask_attempt_id,
            subtask_attempt: dormant.attempt,
            subtask_fencing_token: dormant.fencing_token,
            authority_id,
            authority_fencing_token,
            objective_id,
            capabilities: contract.granted_capabilities,
            allowed_connector_tools: contract.allowed_connector_tools,
            lease_expires_at: retained_lease_expires_at,
        })
    }

    /// Atomically admits one future subtask tool call and consumes its conservative budget.
    /// Budget is charged at admission time, so a transport failure cannot accidentally permit an
    /// extra retry beyond the immutable contract.
    pub fn admit_subtask_tool_call(
        authority: &SubtaskToolAuthority,
        capability: &str,
        risk: SubtaskToolRisk,
    ) -> Result<SubtaskToolAdmission, String> {
        Self::admit_subtask_tool_operation(authority, capability, None, risk)
    }

    pub fn admit_subtask_connector_tool_call(
        authority: &SubtaskToolAuthority,
        capability: &str,
        tool_name: &str,
        risk: SubtaskToolRisk,
    ) -> Result<SubtaskToolAdmission, String> {
        Self::admit_subtask_tool_operation(authority, capability, Some(tool_name), risk)
    }

    fn admit_subtask_tool_operation(
        authority: &SubtaskToolAuthority,
        capability: &str,
        tool_name: Option<&str>,
        risk: SubtaskToolRisk,
    ) -> Result<SubtaskToolAdmission, String> {
        validate_subtask_authority_shape(authority, capability)?;
        validate_subtask_tool_operation(authority, capability, tool_name)?;
        let store = Self::open_subtask_authority_store(authority)?;
        let now = now_millis();
        let mut connection = store.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start subtask tool admission: {error}"))?;
        let parent = AssignmentFence {
            session_id: authority.session_id,
            attempt_id: authority.parent_attempt_id,
            attempt: authority.parent_attempt,
            owner_id: authority.parent_owner_id,
            fencing_token: authority.parent_fencing_token,
        };
        if !assignment_is_current_on(&transaction, parent, now)? {
            return Err("Subtask parent assignment is stale, expired, or cancelled.".to_string());
        }
        let (
            contract_json,
            max_tool_calls,
            max_mutation_calls,
            tool_calls_used,
            mutation_calls_used,
        ) = transaction
            .query_row(
                "SELECT subtasks.contract_json, attempts.max_tool_calls,
                            attempts.max_mutation_calls, authorities.tool_calls_used,
                            authorities.mutation_calls_used
                       FROM scheduler_subtask_authorities authorities
                       JOIN scheduler_subtask_attempts attempts
                         ON attempts.subtask_attempt_id = authorities.subtask_attempt_id
                       JOIN scheduler_prepared_subtasks subtasks
                         ON subtasks.subtask_id = attempts.subtask_id
                      WHERE authorities.authority_id = ?1
                        AND authorities.authority_fencing_token = ?2
                        AND authorities.subtask_attempt_id = ?3
                        AND authorities.parent_attempt_id = ?4
                        AND authorities.parent_fencing_token = ?5
                        AND authorities.state = 'active'
                        AND authorities.lease_expires_at > ?6
                        AND authorities.lease_expires_at = ?12
                        AND subtasks.session_id = ?7
                        AND subtasks.subtask_id = ?8
                        AND subtasks.objective_id = ?9
                        AND attempts.attempt_number = ?10
                        AND attempts.fencing_token = ?11
                        AND EXISTS (
                          SELECT 1 FROM scheduler_task_grants grants
                           WHERE grants.session_id = subtasks.session_id
                             AND grants.capability = ?13
                        )",
                params![
                    to_i64(authority.authority_id)?,
                    to_i64(authority.authority_fencing_token)?,
                    to_i64(authority.subtask_attempt_id)?,
                    to_i64(authority.parent_attempt_id)?,
                    to_i64(authority.parent_fencing_token)?,
                    to_i64(now)?,
                    to_i64(authority.session_id.0)?,
                    to_i64(authority.subtask_id)?,
                    authority.objective_id,
                    i64::from(authority.subtask_attempt),
                    to_i64(authority.subtask_fencing_token)?,
                    to_i64(authority.lease_expires_at)?,
                    capability
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to validate subtask tool authority: {error}"))?
            .ok_or_else(|| {
                "Subtask tool authority is stale, expired, or incompatible.".to_string()
            })?;
        let contract = serde_json::from_str::<PreparedSubtaskContract>(&contract_json)
            .map_err(|error| format!("Failed to decode subtask tool contract: {error}"))?;
        if contract.schema_version != 2
            || contract.objective_id != authority.objective_id
            || contract.granted_capabilities != authority.capabilities
            || contract.allowed_connector_tools != authority.allowed_connector_tools
            || !contract
                .granted_capabilities
                .iter()
                .any(|granted| granted == capability)
        {
            return Err(
                "Subtask tool capability is not granted by its immutable contract.".to_string(),
            );
        }
        let max_tool_calls = u32::try_from(from_i64(max_tool_calls, "subtask tool budget")?)
            .map_err(|_| "Subtask tool budget exceeds u32.".to_string())?;
        let max_mutation_calls =
            u32::try_from(from_i64(max_mutation_calls, "subtask mutation budget")?)
                .map_err(|_| "Subtask mutation budget exceeds u32.".to_string())?;
        let tool_calls_used = u32::try_from(from_i64(tool_calls_used, "subtask tool usage")?)
            .map_err(|_| "Subtask tool usage exceeds u32.".to_string())?;
        let mutation_calls_used =
            u32::try_from(from_i64(mutation_calls_used, "subtask mutation usage")?)
                .map_err(|_| "Subtask mutation usage exceeds u32.".to_string())?;
        if tool_calls_used >= max_tool_calls {
            return Err("Subtask tool-call budget is exhausted.".to_string());
        }
        let mutation_delta = u32::from(risk == SubtaskToolRisk::Mutation);
        if mutation_delta == 1 && mutation_calls_used >= max_mutation_calls {
            return Err("Subtask mutation-call budget is exhausted.".to_string());
        }
        let updated = transaction
            .execute(
                "UPDATE scheduler_subtask_authorities
                    SET tool_calls_used = tool_calls_used + 1,
                        mutation_calls_used = mutation_calls_used + ?2,
                        updated_at = ?3
                  WHERE authority_id = ?1 AND authority_fencing_token = ?4
                    AND state = 'active' AND lease_expires_at > ?3
                    AND tool_calls_used < ?5
                    AND mutation_calls_used + ?2 <= ?6",
                params![
                    to_i64(authority.authority_id)?,
                    i64::from(mutation_delta),
                    to_i64(now)?,
                    to_i64(authority.authority_fencing_token)?,
                    i64::from(max_tool_calls),
                    i64::from(max_mutation_calls)
                ],
            )
            .map_err(|error| format!("Failed to consume subtask tool budget: {error}"))?;
        if updated != 1 {
            return Err("Subtask tool admission lost its authority or budget fence.".to_string());
        }
        let admission = SubtaskToolAdmission {
            authority_id: authority.authority_id,
            tool_calls_used: tool_calls_used + 1,
            mutation_calls_used: mutation_calls_used + mutation_delta,
            max_tool_calls,
            max_mutation_calls,
        };
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit subtask tool admission: {error}"))?;
        Ok(admission)
    }

    /// Extends one exact staged subtask dispatch lease without widening its parent authority.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn renew_subtask_authority(
        authority: &SubtaskToolAuthority,
        lease_duration: Duration,
    ) -> Result<SubtaskToolAuthority, String> {
        Self::renew_subtask_authority_at(authority, now_millis(), duration_millis(lease_duration)?)
    }

    fn renew_subtask_authority_at(
        authority: &SubtaskToolAuthority,
        now: u64,
        lease_millis: u64,
    ) -> Result<SubtaskToolAuthority, String> {
        if lease_millis == 0 || lease_millis > duration_millis(SUBTASK_AUTHORITY_MAX_LEASE)? {
            return Err("Subtask authority lease must be between 1 ms and 30 seconds.".to_string());
        }
        validate_subtask_authority_shape(
            authority,
            authority
                .capabilities
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
        )?;
        let store = Self::open_subtask_authority_store(authority)?;
        let mut connection = store.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start subtask renewal: {error}"))?;
        let parent = AssignmentFence {
            session_id: authority.session_id,
            attempt_id: authority.parent_attempt_id,
            attempt: authority.parent_attempt,
            owner_id: authority.parent_owner_id,
            fencing_token: authority.parent_fencing_token,
        };
        if !assignment_is_current_on(&transaction, parent, now)? {
            return Err("Subtask parent assignment is stale, expired, or cancelled.".to_string());
        }
        validate_subtask_contract_and_grants_on(&transaction, authority)?;
        let parent_lease_expires_at = transaction
            .query_row(
                "SELECT lease_expires_at FROM scheduler_task_attempts WHERE attempt_id = ?1",
                params![to_i64(authority.parent_attempt_id)?],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("Failed to load parent lease for subtask renewal: {error}"))?;
        let lease_expires_at = now
            .checked_add(lease_millis)
            .ok_or_else(|| "Subtask renewal timestamp overflowed.".to_string())?
            .min(from_i64(
                parent_lease_expires_at,
                "parent assignment lease",
            )?);
        if lease_expires_at <= authority.lease_expires_at {
            return Err("Subtask authority renewal must extend the current lease.".to_string());
        }
        let updated = transaction
            .execute(
                "UPDATE scheduler_subtask_authorities
                    SET lease_expires_at = ?2, updated_at = ?3
                  WHERE authority_id = ?1 AND authority_fencing_token = ?4
                    AND subtask_attempt_id = ?5 AND parent_attempt_id = ?6
                    AND parent_fencing_token = ?7 AND state = 'active'
                    AND lease_expires_at = ?8 AND lease_expires_at > ?3",
                params![
                    to_i64(authority.authority_id)?,
                    to_i64(lease_expires_at)?,
                    to_i64(now)?,
                    to_i64(authority.authority_fencing_token)?,
                    to_i64(authority.subtask_attempt_id)?,
                    to_i64(authority.parent_attempt_id)?,
                    to_i64(authority.parent_fencing_token)?,
                    to_i64(authority.lease_expires_at)?
                ],
            )
            .map_err(|error| format!("Failed to renew subtask authority: {error}"))?;
        if updated != 1 {
            return Err("Subtask authority renewal lost its exact lease fence.".to_string());
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit subtask renewal: {error}"))?;
        Ok(SubtaskToolAuthority {
            lease_expires_at,
            ..authority.clone()
        })
    }

    /// Resolves one staged subtask dispatch. Identical terminal replay is idempotent.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn resolve_subtask_authority(
        authority: &SubtaskToolAuthority,
        outcome: SubtaskAuthorityOutcome,
    ) -> Result<SubtaskAuthorityStatus, String> {
        Self::resolve_subtask_authority_at(authority, outcome, now_millis())
    }

    fn resolve_subtask_authority_at(
        authority: &SubtaskToolAuthority,
        outcome: SubtaskAuthorityOutcome,
        now: u64,
    ) -> Result<SubtaskAuthorityStatus, String> {
        validate_subtask_authority_shape(
            authority,
            authority
                .capabilities
                .first()
                .map(String::as_str)
                .unwrap_or_default(),
        )?;
        let store = Self::open_subtask_authority_store(authority)?;
        let mut connection = store.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start subtask resolution: {error}"))?;
        if !subtask_authority_descriptor_matches_on(&transaction, authority)? {
            return Err(
                "Subtask authority resolution descriptor is stale or incompatible.".to_string(),
            );
        }
        let current = subtask_authority_status_on(&transaction, authority.authority_id)?
            .ok_or_else(|| "Subtask authority was not found.".to_string())?;
        if current.state == outcome.state()
            && current.terminal_reason.as_deref() == Some(outcome.reason())
        {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit subtask resolution replay: {error}"))?;
            return Ok(current);
        }
        if current.state != "active" {
            return Err("Subtask authority already has a different terminal outcome.".to_string());
        }
        if outcome == SubtaskAuthorityOutcome::Completed {
            if current
                .lease_expires_at
                .is_none_or(|lease_expires_at| lease_expires_at <= now)
            {
                return Err("Expired subtask authority cannot report completion.".to_string());
            }
            let parent = AssignmentFence {
                session_id: authority.session_id,
                attempt_id: authority.parent_attempt_id,
                attempt: authority.parent_attempt,
                owner_id: authority.parent_owner_id,
                fencing_token: authority.parent_fencing_token,
            };
            if !assignment_is_current_on(&transaction, parent, now)? {
                return Err(
                    "Completed subtask cannot outlive its parent assignment authority.".to_string(),
                );
            }
            validate_subtask_contract_and_grants_on(&transaction, authority)?;
        }
        let updated = transaction
            .execute(
                "UPDATE scheduler_subtask_authorities
                    SET state = ?2, terminal_reason = ?3, completed_at = ?4, updated_at = ?4
                  WHERE authority_id = ?1 AND authority_fencing_token = ?5
                    AND subtask_attempt_id = ?6 AND parent_attempt_id = ?7
                    AND parent_fencing_token = ?8 AND lease_expires_at = ?9
                    AND state = 'active'",
                params![
                    to_i64(authority.authority_id)?,
                    outcome.state(),
                    outcome.reason(),
                    to_i64(now)?,
                    to_i64(authority.authority_fencing_token)?,
                    to_i64(authority.subtask_attempt_id)?,
                    to_i64(authority.parent_attempt_id)?,
                    to_i64(authority.parent_fencing_token)?,
                    to_i64(authority.lease_expires_at)?
                ],
            )
            .map_err(|error| format!("Failed to resolve subtask authority: {error}"))?;
        if updated != 1 {
            return Err("Subtask authority resolution lost its exact fence.".to_string());
        }
        let status = subtask_authority_status_on(&transaction, authority.authority_id)?
            .ok_or_else(|| "Resolved subtask authority could not be reloaded.".to_string())?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit subtask resolution: {error}"))?;
        Ok(status)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn recover_subtask_authorities(&self) -> Result<usize, String> {
        self.recover_subtask_authorities_at(now_millis())
    }

    fn recover_subtask_authorities_at(&self, now: u64) -> Result<usize, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start subtask recovery: {error}"))?;
        let recovered = recover_subtask_authorities_on(&transaction, now)?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit subtask recovery: {error}"))?;
        Ok(recovered)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn subtask_authority_status(
        &self,
        authority_id: u64,
    ) -> Result<Option<SubtaskAuthorityStatus>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        subtask_authority_status_on(&connection, authority_id)
    }

    pub(crate) fn bind_opencode_session(
        &self,
        fence: AssignmentFence,
        opencode_session_id: &str,
    ) -> Result<TaskSessionEvent, String> {
        let opencode_session_id = opencode_session_id.trim();
        if opencode_session_id.is_empty()
            || opencode_session_id.len() > 256
            || opencode_session_id.chars().any(char::is_control)
        {
            return Err("OpenCode session identity is invalid.".to_string());
        }
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start OpenCode identity transaction: {error}"))?;
        if !assignment_is_current_on(&transaction, fence, now)? {
            return Err("OpenCode session binding assignment fence is stale.".to_string());
        }
        let (existing, worker_id) = transaction
            .query_row(
                "SELECT sessions.opencode_session_id, attempts.worker_id
                   FROM scheduler_task_sessions sessions
                   JOIN scheduler_task_attempts attempts
                     ON attempts.attempt_id = sessions.active_attempt_id
                  WHERE sessions.session_id = ?1",
                params![to_i64(fence.session_id.0)?],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| {
                format!("Failed to inspect Task Session OpenCode identity: {error}")
            })?;
        if existing
            .as_deref()
            .is_some_and(|existing| existing != opencode_session_id)
        {
            return Err(format!(
                "Task Session {} already owns a different OpenCode session.",
                fence.session_id.0
            ));
        }
        if existing.is_none() {
            transaction
                .execute(
                    "UPDATE scheduler_task_sessions SET opencode_session_id = ?2
                      WHERE session_id = ?1 AND active_attempt_id = ?3 AND fencing_token = ?4",
                    params![
                        to_i64(fence.session_id.0)?,
                        opencode_session_id,
                        to_i64(fence.attempt_id)?,
                        to_i64(fence.fencing_token)?
                    ],
                )
                .map_err(|error| format!("Failed to persist OpenCode session identity: {error}"))?;
        }
        let event = append_event_in_transaction(
            &transaction,
            fence.session_id,
            Some(fence.attempt_id),
            fence.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Runtime,
                payload: json!({
                    "type": "opencode_session",
                    "action": if existing.is_some() { "resumed" } else { "created" },
                    "task_session_id": fence.session_id.0,
                    "worker_id": from_i64(worker_id, "worker ID")?,
                    "assignment_attempt": fence.attempt,
                    "opencode_session_id": opencode_session_id,
                }),
                progress: None,
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit OpenCode session identity: {error}"))?;
        Ok(event)
    }

    fn assignment_is_current_at(&self, fence: AssignmentFence, now: u64) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        assignment_is_current_on(&connection, fence, now)
    }

    #[cfg(test)]
    pub(crate) fn claim_next(
        &self,
        owner_id: u64,
        worker_id: usize,
        lease_duration: Duration,
        global_limit: usize,
    ) -> Result<Option<DurableAssignment>, String> {
        self.claim_next_with_changes(owner_id, worker_id, lease_duration, global_limit)
            .map(|outcome| outcome.assignment)
    }

    pub(crate) fn claim_next_with_changes(
        &self,
        owner_id: u64,
        worker_id: usize,
        lease_duration: Duration,
        global_limit: usize,
    ) -> Result<ClaimOutcome, String> {
        self.claim_next_with_changes_at(
            owner_id,
            worker_id,
            now_millis(),
            duration_millis(lease_duration)?,
            global_limit,
        )
    }

    #[cfg(test)]
    fn claim_next_at(
        &self,
        owner_id: u64,
        worker_id: usize,
        now: u64,
        lease_millis: u64,
        global_limit: usize,
    ) -> Result<Option<DurableAssignment>, String> {
        self.claim_next_with_changes_at(owner_id, worker_id, now, lease_millis, global_limit)
            .map(|outcome| outcome.assignment)
    }

    fn claim_next_with_changes_at(
        &self,
        owner_id: u64,
        worker_id: usize,
        now: u64,
        lease_millis: u64,
        global_limit: usize,
    ) -> Result<ClaimOutcome, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler claim transaction: {error}"))?;
        let mut changed_session_ids = recover_expired_in_transaction(&transaction, now)?;
        let running: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM scheduler_task_attempts
                  WHERE state = 'running' AND lease_expires_at > ?1",
                params![to_i64(now)?],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to count scheduler assignments: {error}"))?;
        if running >= i64::try_from(global_limit).unwrap_or(i64::MAX) {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit scheduler capacity check: {error}"))?;
            return Ok(ClaimOutcome {
                assignment: None,
                changed_session_ids,
            });
        }

        let candidate = transaction
            .query_row(
                "SELECT candidate.session_id, candidate.label, candidate.payload,
                        candidate.attempt_count, candidate.fencing_token
                   FROM scheduler_task_sessions candidate
                  WHERE candidate.state = 'queued'
                    AND NOT EXISTS (
                      SELECT 1
                        FROM scheduler_task_sessions active
                       WHERE active.state IN ('running', 'cancelling', 'committing')
                         AND active.workspace_id = candidate.workspace_id
                         AND (
                           (candidate.conversation_id IS NOT NULL
                            AND active.conversation_id = candidate.conversation_id)
                           OR
                           (candidate.subject_id IS NOT NULL
                            AND active.subject_id = candidate.subject_id)
                           OR
                           (candidate.execution_run_id IS NOT NULL
                            AND active.execution_run_id = candidate.execution_run_id)
                         )
                    )
                  ORDER BY candidate.enqueue_sequence
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to select FIFO task session: {error}"))?;
        let Some((session_id, label, payload, attempt_count, fencing_token)) = candidate else {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit empty scheduler claim: {error}"))?;
            return Ok(ClaimOutcome {
                assignment: None,
                changed_session_ids,
            });
        };

        let session_id = TaskSessionId(from_i64(session_id, "task session ID")?);
        let attempt = from_i64(attempt_count, "task attempt")?
            .saturating_add(1)
            .try_into()
            .map_err(|_| "Task attempt exceeds u32 range.".to_string())?;
        let fencing_token = from_i64(fencing_token, "fencing token")?.saturating_add(1);
        let dispatch_sequence = next_sequence(&transaction, "next_dispatch_sequence")?;
        let lease_expires_at = now.saturating_add(lease_millis);
        transaction
            .execute(
                "INSERT INTO scheduler_task_attempts
                   (session_id, attempt_number, fencing_token, dispatch_sequence, owner_id,
                    worker_id, state, lease_expires_at, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8)",
                params![
                    to_i64(session_id.0)?,
                    i64::from(attempt),
                    to_i64(fencing_token)?,
                    to_i64(dispatch_sequence)?,
                    to_i64(owner_id)?,
                    i64::try_from(worker_id).map_err(|_| "Worker ID exceeds i64 range.")?,
                    to_i64(lease_expires_at)?,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to create scheduler attempt: {error}"))?;
        let attempt_id = from_i64(transaction.last_insert_rowid(), "task attempt ID")?;
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = 'running', worker_id = ?2, dispatch_sequence = ?3,
                        attempt_count = ?4, active_attempt_id = ?5, fencing_token = ?6,
                        lease_expires_at = ?7, error = NULL,
                        started_at = COALESCE(started_at, ?8), completed_at = NULL
                  WHERE session_id = ?1 AND state = 'queued'",
                params![
                    to_i64(session_id.0)?,
                    i64::try_from(worker_id).map_err(|_| "Worker ID exceeds i64 range.")?,
                    to_i64(dispatch_sequence)?,
                    i64::from(attempt),
                    to_i64(attempt_id)?,
                    to_i64(fencing_token)?,
                    to_i64(lease_expires_at)?,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to assign scheduler session: {error}"))?;
        if updated != 1 {
            return Err("FIFO task session changed during assignment.".to_string());
        }
        let grants = load_capability_grants(&transaction, session_id)?;
        append_event_in_transaction(
            &transaction,
            session_id,
            Some(attempt_id),
            fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({
                    "state": "running",
                    "worker_id": worker_id,
                    "attempt": attempt
                }),
                progress: Some(TaskProgress {
                    phase: "executing".to_string(),
                    completed: 0,
                    total: None,
                }),
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler assignment: {error}"))?;
        if !changed_session_ids.contains(&session_id) {
            changed_session_ids.push(session_id);
        }
        Ok(ClaimOutcome {
            assignment: Some(DurableAssignment {
                fence: AssignmentFence {
                    session_id,
                    attempt_id,
                    attempt,
                    owner_id,
                    fencing_token,
                },
                request: TaskRequest { label, payload },
                grants,
            }),
            changed_session_ids,
        })
    }

    pub(crate) fn renew(
        &self,
        fence: AssignmentFence,
        lease_duration: Duration,
    ) -> Result<bool, String> {
        self.renew_at(fence, now_millis(), duration_millis(lease_duration)?)
    }

    fn renew_at(
        &self,
        fence: AssignmentFence,
        now: u64,
        lease_millis: u64,
    ) -> Result<bool, String> {
        let lease_expires_at = now.saturating_add(lease_millis);
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler renewal transaction: {error}"))?;
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET lease_expires_at = ?6
                  WHERE attempt_id = ?1 AND session_id = ?2 AND attempt_number = ?3
                    AND owner_id = ?4 AND fencing_token = ?5 AND state = 'running'
                    AND lease_expires_at > ?7",
                params![
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.session_id.0)?,
                    i64::from(fence.attempt),
                    to_i64(fence.owner_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(lease_expires_at)?,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to renew scheduler attempt: {error}"))?;
        if updated == 1 {
            transaction
                .execute(
                    "UPDATE scheduler_task_sessions SET lease_expires_at = ?2
                      WHERE session_id = ?1 AND active_attempt_id = ?3
                        AND fencing_token = ?4 AND state IN ('running', 'cancelling')
                        AND lease_expires_at > ?5",
                    params![
                        to_i64(fence.session_id.0)?,
                        to_i64(lease_expires_at)?,
                        to_i64(fence.attempt_id)?,
                        to_i64(fence.fencing_token)?,
                        to_i64(now)?
                    ],
                )
                .map_err(|error| format!("Failed to renew scheduler session: {error}"))?;
            transaction
                .execute(
                    "UPDATE scheduler_owners SET last_seen_at = ?2 WHERE owner_id = ?1",
                    params![to_i64(fence.owner_id)?, to_i64(now)?],
                )
                .map_err(|error| format!("Failed to heartbeat scheduler owner: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler renewal: {error}"))?;
        Ok(updated == 1)
    }

    pub(crate) fn cancel(&self, id: TaskSessionId) -> Result<CancelResult, String> {
        self.cancel_at(id, now_millis())
    }

    fn cancel_at(&self, id: TaskSessionId, now: u64) -> Result<CancelResult, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler cancellation: {error}"))?;
        let (state, active_attempt_id) = transaction
            .query_row(
                "SELECT state, active_attempt_id FROM scheduler_task_sessions WHERE session_id = ?1",
                params![to_i64(id.0)?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .map_err(|error| format!("Failed to read scheduler session state: {error}"))?
            .ok_or_else(|| format!("Task session {} was not found.", id.0))?;
        let changed = match state.as_str() {
            "queued" => {
                transaction
                    .execute(
                        "UPDATE scheduler_task_sessions
                        SET state = 'cancelled', completed_at = ?2
                      WHERE session_id = ?1 AND state = 'queued'",
                        params![to_i64(id.0)?, to_i64(now)?],
                    )
                    .map_err(|error| format!("Failed to cancel queued session: {error}"))?
                    == 1
            }
            "running" => {
                transaction
                    .execute(
                        "UPDATE scheduler_task_sessions SET state = 'cancelling'
                      WHERE session_id = ?1 AND state = 'running'",
                        params![to_i64(id.0)?],
                    )
                    .map_err(|error| format!("Failed to request running cancellation: {error}"))?
                    == 1
            }
            _ => false,
        };
        if changed {
            if let Some(active_attempt_id) = active_attempt_id {
                revoke_subtask_authorities_for_parent_on(
                    &transaction,
                    from_i64(active_attempt_id, "active task attempt ID")?,
                    now,
                    "parent_cancelled",
                )?;
            }
            let next_state = if state == "queued" {
                "cancelled"
            } else {
                "cancelling"
            };
            append_event_in_transaction(
                &transaction,
                id,
                None,
                0,
                &TaskSessionEventInput {
                    kind: TaskSessionEventKind::Lifecycle,
                    payload: json!({ "state": next_state }),
                    progress: Some(TaskProgress {
                        phase: next_state.to_string(),
                        completed: 0,
                        total: None,
                    }),
                },
                now,
            )?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler cancellation: {error}"))?;
        drop(connection);
        let snapshot = self
            .get_session(id)?
            .ok_or_else(|| format!("Task session {} was not found after cancellation.", id.0))?;
        Ok(CancelResult { changed, snapshot })
    }

    /// Atomically validates assignment authority and resolves cancellation versus worker outcome.
    ///
    /// Structured successful output is staged in the durable projection outbox. All other output
    /// terminalizes immediately. A concurrent cancellation always wins and becomes `cancelled`.
    pub(crate) fn resolve_assignment(
        &self,
        fence: AssignmentFence,
        outcome: DurableOutcome,
    ) -> Result<FinishResult, String> {
        #[cfg(test)]
        if self
            .resolution_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err("Injected durable assignment resolution failure.".to_string());
        }
        self.resolve_assignment_at(fence, outcome, now_millis())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_resolutions(&self, count: usize) {
        self.resolution_failures.store(count, Ordering::SeqCst);
    }

    fn resolve_assignment_at(
        &self,
        fence: AssignmentFence,
        outcome: DurableOutcome,
        now: u64,
    ) -> Result<FinishResult, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler completion: {error}"))?;
        let session = transaction
            .query_row(
                "SELECT state, workspace_id, conversation_id, execution_run_id
                   FROM scheduler_task_sessions
                   WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3
                     AND state IN ('running', 'cancelling') AND lease_expires_at > ?4",
                params![
                    to_i64(fence.session_id.0)?,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(now)?
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to validate scheduler fence: {error}"))?;
        let Some((session_state, workspace_id, conversation_id, execution_run_id)) = session else {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit stale scheduler finish: {error}"))?;
            return Ok(FinishResult::Stale);
        };
        let attempt_matches = transaction
            .query_row(
                "SELECT 1 FROM scheduler_task_attempts
                  WHERE attempt_id = ?1 AND session_id = ?2 AND attempt_number = ?3
                    AND owner_id = ?4 AND fencing_token = ?5 AND state = 'running'
                    AND lease_expires_at > ?6",
                params![
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.session_id.0)?,
                    i64::from(fence.attempt),
                    to_i64(fence.owner_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(now)?
                ],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to validate scheduler attempt: {error}"))?
            .is_some();
        if !attempt_matches {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit stale scheduler attempt: {error}"))?;
            return Ok(FinishResult::Stale);
        }

        mark_attempt_resource_mutations_uncertain(
            &transaction,
            fence.attempt_id,
            now,
            "assignment_finished_without_resolution",
        )?;
        let parent_cancelled =
            session_state == "cancelling" || matches!(&outcome, DurableOutcome::Cancelled);
        revoke_subtask_authorities_for_parent_on(
            &transaction,
            fence.attempt_id,
            now,
            if parent_cancelled {
                "parent_cancelled"
            } else {
                "parent_resolved"
            },
        )?;

        if session_state == "cancelling" {
            terminalize_assignment(&transaction, fence, "cancelled", None, now)?;
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit scheduler cancellation: {error}"))?;
            return Ok(FinishResult::Applied);
        }

        let DurableOutcome::Succeeded(output) = outcome else {
            let (state, error) = match outcome {
                DurableOutcome::Failed(error) => ("failed", Some(error)),
                DurableOutcome::Blocked(error) => ("blocked", Some(error)),
                DurableOutcome::Cancelled => ("cancelled", None),
                DurableOutcome::Succeeded(_) => unreachable!(),
            };
            terminalize_assignment(&transaction, fence, state, error.as_deref(), now)?;
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit scheduler completion: {error}"))?;
            return Ok(FinishResult::Applied);
        };
        if matches!(output, TaskExecutionOutput::None) {
            terminalize_assignment(&transaction, fence, "succeeded", None, now)?;
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit scheduler completion: {error}"))?;
            return Ok(FinishResult::Applied);
        }

        let terminal_state = match &output {
            TaskExecutionOutput::Agent(result) => match result.completion_status {
                AgentTaskCompletionStatus::Completed => TaskSessionState::Succeeded,
                AgentTaskCompletionStatus::Blocked => TaskSessionState::Blocked,
            },
            TaskExecutionOutput::Chat(_) | TaskExecutionOutput::Edit(_) => {
                TaskSessionState::Succeeded
            }
            TaskExecutionOutput::None => unreachable!(),
        };
        let terminal_state_text = state_text(terminal_state);
        let output_json = serde_json::to_string(&output)
            .map_err(|error| format!("Failed to encode staged task result: {error}"))?;
        let workspace_id = workspace_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Task completion requires workspace ownership.".to_string())?;
        let conversation_id = conversation_id.filter(|value| !value.trim().is_empty());
        let execution_run_id = execution_run_id.filter(|value| !value.trim().is_empty());
        match &output {
            TaskExecutionOutput::Agent(_) => {
                if conversation_id.is_none() || execution_run_id.is_none() {
                    return Err(
                        "Agent completion requires conversation and execution run ownership."
                            .to_string(),
                    );
                }
            }
            TaskExecutionOutput::Chat(result) => {
                if conversation_id.as_deref() != Some(result.conversation_id.as_str()) {
                    return Err("Chat result does not match conversation ownership.".to_string());
                }
            }
            TaskExecutionOutput::Edit(_) => {}
            TaskExecutionOutput::None => unreachable!(),
        }
        let projection_id = format!(
            "task-session:{}:{}:{}:{}",
            self.instance_id, fence.session_id.0, fence.attempt_id, fence.fencing_token
        );
        transaction
            .execute(
                "INSERT INTO scheduler_task_completions
                   (session_id, projection_id, attempt_id, fencing_token, workspace_id,
                     conversation_id, execution_run_id, terminal_state, output_json, staged_at,
                     next_projection_at)
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    to_i64(fence.session_id.0)?,
                    projection_id,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?,
                    workspace_id,
                    conversation_id.unwrap_or_default(),
                    execution_run_id.unwrap_or_default(),
                    terminal_state_text,
                    output_json,
                    to_i64(now)?
                ],
            )
            .map_err(|error| format!("Failed to stage scheduler completion: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = 'committing', lease_expires_at = NULL
                  WHERE attempt_id = ?1 AND state = 'running'",
                params![to_i64(fence.attempt_id)?],
            )
            .map_err(|error| format!("Failed to stage scheduler attempt: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = 'committing', lease_expires_at = NULL, error = NULL
                  WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3",
                params![
                    to_i64(fence.session_id.0)?,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?
                ],
            )
            .map_err(|error| format!("Failed to stage scheduler session: {error}"))?;
        append_event_in_transaction(
            &transaction,
            fence.session_id,
            Some(fence.attempt_id),
            fence.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({ "state": "committing" }),
                progress: Some(TaskProgress {
                    phase: "committing".to_string(),
                    completed: 0,
                    total: Some(1),
                }),
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler result staging: {error}"))?;
        Ok(FinishResult::Applied)
    }

    /// Returns unprojected completions whose durable retry timestamp is due.
    pub(crate) fn due_pending_completions(
        &self,
        now: u64,
    ) -> Result<Vec<StagedCompletion>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT completions.projection_id, completions.session_id,
                        completions.attempt_id, completions.fencing_token,
                        completions.workspace_id, completions.conversation_id,
                        completions.execution_run_id, completions.terminal_state,
                        completions.output_json, sessions.payload
                   FROM scheduler_task_completions completions
                   JOIN scheduler_task_sessions sessions
                     ON sessions.session_id = completions.session_id
                  WHERE completions.finalized_at IS NULL
                    AND completions.projected_at IS NULL
                    AND completions.next_projection_at <= ?1
                  ORDER BY completions.staged_at, completions.session_id",
            )
            .map_err(|error| format!("Failed to prepare pending completions: {error}"))?;
        let completions = statement
            .query_map(params![to_i64(now)?], staged_completion_from_row)
            .map_err(|error| format!("Failed to query pending completions: {error}"))?
            .map(|row| {
                row.map_err(|error| format!("Failed to decode pending completion: {error}"))?
                    .decode()
            })
            .collect();
        completions
    }

    /// Returns externally projected completions that still need idempotent scheduler finalization.
    pub(crate) fn projected_unfinalized_completions(
        &self,
    ) -> Result<Vec<StagedCompletion>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT completions.projection_id, completions.session_id,
                        completions.attempt_id, completions.fencing_token,
                        completions.workspace_id, completions.conversation_id,
                        completions.execution_run_id, completions.terminal_state,
                        completions.output_json, sessions.payload
                   FROM scheduler_task_completions completions
                   JOIN scheduler_task_sessions sessions
                     ON sessions.session_id = completions.session_id
                  WHERE completions.finalized_at IS NULL
                    AND completions.projected_at IS NOT NULL
                  ORDER BY completions.staged_at, completions.session_id",
            )
            .map_err(|error| format!("Failed to prepare projected completions: {error}"))?;
        let completions = statement
            .query_map([], staged_completion_from_row)
            .map_err(|error| format!("Failed to query projected completions: {error}"))?
            .map(|row| {
                row.map_err(|error| format!("Failed to decode projected completion: {error}"))?
                    .decode()
            })
            .collect();
        completions
    }

    pub(crate) fn pending_completion_count(&self) -> Result<usize, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT COUNT(*) FROM scheduler_task_completions WHERE finalized_at IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("Failed to count pending completions: {error}"))
            .and_then(|count| {
                usize::try_from(count).map_err(|_| "Invalid pending completion count.".to_string())
            })
    }

    pub fn task_session_result(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Option<TaskSessionResult>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT output_json, terminal_state, projection_error, projected_at, finalized_at
                   FROM scheduler_task_completions WHERE session_id = ?1",
                params![to_i64(session_id.0)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to query task session result: {error}"))?
            .map(
                |(output, terminal_state, projection_error, projected_at, finalized_at)| {
                    Ok(TaskSessionResult {
                        session_id,
                        output: serde_json::from_str(&output).map_err(|error| {
                            format!("Failed to decode task session result: {error}")
                        })?,
                        terminal_state: parse_state(&terminal_state)?,
                        projection_error,
                        projected_at: projected_at
                            .map(|value| from_i64(value, "completion projection timestamp"))
                            .transpose()?,
                        finalized_at: finalized_at
                            .map(|value| from_i64(value, "completion finalization timestamp"))
                            .transpose()?,
                    })
                },
            )
            .transpose()
    }

    pub(crate) fn objective_checkpoints(
        &self,
        session_id: TaskSessionId,
    ) -> Result<Vec<AgentTaskObjectiveCheckpoint>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT objective_id, evidence_json, tool_receipts_json,
                        source_attempt_id, recorded_at
                   FROM scheduler_task_objective_checkpoints
                  WHERE session_id = ?1
                  ORDER BY recorded_at, objective_id",
            )
            .map_err(|error| format!("Failed to prepare objective checkpoint query: {error}"))?;
        let checkpoints = statement
            .query_map(params![to_i64(session_id.0)?], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| format!("Failed to query objective checkpoints: {error}"))?
            .map(|row| {
                let (objective_id, evidence, tool_receipts, source_attempt_id, recorded_at) =
                    row.map_err(|error| format!("Failed to read objective checkpoint: {error}"))?;
                Ok(AgentTaskObjectiveCheckpoint {
                    objective_id,
                    evidence: serde_json::from_str(&evidence).map_err(|error| {
                        format!("Failed to decode objective checkpoint evidence: {error}")
                    })?,
                    tool_receipts: serde_json::from_str(&tool_receipts).map_err(|error| {
                        format!("Failed to decode objective checkpoint tool receipts: {error}")
                    })?,
                    source_attempt_id: from_i64(source_attempt_id, "checkpoint attempt ID")?,
                    recorded_at: from_i64(recorded_at, "checkpoint timestamp")?,
                })
            })
            .collect();
        checkpoints
    }

    pub(crate) fn record_completion_error(
        &self,
        completion: &StagedCompletion,
        error: &str,
    ) -> Result<bool, String> {
        self.record_completion_error_at(completion, error, now_millis())
    }

    fn record_completion_error_at(
        &self,
        completion: &StagedCompletion,
        error: &str,
        now: u64,
    ) -> Result<bool, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start completion error recording: {error}"))?;
        let attempt_count = transaction
            .query_row(
                "SELECT projection_attempt_count FROM scheduler_task_completions
                  WHERE session_id = ?1 AND attempt_id = ?2 AND fencing_token = ?3
                    AND finalized_at IS NULL",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to read completion projection attempt: {error}"))?;
        let Some(attempt_count) = attempt_count else {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit stale completion error: {error}"))?;
            return Ok(false);
        };
        let next_attempt = u32::try_from(attempt_count)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let delay = projection_retry_delay_millis(next_attempt);
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_completions
                    SET projection_error = ?4,
                        projection_attempt_count = projection_attempt_count + 1,
                        next_projection_at = ?5
                  WHERE session_id = ?1 AND attempt_id = ?2 AND fencing_token = ?3
                    AND finalized_at IS NULL",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?,
                    error,
                    to_i64(now.saturating_add(delay))?
                ],
            )
            .map_err(|error| format!("Failed to record completion projection error: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit completion projection error: {error}"))?;
        Ok(updated == 1)
    }

    /// Permanently rejects a deterministic projection conflict and releases Task Session ownership.
    pub(crate) fn reject_completion(
        &self,
        completion: &StagedCompletion,
        error: &str,
    ) -> Result<FinishResult, String> {
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start completion rejection: {error}"))?;
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = 'blocked', active_attempt_id = NULL, lease_expires_at = NULL,
                        completed_at = ?4, error = ?5
                  WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3
                    AND state = 'committing'",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?,
                    to_i64(now)?,
                    error,
                ],
            )
            .map_err(|error| format!("Failed to reject scheduler session: {error}"))?;
        if updated != 1 {
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit stale completion rejection: {error}"))?;
            return Ok(FinishResult::Stale);
        }
        transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = 'blocked', completed_at = ?2, error = ?3
                  WHERE attempt_id = ?1 AND state = 'committing'",
                params![to_i64(completion.attempt_id)?, to_i64(now)?, error],
            )
            .map_err(|error| format!("Failed to reject scheduler attempt: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_completions
                    SET terminal_state = 'blocked', projection_error = ?2,
                        projection_attempt_count = projection_attempt_count + 1,
                        finalized_at = ?3
                  WHERE session_id = ?1 AND attempt_id = ?4 AND fencing_token = ?5
                    AND finalized_at IS NULL",
                params![
                    to_i64(completion.session_id.0)?,
                    error,
                    to_i64(now)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?,
                ],
            )
            .map_err(|error| format!("Failed to finalize rejected completion: {error}"))?;
        append_event_in_transaction(
            &transaction,
            completion.session_id,
            Some(completion.attempt_id),
            completion.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({ "state": "blocked", "error": error, "action": "terminal" }),
                progress: Some(TaskProgress {
                    phase: "blocked".to_string(),
                    completed: 1,
                    total: Some(1),
                }),
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit completion rejection: {error}"))?;
        Ok(FinishResult::Applied)
    }

    pub(crate) fn mark_completion_projected(
        &self,
        completion: &StagedCompletion,
    ) -> Result<bool, String> {
        self.mark_completion_projected_at(completion, now_millis())
    }

    fn mark_completion_projected_at(
        &self,
        completion: &StagedCompletion,
        now: u64,
    ) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "UPDATE scheduler_task_completions
                    SET projected_at = COALESCE(projected_at, ?4), projection_error = NULL
                  WHERE session_id = ?1 AND attempt_id = ?2 AND fencing_token = ?3
                    AND finalized_at IS NULL",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?,
                    to_i64(now)?
                ],
            )
            .map(|updated| updated == 1)
            .map_err(|error| format!("Failed to mark completion projected: {error}"))
    }

    pub(crate) fn finalize_completion(
        &self,
        completion: &StagedCompletion,
    ) -> Result<FinishResult, String> {
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start completion finalization: {error}"))?;
        let projected = transaction
            .query_row(
                "SELECT 1 FROM scheduler_task_completions
                  WHERE session_id = ?1 AND attempt_id = ?2 AND fencing_token = ?3
                    AND projected_at IS NOT NULL AND finalized_at IS NULL",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?
                ],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to validate projected completion: {error}"))?
            .is_some();
        if !projected {
            transaction.commit().map_err(|error| {
                format!("Failed to commit stale completion finalization: {error}")
            })?;
            return Ok(FinishResult::Stale);
        }
        let state = state_text(completion.terminal_state);
        let error = match &completion.output {
            TaskExecutionOutput::Agent(result)
                if result.completion_status == AgentTaskCompletionStatus::Blocked =>
            {
                result
                    .blocked_reason
                    .as_deref()
                    .or(Some(result.summary.as_str()))
            }
            _ => None,
        };
        let updated = transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = ?4, active_attempt_id = NULL, completed_at = ?5, error = ?6
                  WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3
                    AND state = 'committing'",
                params![
                    to_i64(completion.session_id.0)?,
                    to_i64(completion.attempt_id)?,
                    to_i64(completion.fencing_token)?,
                    state,
                    to_i64(now)?,
                    error
                ],
            )
            .map_err(|error| format!("Failed to finalize scheduler session: {error}"))?;
        if updated != 1 {
            transaction.commit().map_err(|error| {
                format!("Failed to commit stale scheduler finalization: {error}")
            })?;
            return Ok(FinishResult::Stale);
        }
        transaction
            .execute(
                "UPDATE scheduler_task_attempts SET state = ?2, completed_at = ?3
                  WHERE attempt_id = ?1 AND state = 'committing'",
                params![to_i64(completion.attempt_id)?, state, to_i64(now)?],
            )
            .map_err(|error| format!("Failed to finalize scheduler attempt: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_completions SET finalized_at = ?2 WHERE session_id = ?1",
                params![to_i64(completion.session_id.0)?, to_i64(now)?],
            )
            .map_err(|error| format!("Failed to finalize scheduler completion record: {error}"))?;
        append_event_in_transaction(
            &transaction,
            completion.session_id,
            Some(completion.attempt_id),
            completion.fencing_token,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({ "state": state, "error": error }),
                progress: Some(TaskProgress {
                    phase: state.to_string(),
                    completed: 1,
                    total: Some(1),
                }),
            },
            now,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit completion finalization: {error}"))?;
        Ok(FinishResult::Applied)
    }

    pub(crate) fn recover_expired(&self) -> Result<usize, String> {
        self.recover_expired_sessions()
            .map(|sessions| sessions.len())
    }

    pub(crate) fn recover_expired_sessions(&self) -> Result<Vec<TaskSessionId>, String> {
        self.recover_expired_sessions_at(now_millis())
    }

    #[cfg(test)]
    fn recover_expired_at(&self, now: u64) -> Result<usize, String> {
        self.recover_expired_sessions_at(now)
            .map(|sessions| sessions.len())
    }

    fn recover_expired_sessions_at(&self, now: u64) -> Result<Vec<TaskSessionId>, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler recovery: {error}"))?;
        let recovered = recover_expired_in_transaction(&transaction, now)?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler recovery: {error}"))?;
        Ok(recovered)
    }

    pub(crate) fn abandon_owner(&self, owner_id: u64) -> Result<usize, String> {
        let now = now_millis();
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler owner cleanup: {error}"))?;
        let recovered = recover_matching_attempts(
            &transaction,
            "attempts.owner_id = ?1",
            params![to_i64(owner_id)?],
            now,
            "Scheduler owner shut down.",
            "scheduler_owner_shutdown",
        )?;
        recover_subtask_authorities_on(&transaction, now)?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler owner cleanup: {error}"))?;
        Ok(recovered.len())
    }

    pub(crate) fn remove_terminal(&self, id: TaskSessionId) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM scheduler_task_sessions
                  WHERE session_id = ?1 AND state IN ('succeeded', 'failed', 'blocked', 'cancelled')
                    AND NOT EXISTS (
                      SELECT 1 FROM scheduler_resource_mutations mutations
                       WHERE mutations.session_id = scheduler_task_sessions.session_id
                         AND mutations.state IN ('reserved', 'succeeded', 'uncertain')
                    )",
                params![to_i64(id.0)?],
            )
            .map(|updated| updated == 1)
            .map_err(|error| format!("Failed to remove terminal task session: {error}"))
    }
}

fn bind_prepared_subtasks_on(
    transaction: &Transaction<'_>,
    fence: AssignmentFence,
    contracts: &[PreparedSubtaskContract],
    now: u64,
) -> Result<(), String> {
    let existing_count: u64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM scheduler_prepared_subtasks WHERE session_id = ?1",
            params![to_i64(fence.session_id.0)?],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to inspect prepared subtask records: {error}"))?;
    if existing_count > 0 && existing_count != contracts.len() as u64 {
        return Err("Prepared subtask set changed across assignment attempts.".to_string());
    }
    for contract in contracts {
        let contract_json = serde_json::to_string(contract)
            .map_err(|error| format!("Failed to encode prepared subtask contract: {error}"))?;
        let existing = transaction
            .query_row(
                "SELECT subtask_id, contract_json
                   FROM scheduler_prepared_subtasks
                  WHERE session_id = ?1 AND objective_id = ?2",
                params![to_i64(fence.session_id.0)?, contract.objective_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| format!("Failed to inspect prepared subtask identity: {error}"))?;
        let subtask_id = match existing {
            Some((subtask_id, existing)) if existing == contract_json => subtask_id,
            Some(_) => {
                return Err(
                    "Prepared subtask contract changed across assignment attempts.".to_string(),
                )
            }
            None if existing_count > 0 => {
                return Err(
                    "Prepared subtask objective changed across assignment attempts.".to_string(),
                )
            }
            None => {
                transaction
                    .execute(
                        "INSERT INTO scheduler_prepared_subtasks
                           (session_id, contract_id, objective_id, contract_json, state,
                            execution_enabled, created_from_attempt_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, 'prepared', 0, ?5, ?6)",
                        params![
                            to_i64(fence.session_id.0)?,
                            contract.contract_id,
                            contract.objective_id,
                            contract_json,
                            to_i64(fence.attempt_id)?,
                            to_i64(now)?
                        ],
                    )
                    .map_err(|error| format!("Failed to persist prepared subtask: {error}"))?;
                transaction.last_insert_rowid()
            }
        };
        let attempt_count: u64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM scheduler_subtask_attempts WHERE subtask_id = ?1",
                params![subtask_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to inspect dormant subtask attempt: {error}"))?;
        if attempt_count == 0 {
            transaction
                .execute(
                    "INSERT INTO scheduler_subtask_attempts
                       (subtask_id, attempt_number, fencing_token, state,
                        wall_clock_seconds, max_tool_calls, max_mutation_calls,
                        tool_calls_used, mutation_calls_used, authority_active, created_at)
                     VALUES (?1, 1, 1, 'dormant', ?2, ?3, ?4, 0, 0, 0, ?5)",
                    params![
                        subtask_id,
                        to_i64(contract.budget.wall_clock_seconds)?,
                        i64::from(contract.budget.max_tool_calls),
                        i64::from(contract.budget.max_mutation_calls),
                        to_i64(now)?
                    ],
                )
                .map_err(|error| format!("Failed to persist dormant subtask attempt: {error}"))?;
        } else if attempt_count != 1 {
            return Err("Prepared subtask has an invalid dormant attempt count.".to_string());
        } else {
            let stored_attempt = transaction
                .query_row(
                    "SELECT state, wall_clock_seconds, max_tool_calls, max_mutation_calls,
                            tool_calls_used, mutation_calls_used, authority_active
                       FROM scheduler_subtask_attempts WHERE subtask_id = ?1",
                    params![subtask_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .map_err(|error| format!("Failed to validate dormant subtask attempt: {error}"))?;
            if stored_attempt
                != (
                    "dormant".to_string(),
                    to_i64(contract.budget.wall_clock_seconds)?,
                    i64::from(contract.budget.max_tool_calls),
                    i64::from(contract.budget.max_mutation_calls),
                    0,
                    0,
                    0,
                )
            {
                return Err("Prepared subtask dormant allocation changed unexpectedly.".to_string());
            }
        }
    }
    Ok(())
}

const SESSION_COLUMNS: &str =
    "session_id, label, payload, state, worker_id, dispatch_sequence, attempt_count,
     COALESCE(active_attempt_id, (SELECT MAX(attempt_id) FROM scheduler_task_attempts
       WHERE scheduler_task_attempts.session_id = scheduler_task_sessions.session_id)),
     fencing_token, lease_expires_at, error, created_at, started_at,
     completed_at, progress_phase, progress_completed, progress_total, next_event_sequence,
     opencode_session_id";
const SESSION_SELECT_ALL: &str =
    "SELECT session_id, label, payload, state, worker_id, dispatch_sequence, attempt_count,
            COALESCE(active_attempt_id, (SELECT MAX(attempt_id) FROM scheduler_task_attempts
              WHERE scheduler_task_attempts.session_id = scheduler_task_sessions.session_id)),
            fencing_token, lease_expires_at, error, created_at, started_at,
            completed_at, progress_phase, progress_completed, progress_total, next_event_sequence,
            opencode_session_id
       FROM scheduler_task_sessions ORDER BY session_id";

struct StoredSession {
    id: i64,
    label: String,
    payload: String,
    state: String,
    worker_id: Option<i64>,
    dispatch_sequence: Option<i64>,
    attempt: i64,
    attempt_id: Option<i64>,
    fencing_token: i64,
    lease_expires_at: Option<i64>,
    error: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    progress_phase: Option<String>,
    progress_completed: Option<i64>,
    progress_total: Option<i64>,
    next_event_sequence: i64,
    opencode_session_id: Option<String>,
}

struct StoredCompletion {
    projection_id: String,
    session_id: i64,
    attempt_id: i64,
    fencing_token: i64,
    workspace_id: String,
    conversation_id: String,
    execution_run_id: String,
    terminal_state: String,
    output_json: String,
    session_payload: String,
}

impl StoredCompletion {
    fn decode(self) -> Result<StagedCompletion, String> {
        let output: TaskExecutionOutput = serde_json::from_str(&self.output_json)
            .map_err(|error| format!("Failed to decode staged task result: {error}"))?;
        let chat_head = if matches!(output, TaskExecutionOutput::Chat(_)) {
            let request = TaskRequest::with_payload("staged-chat", self.session_payload);
            match request.envelope()? {
                Some(TaskSessionEnvelope::V2(envelope)) => match envelope.prompt_input {
                    TaskSessionInputV2::Chat(input) => Some(StagedChatHead {
                        message_id: input.message_id,
                        message_sequence: input.message_sequence,
                        message: input.message,
                    }),
                    TaskSessionInputV2::Edit(_) => {
                        return Err("Chat completion has an Edit Task Session envelope.".to_string())
                    }
                },
                _ => return Err("Chat completion requires a V2 Task Session envelope.".to_string()),
            }
        } else {
            None
        };
        Ok(StagedCompletion {
            projection_id: self.projection_id,
            session_id: TaskSessionId(from_i64(self.session_id, "task session ID")?),
            attempt_id: from_i64(self.attempt_id, "task attempt ID")?,
            fencing_token: from_i64(self.fencing_token, "fencing token")?,
            workspace_id: self.workspace_id,
            conversation_id: self.conversation_id,
            execution_run_id: self.execution_run_id,
            output,
            terminal_state: parse_state(&self.terminal_state)?,
            chat_head,
        })
    }
}

fn staged_completion_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCompletion> {
    Ok(StoredCompletion {
        projection_id: row.get(0)?,
        session_id: row.get(1)?,
        attempt_id: row.get(2)?,
        fencing_token: row.get(3)?,
        workspace_id: row.get(4)?,
        conversation_id: row.get(5)?,
        execution_run_id: row.get(6)?,
        terminal_state: row.get(7)?,
        output_json: row.get(8)?,
        session_payload: row.get(9)?,
    })
}

fn terminalize_assignment(
    transaction: &Transaction<'_>,
    fence: AssignmentFence,
    state: &str,
    error: Option<&str>,
    now: u64,
) -> Result<(), String> {
    let opencode_session_id = transaction
        .query_row(
            "SELECT opencode_session_id FROM scheduler_task_sessions WHERE session_id = ?1",
            params![to_i64(fence.session_id.0)?],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| format!("Failed to load terminal OpenCode identity: {error}"))?;
    let action =
        if state == "blocked" && error.is_some_and(|error| error.contains("[approval_required]")) {
            "paused"
        } else {
            "terminal"
        };
    transaction
        .execute(
            "UPDATE scheduler_task_attempts
                SET state = ?2, lease_expires_at = NULL, completed_at = ?3, error = ?4
              WHERE attempt_id = ?1 AND state = 'running'",
            params![to_i64(fence.attempt_id)?, state, to_i64(now)?, error],
        )
        .map_err(|error| format!("Failed to finish scheduler attempt: {error}"))?;
    transaction
        .execute(
            "UPDATE scheduler_task_sessions
                SET state = ?2, active_attempt_id = NULL, lease_expires_at = NULL,
                    completed_at = ?3, error = ?4
              WHERE session_id = ?1 AND active_attempt_id = ?5 AND fencing_token = ?6",
            params![
                to_i64(fence.session_id.0)?,
                state,
                to_i64(now)?,
                error,
                to_i64(fence.attempt_id)?,
                to_i64(fence.fencing_token)?
            ],
        )
        .map_err(|error| format!("Failed to finish scheduler session: {error}"))?;
    append_event_in_transaction(
        transaction,
        fence.session_id,
        Some(fence.attempt_id),
        fence.fencing_token,
        &TaskSessionEventInput {
            kind: TaskSessionEventKind::Lifecycle,
            payload: json!({
                "state": state,
                "error": error,
                "action": action,
                "task_session_id": fence.session_id.0,
                "opencode_session_id": opencode_session_id,
            }),
            progress: Some(TaskProgress {
                phase: state.to_string(),
                completed: 1,
                total: Some(1),
            }),
        },
        now,
    )?;
    Ok(())
}

fn projection_retry_delay_millis(attempt: u32) -> u64 {
    const BASE_MILLIS: u64 = 100;
    const CAP_MILLIS: u64 = 30_000;
    BASE_MILLIS
        .saturating_mul(
            1_u64
                .checked_shl(attempt.saturating_sub(1).min(63))
                .unwrap_or(u64::MAX),
        )
        .min(CAP_MILLIS)
}

impl StoredSession {
    fn into_snapshot(self) -> Result<TaskSessionSnapshot, String> {
        Ok(TaskSessionSnapshot {
            id: TaskSessionId(from_i64(self.id, "task session ID")?),
            request: TaskRequest {
                label: self.label,
                payload: self.payload,
            },
            state: parse_state(&self.state)?,
            worker_id: self
                .worker_id
                .map(|value| usize::try_from(value).map_err(|_| "Invalid Worker ID.".to_string()))
                .transpose()?,
            dispatch_sequence: self
                .dispatch_sequence
                .map(|value| from_i64(value, "dispatch sequence"))
                .transpose()?,
            attempt: u32::try_from(self.attempt)
                .map_err(|_| "Invalid task attempt count.".to_string())?,
            attempt_id: self
                .attempt_id
                .map(|value| from_i64(value, "task attempt ID"))
                .transpose()?,
            fencing_token: from_i64(self.fencing_token, "fencing token")?,
            opencode_session_id: self.opencode_session_id,
            lease_expires_at: self
                .lease_expires_at
                .map(|value| from_i64(value, "lease expiry"))
                .transpose()?,
            error: self.error,
            created_at: from_i64(self.created_at, "task creation timestamp")?,
            started_at: self
                .started_at
                .map(|value| from_i64(value, "task start timestamp"))
                .transpose()?,
            completed_at: self
                .completed_at
                .map(|value| from_i64(value, "task completion timestamp"))
                .transpose()?,
            progress: self
                .progress_phase
                .map(|phase| -> Result<TaskProgress, String> {
                    Ok(TaskProgress {
                        phase,
                        completed: from_i64(
                            self.progress_completed.unwrap_or_default(),
                            "task progress",
                        )?,
                        total: self
                            .progress_total
                            .map(|value| from_i64(value, "task progress total"))
                            .transpose()?,
                    })
                })
                .transpose()?,
            last_event_sequence: from_i64(
                self.next_event_sequence.saturating_sub(1),
                "last event sequence",
            )?,
        })
    }
}

fn stored_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSession> {
    Ok(StoredSession {
        id: row.get(0)?,
        label: row.get(1)?,
        payload: row.get(2)?,
        state: row.get(3)?,
        worker_id: row.get(4)?,
        dispatch_sequence: row.get(5)?,
        attempt: row.get(6)?,
        attempt_id: row.get(7)?,
        fencing_token: row.get(8)?,
        lease_expires_at: row.get(9)?,
        error: row.get(10)?,
        created_at: row.get(11)?,
        started_at: row.get(12)?,
        completed_at: row.get(13)?,
        progress_phase: row.get(14)?,
        progress_completed: row.get(15)?,
        progress_total: row.get(16)?,
        next_event_sequence: row.get(17)?,
        opencode_session_id: row.get(18)?,
    })
}

fn load_session(
    connection: &Connection,
    id: TaskSessionId,
) -> Result<Option<TaskSessionSnapshot>, String> {
    let sql =
        format!("SELECT {SESSION_COLUMNS} FROM scheduler_task_sessions WHERE session_id = ?1");
    connection
        .query_row(&sql, params![to_i64(id.0)?], stored_session_from_row)
        .optional()
        .map_err(|error| format!("Failed to load scheduler session: {error}"))?
        .map(StoredSession::into_snapshot)
        .transpose()
}

fn load_capability_grants(
    connection: &Connection,
    session_id: TaskSessionId,
) -> Result<Vec<TaskCapabilityGrant>, String> {
    let mut statement = connection
        .prepare(
            "SELECT capability, grant_source, granted_at
               FROM scheduler_task_grants
              WHERE session_id = ?1
              ORDER BY capability",
        )
        .map_err(|error| format!("Failed to prepare task capability grants: {error}"))?;
    let rows = statement
        .query_map(params![to_i64(session_id.0)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("Failed to query task capability grants: {error}"))?;
    rows.map(|row| {
        let (capability, grant_source, granted_at) =
            row.map_err(|error| format!("Failed to decode task capability grant: {error}"))?;
        Ok(TaskCapabilityGrant {
            capability,
            grant_source,
            granted_at: from_i64(granted_at, "task capability grant timestamp")?,
        })
    })
    .collect()
}

fn assignment_is_current_on(
    connection: &Connection,
    fence: AssignmentFence,
    now: u64,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT 1
               FROM scheduler_task_sessions sessions
               JOIN scheduler_task_attempts attempts
                 ON attempts.attempt_id = sessions.active_attempt_id
              WHERE sessions.session_id = ?1
                AND sessions.state = 'running'
                AND sessions.active_attempt_id = ?2
                AND sessions.fencing_token = ?3
                AND sessions.lease_expires_at > ?4
                AND attempts.session_id = sessions.session_id
                AND attempts.attempt_number = ?5
                AND attempts.owner_id = ?6
                AND attempts.fencing_token = ?3
                AND attempts.state = 'running'
                AND attempts.lease_expires_at > ?4",
            params![
                to_i64(fence.session_id.0)?,
                to_i64(fence.attempt_id)?,
                to_i64(fence.fencing_token)?,
                to_i64(now)?,
                i64::from(fence.attempt),
                to_i64(fence.owner_id)?
            ],
            |_| Ok(()),
        )
        .optional()
        .map(|current| current.is_some())
        .map_err(|error| format!("Failed to validate assignment authority: {error}"))
}

type RawResourceMutation = (
    i64,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

fn decode_resource_mutation_row(row: &Row<'_>) -> rusqlite::Result<RawResourceMutation> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
        row.get(20)?,
    ))
}

fn decode_resource_mutation(raw: RawResourceMutation) -> Result<ResourceMutationRecord, String> {
    let (
        mutation_id,
        operation_key,
        identity_json,
        connector_id,
        tool_name,
        state,
        session_id,
        attempt_id,
        attempt,
        fencing_token,
        evidence_json,
        failure_kind,
        failure_code,
        revision,
        reserved_at,
        resolved_at,
        superseded_at,
        supersede_reason,
        checkpoint_objective_id,
        checkpoint_tool_call_id,
        checkpoint_recorded_at,
    ) = raw;
    let identity: ResourceOperationIdentity = serde_json::from_str(&identity_json)
        .map_err(|error| format!("Stored resource operation identity is invalid: {error}"))?;
    identity.validate()?;
    let evidence = evidence_json
        .map(|value| {
            serde_json::from_str::<ResourceMutationEvidence>(&value)
                .map_err(|error| format!("Stored resource mutation evidence is invalid: {error}"))
        })
        .transpose()?;
    if let Some(evidence) = &evidence {
        evidence.validate()?;
        if evidence.identity != identity {
            return Err("Stored resource mutation evidence identity does not match.".to_string());
        }
    }
    Ok(ResourceMutationRecord {
        mutation_id: from_i64(mutation_id, "resource mutation ID")?,
        operation_key,
        identity,
        connector_id,
        tool_name,
        state: ResourceMutationState::parse(&state)?,
        session_id: TaskSessionId(from_i64(session_id, "resource mutation session ID")?),
        attempt_id: from_i64(attempt_id, "resource mutation attempt ID")?,
        attempt: u32::try_from(attempt)
            .map_err(|_| "Stored resource mutation attempt number is invalid.".to_string())?,
        fencing_token: from_i64(fencing_token, "resource mutation fencing token")?,
        evidence,
        failure_kind,
        failure_code,
        revision: from_i64(revision, "resource mutation revision")?,
        reserved_at: from_i64(reserved_at, "resource mutation reservation timestamp")?,
        resolved_at: resolved_at
            .map(|value| from_i64(value, "resource mutation resolution timestamp"))
            .transpose()?,
        superseded_at: superseded_at
            .map(|value| from_i64(value, "resource mutation supersede timestamp"))
            .transpose()?,
        supersede_reason,
        checkpoint_objective_id,
        checkpoint_tool_call_id,
        checkpoint_recorded_at: checkpoint_recorded_at
            .map(|value| from_i64(value, "resource mutation checkpoint timestamp"))
            .transpose()?,
    })
}

fn resource_mutation_on(
    connection: &Connection,
    mutation_id: u64,
) -> Result<Option<ResourceMutationRecord>, String> {
    connection
        .query_row(
            "SELECT mutation_id, operation_key, identity_json, connector_id, tool_name,
                    state, session_id, attempt_id, attempt_number, fencing_token,
                    evidence_json, failure_kind, failure_code, revision, reserved_at,
                    resolved_at, superseded_at, supersede_reason,
                    checkpoint_objective_id, checkpoint_tool_call_id,
                    checkpoint_recorded_at
               FROM scheduler_resource_mutations WHERE mutation_id = ?1",
            params![to_i64(mutation_id)?],
            decode_resource_mutation_row,
        )
        .optional()
        .map_err(|error| format!("Failed to read resource mutation: {error}"))?
        .map(decode_resource_mutation)
        .transpose()
}

fn resource_mutation_by_active_key_on(
    connection: &Connection,
    operation_key: &str,
) -> Result<Option<ResourceMutationRecord>, String> {
    connection
        .query_row(
            "SELECT mutation_id, operation_key, identity_json, connector_id, tool_name,
                    state, session_id, attempt_id, attempt_number, fencing_token,
                    evidence_json, failure_kind, failure_code, revision, reserved_at,
                    resolved_at, superseded_at, supersede_reason,
                    checkpoint_objective_id, checkpoint_tool_call_id,
                    checkpoint_recorded_at
               FROM scheduler_resource_mutations
              WHERE operation_key = ?1 AND state IN ('reserved', 'succeeded', 'uncertain')
              ORDER BY mutation_id DESC LIMIT 1",
            params![operation_key],
            decode_resource_mutation_row,
        )
        .optional()
        .map_err(|error| format!("Failed to look up active resource mutation: {error}"))?
        .map(decode_resource_mutation)
        .transpose()
}

fn bind_resource_mutation_checkpoint_on(
    connection: &Connection,
    fence: AssignmentFence,
    objective_id: &str,
    receipt: &AgentTaskObjectiveToolReceipt,
    operation_key: &str,
    recorded_at: u64,
) -> Result<(), String> {
    let record =
        resource_mutation_by_active_key_on(connection, operation_key)?.ok_or_else(|| {
            "Objective checkpoint resource mutation receipt has no active ledger record."
                .to_string()
        })?;
    let current_attempt = record.attempt_id == fence.attempt_id
        && record.attempt == fence.attempt
        && record.fencing_token == fence.fencing_token;
    let jira_replay_adoption = record.identity.connector == "jira"
        && record.identity.operation == "add_comment"
        && crate::infrastructure::jira::trusted_jira_comment_tool(&record.tool_name);
    if record.state != ResourceMutationState::Succeeded
        || record.session_id != fence.session_id
        || (!current_attempt && !jira_replay_adoption)
        || record.tool_name != receipt.tool_name
    {
        return Err(
            "Objective checkpoint resource mutation receipt did not match a succeeded ledger record."
                .to_string(),
        );
    }
    match (
        record.checkpoint_objective_id.as_deref(),
        record.checkpoint_tool_call_id.as_deref(),
        record.checkpoint_recorded_at,
    ) {
        (None, None, None) => {}
        (Some(bound_objective), Some(bound_tool_call), Some(_))
            if bound_objective == objective_id && bound_tool_call == receipt.tool_call_id =>
        {
            return Ok(());
        }
        _ => {
            return Err(
                "Resource mutation ledger record is already bound to a different checkpoint receipt."
                    .to_string(),
            );
        }
    }
    let updated = connection
        .execute(
            "UPDATE scheduler_resource_mutations
                SET checkpoint_objective_id = ?2, checkpoint_tool_call_id = ?3,
                    checkpoint_recorded_at = ?4, revision = revision + 1
              WHERE mutation_id = ?1 AND operation_key = ?5 AND state = 'succeeded'
                AND session_id = ?6 AND tool_name = ?7
                AND checkpoint_objective_id IS NULL
                AND checkpoint_tool_call_id IS NULL
                AND checkpoint_recorded_at IS NULL",
            params![
                to_i64(record.mutation_id)?,
                objective_id,
                receipt.tool_call_id,
                to_i64(recorded_at)?,
                operation_key,
                to_i64(fence.session_id.0)?,
                receipt.tool_name,
            ],
        )
        .map_err(|error| format!("Failed to bind resource mutation checkpoint: {error}"))?;
    if updated != 1 {
        return Err("Resource mutation checkpoint binding lost its ledger fence.".to_string());
    }
    Ok(())
}

fn validate_external_authority_shape(
    authority: &ExternalAssignmentAuthority,
) -> Result<(), String> {
    if authority.connector_id.trim().is_empty()
        || authority.connector_id != authority.connector_id.trim()
        || authority.capability != format!("external_tools:{}", authority.connector_id)
        || authority.connector_binding_digest.len() != 64
        || !authority
            .connector_binding_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Resource mutation external authority is invalid.".to_string());
    }
    Ok(())
}

fn validate_subtask_authority_shape(
    authority: &SubtaskToolAuthority,
    capability: &str,
) -> Result<(), String> {
    if authority.scheduler_instance_id.trim().is_empty()
        || authority.objective_id.trim().is_empty()
        || authority.objective_id != authority.objective_id.trim()
        || capability.trim().is_empty()
        || capability != capability.trim()
        || authority.capabilities.is_empty()
        || authority.capabilities.len() > 64
        || authority
            .capabilities
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || !authority
            .capabilities
            .iter()
            .any(|granted| granted == capability)
        || authority
            .allowed_connector_tools
            .iter()
            .any(|(capability, tools)| {
                !capability.starts_with("external_tools:")
                    || !authority.capabilities.contains(capability)
                    || tools.is_empty()
                    || tools.len() > 64
                    || tools.windows(2).any(|pair| pair[0] >= pair[1])
                    || tools.iter().any(|tool| {
                        tool.trim().is_empty()
                            || tool != tool.trim()
                            || tool.len() > 128
                            || tool.contains("..")
                    })
            })
        || authority.capabilities.iter().any(|capability| {
            capability.starts_with("external_tools:")
                && !authority.allowed_connector_tools.contains_key(capability)
        })
    {
        return Err("Subtask tool authority shape or capability is invalid.".to_string());
    }
    Ok(())
}

fn validate_subtask_tool_operation(
    authority: &SubtaskToolAuthority,
    capability: &str,
    tool_name: Option<&str>,
) -> Result<(), String> {
    if capability.starts_with("external_tools:") {
        let tool_name = tool_name
            .ok_or_else(|| "Subtask connector tool operation identity is required.".to_string())?;
        if !authority
            .allowed_connector_tools
            .get(capability)
            .is_some_and(|tools| tools.iter().any(|tool| tool == tool_name))
        {
            return Err(
                "Subtask connector tool operation is not granted by its objective contract."
                    .to_string(),
            );
        }
    } else if tool_name.is_some() {
        return Err("Built-in subtask admission cannot carry a connector tool name.".to_string());
    }
    Ok(())
}

fn validate_subtask_contract_and_grants_on(
    connection: &Connection,
    authority: &SubtaskToolAuthority,
) -> Result<(), String> {
    let contract_json = connection
        .query_row(
            "SELECT subtasks.contract_json
               FROM scheduler_prepared_subtasks subtasks
               JOIN scheduler_subtask_attempts attempts
                 ON attempts.subtask_id = subtasks.subtask_id
              WHERE subtasks.session_id = ?1 AND subtasks.subtask_id = ?2
                AND subtasks.objective_id = ?3
                AND attempts.subtask_attempt_id = ?4
                AND attempts.attempt_number = ?5
                AND attempts.fencing_token = ?6",
            params![
                to_i64(authority.session_id.0)?,
                to_i64(authority.subtask_id)?,
                authority.objective_id,
                to_i64(authority.subtask_attempt_id)?,
                i64::from(authority.subtask_attempt),
                to_i64(authority.subtask_fencing_token)?
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to validate subtask contract identity: {error}"))?
        .ok_or_else(|| "Subtask contract identity is stale or incompatible.".to_string())?;
    let contract = serde_json::from_str::<PreparedSubtaskContract>(&contract_json)
        .map_err(|error| format!("Failed to decode subtask lifecycle contract: {error}"))?;
    if contract.schema_version != 2
        || contract.objective_id != authority.objective_id
        || contract.granted_capabilities != authority.capabilities
        || contract.allowed_connector_tools != authority.allowed_connector_tools
    {
        return Err("Subtask lifecycle authority changed its immutable contract.".to_string());
    }
    for capability in &authority.capabilities {
        let still_granted = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM scheduler_task_grants
                    WHERE session_id = ?1 AND capability = ?2
                 )",
                params![to_i64(authority.session_id.0)?, capability],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("Failed to validate subtask lifecycle grants: {error}"))?;
        if still_granted == 0 {
            return Err("Subtask lifecycle capability was revoked from its parent.".to_string());
        }
    }
    Ok(())
}

fn subtask_authority_status_on(
    connection: &Connection,
    authority_id: u64,
) -> Result<Option<SubtaskAuthorityStatus>, String> {
    connection
        .query_row(
            "SELECT authority_id, state, terminal_reason, lease_expires_at,
                    tool_calls_used, mutation_calls_used, completed_at
               FROM scheduler_subtask_authorities WHERE authority_id = ?1",
            params![to_i64(authority_id)?],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to load subtask authority status: {error}"))?
        .map(
            |(
                authority_id,
                state,
                terminal_reason,
                lease_expires_at,
                tool_calls_used,
                mutation_calls_used,
                completed_at,
            )| {
                let terminal_reason_valid = match (state.as_str(), terminal_reason.as_deref()) {
                    ("active", None) | ("completed", Some("completed")) => true,
                    (
                        "revoked",
                        Some(
                            "cancelled" | "failed" | "lease_expired" | "parent_inactive"
                            | "parent_cancelled" | "parent_resolved",
                        ),
                    ) => true,
                    _ => false,
                };
                if !terminal_reason_valid
                    || (state == "active" && completed_at.is_some())
                    || (state != "active" && completed_at.is_none())
                {
                    return Err("Stored subtask authority lifecycle is inconsistent.".to_string());
                }
                Ok(SubtaskAuthorityStatus {
                    authority_id: from_i64(authority_id, "subtask authority ID")?,
                    state: state.clone(),
                    terminal_reason,
                    lease_expires_at: (state == "active")
                        .then(|| from_i64(lease_expires_at, "subtask authority lease"))
                        .transpose()?,
                    tool_calls_used: u32::try_from(from_i64(
                        tool_calls_used,
                        "subtask tool usage",
                    )?)
                    .map_err(|_| "Subtask tool usage exceeds u32.".to_string())?,
                    mutation_calls_used: u32::try_from(from_i64(
                        mutation_calls_used,
                        "subtask mutation usage",
                    )?)
                    .map_err(|_| "Subtask mutation usage exceeds u32.".to_string())?,
                    completed_at: completed_at
                        .map(|value| from_i64(value, "subtask completion timestamp"))
                        .transpose()?,
                })
            },
        )
        .transpose()
}

fn subtask_authority_descriptor_matches_on(
    connection: &Connection,
    authority: &SubtaskToolAuthority,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1
                 FROM scheduler_subtask_authorities authorities
                 JOIN scheduler_subtask_attempts subtask_attempts
                   ON subtask_attempts.subtask_attempt_id = authorities.subtask_attempt_id
                 JOIN scheduler_prepared_subtasks subtasks
                   ON subtasks.subtask_id = subtask_attempts.subtask_id
                 JOIN scheduler_task_attempts parent_attempts
                   ON parent_attempts.attempt_id = authorities.parent_attempt_id
                WHERE authorities.authority_id = ?1
                  AND authorities.authority_fencing_token = ?2
                  AND authorities.lease_expires_at = ?3
                  AND authorities.parent_attempt_id = ?4
                  AND authorities.parent_fencing_token = ?5
                  AND subtask_attempts.subtask_attempt_id = ?6
                  AND subtask_attempts.attempt_number = ?7
                  AND subtask_attempts.fencing_token = ?8
                  AND subtasks.subtask_id = ?9
                  AND subtasks.session_id = ?10
                  AND subtasks.objective_id = ?11
                  AND parent_attempts.session_id = ?10
                  AND parent_attempts.attempt_number = ?12
                  AND parent_attempts.owner_id = ?13
                  AND parent_attempts.fencing_token = ?5
             )",
            params![
                to_i64(authority.authority_id)?,
                to_i64(authority.authority_fencing_token)?,
                to_i64(authority.lease_expires_at)?,
                to_i64(authority.parent_attempt_id)?,
                to_i64(authority.parent_fencing_token)?,
                to_i64(authority.subtask_attempt_id)?,
                i64::from(authority.subtask_attempt),
                to_i64(authority.subtask_fencing_token)?,
                to_i64(authority.subtask_id)?,
                to_i64(authority.session_id.0)?,
                authority.objective_id,
                i64::from(authority.parent_attempt),
                to_i64(authority.parent_owner_id)?
            ],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|error| format!("Failed to validate subtask authority descriptor: {error}"))
}

fn recover_subtask_authorities_on(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<usize, String> {
    transaction
        .execute(
            "UPDATE scheduler_subtask_authorities AS authorities
                SET state = 'revoked',
                    terminal_reason = CASE
                      WHEN authorities.lease_expires_at <= ?1 THEN 'lease_expired'
                      ELSE 'parent_inactive'
                    END,
                    completed_at = ?1,
                    updated_at = ?1
              WHERE authorities.state = 'active'
                AND (
                  authorities.lease_expires_at <= ?1
                  OR NOT EXISTS (
                    SELECT 1
                      FROM scheduler_task_attempts attempts
                      JOIN scheduler_task_sessions sessions
                        ON sessions.active_attempt_id = attempts.attempt_id
                     WHERE attempts.attempt_id = authorities.parent_attempt_id
                       AND attempts.fencing_token = authorities.parent_fencing_token
                       AND attempts.state = 'running'
                       AND attempts.lease_expires_at > ?1
                       AND sessions.state = 'running'
                       AND sessions.lease_expires_at > ?1
                  )
                )",
            params![to_i64(now)?],
        )
        .map_err(|error| format!("Failed to recover expired subtask authorities: {error}"))
}

fn revoke_subtask_authorities_for_parent_on(
    transaction: &Transaction<'_>,
    parent_attempt_id: u64,
    now: u64,
    reason: &str,
) -> Result<usize, String> {
    if !matches!(reason, "parent_cancelled" | "parent_resolved") {
        return Err("Subtask parent revocation reason is invalid.".to_string());
    }
    transaction
        .execute(
            "UPDATE scheduler_subtask_authorities
                SET state = 'revoked', terminal_reason = ?2,
                    completed_at = ?3, updated_at = ?3
              WHERE parent_attempt_id = ?1 AND state = 'active'",
            params![to_i64(parent_attempt_id)?, reason, to_i64(now)?],
        )
        .map_err(|error| format!("Failed to revoke parent subtask authorities: {error}"))
}

fn external_authority_is_current_on(
    connection: &Connection,
    authority: &ExternalAssignmentAuthority,
    now: u64,
) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT 1
               FROM scheduler_task_sessions sessions
               JOIN scheduler_task_attempts attempts
                 ON attempts.attempt_id = sessions.active_attempt_id
               JOIN scheduler_task_grants grants ON grants.session_id = sessions.session_id
              WHERE sessions.session_id = ?1 AND sessions.state = 'running'
                AND sessions.active_attempt_id = ?2 AND sessions.fencing_token = ?3
                AND sessions.lease_expires_at > ?4
                AND attempts.session_id = sessions.session_id
                AND attempts.attempt_number = ?5 AND attempts.owner_id = ?6
                AND attempts.fencing_token = ?3 AND attempts.state = 'running'
                AND attempts.lease_expires_at > ?4 AND grants.capability = ?7",
            params![
                to_i64(authority.session_id.0)?,
                to_i64(authority.attempt_id)?,
                to_i64(authority.fencing_token)?,
                to_i64(now)?,
                i64::from(authority.attempt),
                to_i64(authority.owner_id)?,
                authority.capability
            ],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|error| format!("Failed to validate resource mutation authority: {error}"))
}

fn validate_resolution_evidence(
    record: &ResourceMutationRecord,
    evidence: &ResourceMutationEvidence,
) -> Result<(), String> {
    evidence.validate()?;
    if evidence.identity != record.identity {
        return Err(
            "Resource mutation evidence did not match its reservation identity.".to_string(),
        );
    }
    Ok(())
}

fn validate_failure_identity(
    record: &ResourceMutationRecord,
    evidence: Option<&ResourceMutationEvidence>,
    kind: &str,
    code: &str,
) -> Result<(), String> {
    if let Some(evidence) = evidence {
        validate_resolution_evidence(record, evidence)?;
    }
    validate_ledger_token(kind, "failure kind")?;
    validate_ledger_token(code, "failure code")
}

fn validate_ledger_token(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
        })
    {
        return Err(format!("Resource mutation {field} is invalid."));
    }
    Ok(())
}

fn validate_operation_key(value: &str) -> Result<(), String> {
    if value.strip_prefix("sha256:").is_none_or(|digest| {
        digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err("Resource mutation operation key is invalid.".to_string());
    }
    Ok(())
}

fn validate_supersede_reason(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 500 || value.chars().any(char::is_control) {
        return Err("Resource mutation supersede reason is invalid.".to_string());
    }
    Ok(())
}

fn mark_attempt_resource_mutations_uncertain(
    transaction: &Transaction<'_>,
    attempt_id: u64,
    now: u64,
    code: &str,
) -> Result<usize, String> {
    validate_ledger_token(code, "uncertainty code")?;
    transaction
        .execute(
            "UPDATE scheduler_resource_mutations
                SET state = 'uncertain', failure_kind = 'lifecycle', failure_code = ?2,
                    revision = revision + 1, resolved_at = ?3
              WHERE attempt_id = ?1 AND state = 'reserved'",
            params![to_i64(attempt_id)?, code, to_i64(now)?],
        )
        .map_err(|error| format!("Failed to retain uncertain resource mutations: {error}"))
}

fn validate_capability_grants(
    capabilities: &[String],
    grant_source: &str,
) -> Result<Vec<String>, String> {
    if !capabilities.is_empty() && grant_source.trim().is_empty() {
        return Err("Task capability grant source is required.".to_string());
    }
    if capabilities
        .iter()
        .any(|capability| capability.trim().is_empty() || capability != capability.trim())
    {
        return Err("Task capability grants must be non-empty canonical values.".to_string());
    }
    let mut normalized = capabilities.to_vec();
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn next_sequence(transaction: &Transaction<'_>, column: &str) -> Result<u64, String> {
    let query = format!("SELECT {column} FROM scheduler_metadata WHERE singleton = 1");
    let value: i64 = transaction
        .query_row(&query, [], |row| row.get(0))
        .map_err(|error| format!("Failed to read scheduler sequence: {error}"))?;
    let update =
        format!("UPDATE scheduler_metadata SET {column} = {column} + 1 WHERE singleton = 1");
    transaction
        .execute(&update, [])
        .map_err(|error| format!("Failed to advance scheduler sequence: {error}"))?;
    from_i64(value, "scheduler sequence")
}

fn recover_expired_in_transaction(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<Vec<TaskSessionId>, String> {
    let recovered = recover_matching_attempts(
        transaction,
        "attempts.lease_expires_at <= ?1",
        params![to_i64(now)?],
        now,
        "Assignment lease expired.",
        "assignment_lease_expired",
    )?;
    recover_subtask_authorities_on(transaction, now)?;
    Ok(recovered)
}

fn recover_matching_attempts<P: rusqlite::Params>(
    transaction: &Transaction<'_>,
    predicate: &str,
    parameters: P,
    now: u64,
    attempt_error: &str,
    event_reason: &str,
) -> Result<Vec<TaskSessionId>, String> {
    let query = format!(
        "SELECT attempts.attempt_id, attempts.session_id, attempts.fencing_token,
                sessions.label, sessions.payload, sessions.state,
                sessions.opencode_session_id
           FROM scheduler_task_attempts attempts
           JOIN scheduler_task_sessions sessions ON sessions.session_id = attempts.session_id
          WHERE attempts.state = 'running' AND {predicate}"
    );
    let attempts = {
        let mut statement = transaction
            .prepare(&query)
            .map_err(|error| format!("Failed to prepare scheduler recovery: {error}"))?;
        let rows = statement
            .query_map(parameters, |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|error| format!("Failed to query scheduler recovery: {error}"))?;
        let decoded = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode scheduler recovery: {error}"))?;
        decoded
    };
    for (attempt_id, session_id, fencing_token, label, payload, previous_state, opencode_id) in
        &attempts
    {
        let uncertain_mutation_count = mark_attempt_resource_mutations_uncertain(
            transaction,
            from_i64(*attempt_id, "task attempt ID")?,
            now,
            event_reason,
        )?;
        let request = TaskRequest::with_payload(label, payload);
        let is_agent = request_is_agent_envelope(&request)?;
        let missing_runtime_identity = previous_state != "cancelling"
            && is_agent
            && opencode_id
                .as_deref()
                .is_none_or(|identity| identity.trim().is_empty());
        let mutation_reconciliation_required =
            previous_state != "cancelling" && uncertain_mutation_count > 0;
        let attempt_error = if mutation_reconciliation_required {
            RECOVERY_REQUIRES_MUTATION_RECONCILIATION
        } else if missing_runtime_identity {
            RECOVERY_REQUIRES_RETRY_FRESH
        } else {
            attempt_error
        };
        let attempt_updated = transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = 'interrupted', lease_expires_at = NULL,
                        completed_at = ?2, error = ?3
                  WHERE attempt_id = ?1 AND state = 'running'",
                params![attempt_id, to_i64(now)?, attempt_error],
            )
            .map_err(|error| format!("Failed to interrupt scheduler attempt: {error}"))?;
        if attempt_updated != 1 {
            return Err("Scheduler recovery lost ownership of an active attempt.".to_string());
        }
        let (next_state, recovery_reason, session_error) = if previous_state == "cancelling" {
            ("cancelled", event_reason, None)
        } else if mutation_reconciliation_required {
            (
                "blocked",
                "recovery_uncertain_mutation",
                Some(RECOVERY_REQUIRES_MUTATION_RECONCILIATION),
            )
        } else if missing_runtime_identity {
            (
                "blocked",
                "recovery_missing_opencode_session",
                Some(RECOVERY_REQUIRES_RETRY_FRESH),
            )
        } else {
            ("queued", event_reason, None)
        };
        let session_updated = transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = ?4,
                        active_attempt_id = NULL, lease_expires_at = NULL,
                        completed_at = CASE WHEN ?4 IN ('cancelled', 'blocked') THEN ?2 ELSE NULL END,
                        error = ?5
                  WHERE session_id = ?1 AND active_attempt_id = ?3
                    AND state IN ('running', 'cancelling')",
                params![session_id, to_i64(now)?, attempt_id, next_state, session_error],
            )
            .map_err(|error| format!("Failed to recover scheduler session: {error}"))?;
        if session_updated != 1 {
            return Err("Scheduler recovery lost ownership of an active session.".to_string());
        }
        let recovery = match (next_state, mutation_reconciliation_required) {
            ("blocked", true) => Some("operator_reconciliation"),
            ("queued", _) if is_agent => Some("resume"),
            ("blocked", false) => Some("retry_fresh_required"),
            _ => None,
        };
        append_event_in_transaction(
            transaction,
            TaskSessionId(from_i64(*session_id, "task session ID")?),
            Some(from_i64(*attempt_id, "task attempt ID")?),
            from_i64(*fencing_token, "fencing token")?,
            &TaskSessionEventInput {
                kind: TaskSessionEventKind::Lifecycle,
                payload: json!({
                    "state": next_state,
                    "reason": recovery_reason,
                    "recovery": recovery,
                    "action": recovery,
                    "error": session_error,
                    "uncertain_mutation_count": uncertain_mutation_count,
                    "opencode_session_id": opencode_id
                }),
                progress: Some(TaskProgress {
                    phase: next_state.to_string(),
                    completed: 0,
                    total: None,
                }),
            },
            now,
        )?;
    }
    attempts
        .into_iter()
        .map(|(_, session_id, _, _, _, _, _)| {
            from_i64(session_id, "task session ID").map(TaskSessionId)
        })
        .collect()
}

fn request_is_agent_envelope(request: &TaskRequest) -> Result<bool, String> {
    if request.payload.trim().is_empty() {
        return Ok(false);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&request.payload) else {
        return Ok(false);
    };
    let claims_envelope_schema =
        value.get("schema_version").is_some() || value.get("session").is_some();
    if !claims_envelope_schema {
        return Ok(false);
    }
    request.envelope().map(|envelope| {
        envelope.is_some_and(|value| value.session().kind == TaskSessionKind::Agent)
    })
}

fn append_event_in_transaction(
    transaction: &Transaction<'_>,
    session_id: TaskSessionId,
    attempt_id: Option<u64>,
    fencing_token: u64,
    input: &TaskSessionEventInput,
    now: u64,
) -> Result<TaskSessionEvent, String> {
    input
        .progress
        .as_ref()
        .map(TaskProgress::validate)
        .transpose()?;
    let sequence: i64 = transaction
        .query_row(
            "SELECT next_event_sequence FROM scheduler_task_sessions WHERE session_id = ?1",
            params![to_i64(session_id.0)?],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to allocate task event sequence: {error}"))?;
    let payload_json = serde_json::to_string(&input.payload)
        .map_err(|error| format!("Failed to serialize task event payload: {error}"))?;
    let event_type = indexed_event_type(input.kind, &input.payload);
    let progress_json = input
        .progress
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("Failed to serialize task event progress: {error}"))?;
    transaction
        .execute(
            "INSERT INTO scheduler_task_events
               (session_id, attempt_id, fencing_token, sequence, event_kind, event_type,
                 payload_json, progress_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                to_i64(session_id.0)?,
                attempt_id.map(to_i64).transpose()?,
                to_i64(fencing_token)?,
                sequence,
                event_kind_name(input.kind),
                event_type,
                payload_json,
                progress_json,
                to_i64(now)?
            ],
        )
        .map_err(|error| format!("Failed to append task event: {error}"))?;
    let event_id = from_i64(transaction.last_insert_rowid(), "task event ID")?;
    match &input.progress {
        Some(progress) => {
            transaction
                .execute(
                    "UPDATE scheduler_task_sessions
                        SET next_event_sequence = next_event_sequence + 1,
                            progress_phase = ?2, progress_completed = ?3, progress_total = ?4
                      WHERE session_id = ?1",
                    params![
                        to_i64(session_id.0)?,
                        progress.phase,
                        to_i64(progress.completed)?,
                        progress.total.map(to_i64).transpose()?
                    ],
                )
                .map_err(|error| format!("Failed to update task progress: {error}"))?;
        }
        None => {
            transaction
                .execute(
                    "UPDATE scheduler_task_sessions
                        SET next_event_sequence = next_event_sequence + 1
                      WHERE session_id = ?1",
                    params![to_i64(session_id.0)?],
                )
                .map_err(|error| format!("Failed to advance task event sequence: {error}"))?;
        }
    }
    Ok(TaskSessionEvent {
        id: event_id,
        session_id,
        attempt_id,
        fencing_token,
        sequence: from_i64(sequence, "task event sequence")?,
        kind: input.kind,
        payload: input.payload.clone(),
        progress: input.progress.clone(),
        created_at: now,
    })
}

fn indexed_event_type(kind: TaskSessionEventKind, payload: &serde_json::Value) -> &'static str {
    let payload_type = payload.get("type").and_then(serde_json::Value::as_str);
    match (kind, payload_type) {
        (TaskSessionEventKind::Lifecycle, _) => "lifecycle",
        (TaskSessionEventKind::Progress, _) => "progress",
        (TaskSessionEventKind::Activity, _) => "activity",
        (TaskSessionEventKind::Tool, Some("tool_started")) => "tool_started",
        (TaskSessionEventKind::Tool, Some("tool_completed")) => "tool_completed",
        (TaskSessionEventKind::Tool, _) => "tool",
        (TaskSessionEventKind::Runtime, Some("execution_trace_stage")) => "execution_trace_stage",
        (TaskSessionEventKind::Runtime, Some("usage_updated")) => "usage_updated",
        (TaskSessionEventKind::Runtime, Some("opencode_session")) => "opencode_session",
        (TaskSessionEventKind::Runtime, Some("approval_requested")) => "approval_requested",
        (TaskSessionEventKind::Runtime, Some("runtime_recovery_decision")) => {
            "runtime_recovery_decision"
        }
        (TaskSessionEventKind::Runtime, Some("objective_checkpointed")) => "objective_checkpointed",
        (TaskSessionEventKind::Runtime, Some("capability_repair_decision")) => {
            "capability_repair_decision"
        }
        (TaskSessionEventKind::Runtime, Some("connector_session_recovered")) => {
            "connector_session_recovered"
        }
        (TaskSessionEventKind::Runtime, Some("subtask_contracts_prepared")) => {
            "subtask_contracts_prepared"
        }
        (TaskSessionEventKind::Runtime, _) => "runtime",
    }
}

struct StoredEvent {
    id: i64,
    session_id: i64,
    attempt_id: Option<i64>,
    fencing_token: i64,
    sequence: i64,
    kind: String,
    payload_json: String,
    progress_json: Option<String>,
    created_at: i64,
}

fn trace_entry(
    event: StoredEvent,
    event_type: String,
    attempt_number: Option<i64>,
    worker_id: Option<i64>,
) -> Result<TaskExecutionTraceEntry, String> {
    let payload: serde_json::Value = serde_json::from_str(&event.payload_json)
        .map_err(|error| format!("Failed to decode execution trace metadata: {error}"))?;
    let string = |key: &str, limit: usize| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(|value| value.chars().take(limit).collect::<String>())
            .filter(|value| !value.trim().is_empty())
    };
    let recovery = match event_type.as_str() {
        "connector_session_recovered" => Some("connector_session_recreated".to_string()),
        "subtask_contracts_prepared" => Some("subtask_authority_prepared".to_string()),
        _ => string("recovery", 64).or_else(|| string("action", 64)),
    };
    Ok(TaskExecutionTraceEntry {
        sequence: from_i64(event.sequence, "task event sequence")?,
        attempt_id: event
            .attempt_id
            .map(|value| from_i64(value, "task attempt ID"))
            .transpose()?,
        assignment_attempt: attempt_number
            .map(|value| {
                u32::try_from(value).map_err(|_| "Assignment attempt exceeds u32.".to_string())
            })
            .transpose()?,
        fencing_token: from_i64(event.fencing_token, "fencing token")?,
        event_type,
        created_at: from_i64(event.created_at, "task event timestamp")?,
        state: string("state", 32),
        stage: string("stage", 64),
        duration_us: payload
            .get("duration_us")
            .and_then(serde_json::Value::as_u64),
        outcome: string("outcome", 32),
        worker_id: worker_id
            .map(|value| from_i64(value, "worker ID"))
            .transpose()?,
        runtime_id: string("runtime_id", 256),
        opencode_session_id: string("opencode_session_id", 256),
        tool_call_id: string("tool_call_id", 256),
        tool_name: string("tool_name", 128),
        tool_success: payload.get("success").and_then(serde_json::Value::as_bool),
        input_tokens: payload
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        output_tokens: payload
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        recovery,
        approval_operation: string("operation", 128),
    })
}

impl StoredEvent {
    fn into_event(self) -> Result<TaskSessionEvent, String> {
        Ok(TaskSessionEvent {
            id: from_i64(self.id, "task event ID")?,
            session_id: TaskSessionId(from_i64(self.session_id, "task session ID")?),
            attempt_id: self
                .attempt_id
                .map(|value| from_i64(value, "task attempt ID"))
                .transpose()?,
            fencing_token: from_i64(self.fencing_token, "fencing token")?,
            sequence: from_i64(self.sequence, "task event sequence")?,
            kind: parse_event_kind(&self.kind)?,
            payload: serde_json::from_str(&self.payload_json)
                .map_err(|error| format!("Failed to decode task event payload: {error}"))?,
            progress: self
                .progress_json
                .map(|value| {
                    serde_json::from_str(&value)
                        .map_err(|error| format!("Failed to decode task event progress: {error}"))
                })
                .transpose()?,
            created_at: from_i64(self.created_at, "task event timestamp")?,
        })
    }
}

fn stored_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    Ok(StoredEvent {
        id: row.get(0)?,
        session_id: row.get(1)?,
        attempt_id: row.get(2)?,
        fencing_token: row.get(3)?,
        sequence: row.get(4)?,
        kind: row.get(5)?,
        payload_json: row.get(6)?,
        progress_json: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn event_kind_name(kind: TaskSessionEventKind) -> &'static str {
    match kind {
        TaskSessionEventKind::Lifecycle => "lifecycle",
        TaskSessionEventKind::Activity => "activity",
        TaskSessionEventKind::Progress => "progress",
        TaskSessionEventKind::Runtime => "runtime",
        TaskSessionEventKind::Tool => "tool",
    }
}

fn parse_event_kind(value: &str) -> Result<TaskSessionEventKind, String> {
    match value {
        "lifecycle" => Ok(TaskSessionEventKind::Lifecycle),
        "activity" => Ok(TaskSessionEventKind::Activity),
        "progress" => Ok(TaskSessionEventKind::Progress),
        "runtime" => Ok(TaskSessionEventKind::Runtime),
        "tool" => Ok(TaskSessionEventKind::Tool),
        _ => Err(format!("Unknown task event kind '{value}'.")),
    }
}

fn parse_state(value: &str) -> Result<TaskSessionState, String> {
    match value {
        "queued" => Ok(TaskSessionState::Queued),
        "running" => Ok(TaskSessionState::Running),
        "cancelling" => Ok(TaskSessionState::Cancelling),
        "committing" => Ok(TaskSessionState::Committing),
        "succeeded" => Ok(TaskSessionState::Succeeded),
        "failed" => Ok(TaskSessionState::Failed),
        "blocked" => Ok(TaskSessionState::Blocked),
        "cancelled" => Ok(TaskSessionState::Cancelled),
        _ => Err(format!("Unknown task session state '{value}'.")),
    }
}

fn state_text(state: TaskSessionState) -> &'static str {
    match state {
        TaskSessionState::Queued => "queued",
        TaskSessionState::Running => "running",
        TaskSessionState::Cancelling => "cancelling",
        TaskSessionState::Committing => "committing",
        TaskSessionState::Succeeded => "succeeded",
        TaskSessionState::Failed => "failed",
        TaskSessionState::Blocked => "blocked",
        TaskSessionState::Cancelled => "cancelled",
    }
}

fn execute_batch_with_busy_retry(
    connection: &Connection,
    statements: &str,
) -> rusqlite::Result<()> {
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        match connection.execute_batch(statements) {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                ) && attempt < MAX_ATTEMPTS =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("schema initialization retry loop always returns")
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let query = format!("PRAGMA table_info({table})");
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| format!("Failed to inspect scheduler table {table}: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("Failed to query scheduler table {table}: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to decode scheduler table {table}: {error}"))?;
    drop(statement);
    if columns.iter().any(|name| name == column) {
        return Ok(());
    }
    connection
        .execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"), [])
        .map_err(|error| format!("Failed to add scheduler column {table}.{column}: {error}"))?;
    Ok(())
}

fn database_path() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path).join("spacesly").join("scheduler.db"));
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not configured.".to_string())?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("spacesly")
        .join("scheduler.db"))
}

fn duration_millis(duration: Duration) -> Result<u64, String> {
    duration
        .as_millis()
        .try_into()
        .map_err(|_| "Duration exceeds u64 milliseconds.".to_string())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("Value {value} exceeds SQLite integer range."))
}

fn from_i64(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} cannot be negative."))
}

fn request_ownership(request: &TaskRequest) -> TaskOwnership {
    let Ok(Some(envelope)) = request.envelope() else {
        return TaskOwnership::default();
    };
    let session = envelope.session();
    TaskOwnership {
        workspace_id: Some(session.workspace_id.clone()),
        conversation_id: session.conversation_id.clone(),
        subject_id: session.subject_id.clone(),
        execution_run_id: session.execution_run_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::resource_idempotency::{
        ResourceExecutionResult, ResourceExecutionStatus, ResourceIdentity, ResourceLookupResult,
        ResourceLookupStatus, ResourceRetryResumeStatus,
    };
    use crate::domain::subtask_authority::prepare_subtask_contracts;
    use crate::domain::task_examination::{
        SemanticPlannerEvidence, TaskCapabilityRecord, TaskExaminationRecord,
        TaskExaminationStatus, TASK_EXAMINATION_SCHEMA_VERSION, TASK_EXAMINER_VERSION,
    };
    use crate::domain::task_session::{TaskSessionEnvelopeV1, TaskSessionKind};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::tempdir;

    const LEASE_MILLIS: u64 = 1_000;

    #[test]
    fn first_phase_database_schema_migrates_without_data_loss() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let connection = rusqlite::Connection::open(&path).expect("legacy database opens");
        connection
            .execute_batch(
                "CREATE TABLE scheduler_metadata (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   next_enqueue_sequence INTEGER NOT NULL,
                   next_dispatch_sequence INTEGER NOT NULL
                 );
                 INSERT INTO scheduler_metadata
                   (singleton, next_enqueue_sequence, next_dispatch_sequence)
                 VALUES (1, 1, 1);
                 CREATE TABLE scheduler_task_sessions (
                   session_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   enqueue_sequence INTEGER NOT NULL UNIQUE,
                   label TEXT NOT NULL,
                   payload TEXT NOT NULL,
                   state TEXT NOT NULL,
                   worker_id INTEGER,
                   dispatch_sequence INTEGER,
                   attempt_count INTEGER NOT NULL DEFAULT 0,
                   active_attempt_id INTEGER,
                   fencing_token INTEGER NOT NULL DEFAULT 0,
                   lease_expires_at INTEGER,
                   error TEXT,
                   created_at INTEGER NOT NULL,
                   started_at INTEGER,
                   completed_at INTEGER
                 );
                  CREATE TABLE scheduler_task_objective_checkpoints (
                   session_id INTEGER NOT NULL,
                   objective_id TEXT NOT NULL,
                   evidence_json TEXT NOT NULL,
                   source_attempt_id INTEGER NOT NULL,
                   source_fencing_token INTEGER NOT NULL,
                    recorded_at INTEGER NOT NULL,
                    PRIMARY KEY(session_id, objective_id)
                  );
                  CREATE TABLE scheduler_resource_mutations (
                    mutation_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    operation_key TEXT NOT NULL,
                    identity_json TEXT NOT NULL,
                    connector_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    state TEXT NOT NULL,
                    session_id INTEGER NOT NULL,
                    attempt_id INTEGER NOT NULL,
                    attempt_number INTEGER NOT NULL,
                    fencing_token INTEGER NOT NULL,
                    evidence_json TEXT,
                    failure_kind TEXT,
                    failure_code TEXT,
                    revision INTEGER NOT NULL DEFAULT 1,
                    reserved_at INTEGER NOT NULL,
                    resolved_at INTEGER,
                    superseded_at INTEGER,
                    supersede_reason TEXT
                  );",
            )
            .expect("legacy schema created");
        drop(connection);

        let barrier = Arc::new(Barrier::new(3));
        let first = {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                SchedulerStore::open_at(path)
            })
        };
        let second = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                SchedulerStore::open_at(path)
            })
        };
        barrier.wait();
        let store = first
            .join()
            .expect("first migration joins")
            .expect("first migration succeeds");
        second
            .join()
            .expect("second migration joins")
            .expect("second migration succeeds");
        let session = store
            .enqueue_at(&TaskRequest::new("migrated"), 10)
            .expect("task enqueued");
        let restored = store
            .get_session(session.id)
            .expect("session read")
            .expect("session exists");
        assert_eq!(restored.last_event_sequence, 1);
        assert_eq!(
            store
                .events_after(session.id, 0)
                .expect("events read")
                .len(),
            1
        );
        let checkpoint_columns = store
            .connection
            .lock()
            .expect("scheduler connection")
            .prepare("PRAGMA table_info(scheduler_task_objective_checkpoints)")
            .expect("checkpoint columns prepared")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("checkpoint columns queried")
            .collect::<Result<Vec<_>, _>>()
            .expect("checkpoint columns read");
        assert!(checkpoint_columns
            .iter()
            .any(|column| column == "tool_receipts_json"));
        let mutation_columns = store
            .connection
            .lock()
            .expect("scheduler connection")
            .prepare("PRAGMA table_info(scheduler_resource_mutations)")
            .expect("mutation columns prepared")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("mutation columns queried")
            .collect::<Result<Vec<_>, _>>()
            .expect("mutation columns read");
        for expected in [
            "checkpoint_objective_id",
            "checkpoint_tool_call_id",
            "checkpoint_recorded_at",
        ] {
            assert!(mutation_columns.iter().any(|column| column == expected));
        }
        let scheduler_tables = store
            .connection
            .lock()
            .expect("scheduler connection")
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .expect("scheduler tables prepared")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("scheduler tables queried")
            .collect::<Result<Vec<_>, _>>()
            .expect("scheduler tables read");
        for expected in [
            "scheduler_prepared_subtasks",
            "scheduler_subtask_attempts",
            "scheduler_subtask_authorities",
        ] {
            assert!(scheduler_tables.iter().any(|table| table == expected));
        }
        let subtask_authority_columns = store
            .connection
            .lock()
            .expect("scheduler connection")
            .prepare("PRAGMA table_info(scheduler_subtask_authorities)")
            .expect("subtask authority columns prepared")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("subtask authority columns queried")
            .collect::<Result<Vec<_>, _>>()
            .expect("subtask authority columns read");
        for expected in ["completed_at", "terminal_reason"] {
            assert!(subtask_authority_columns
                .iter()
                .any(|column| column == expected));
        }
    }

    #[test]
    fn query_store_replays_without_lifecycle_write_authority() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let session = {
            let store = SchedulerStore::open_at(path.clone()).expect("store opens");
            store
                .enqueue_at(&TaskRequest::new("query-only"), 1)
                .expect("task enqueued")
        };
        let query = SchedulerStore::open_query_at(path).expect("query store opens");
        assert_eq!(
            query
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .last_event_sequence,
            1
        );
        assert_eq!(
            query
                .event_page(session.id, 0, 100)
                .expect("event page read")
                .events
                .len(),
            1
        );
        assert!(query.enqueue(&TaskRequest::new("forbidden")).is_err());
    }

    #[test]
    fn sessions_and_fifo_order_survive_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let first = store
            .enqueue_at(&TaskRequest::new("first"), 10)
            .expect("first enqueued");
        let second = store
            .enqueue_at(&TaskRequest::new("second"), 20)
            .expect("second enqueued");
        drop(store);

        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        let owner = reopened.register_owner().expect("owner registered");
        let first_assignment = reopened
            .claim_next_at(owner, 1, 30, LEASE_MILLIS, 5)
            .expect("first claimed")
            .expect("first assignment");
        let second_assignment = reopened
            .claim_next_at(owner, 2, 30, LEASE_MILLIS, 5)
            .expect("second claimed")
            .expect("second assignment");
        assert_eq!(first_assignment.fence.session_id, first.id);
        assert_eq!(second_assignment.fence.session_id, second.id);
        let first_snapshot = reopened
            .get_session(first.id)
            .expect("first session read")
            .expect("first session exists");
        let second_snapshot = reopened
            .get_session(second.id)
            .expect("second session read")
            .expect("second session exists");
        assert!(first_snapshot.dispatch_sequence < second_snapshot.dispatch_sequence);
    }

    #[test]
    fn two_connections_cannot_claim_the_same_fifo_session() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let first_store = SchedulerStore::open_at(path.clone()).expect("first store opens");
        let second_store = SchedulerStore::open_at(path).expect("second store opens");
        first_store
            .enqueue_at(&TaskRequest::new("only"), 10)
            .expect("task enqueued");
        let first_owner = first_store.register_owner().expect("first owner");
        let second_owner = second_store.register_owner().expect("second owner");
        let barrier = Arc::new(Barrier::new(3));
        let first = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                first_store
                    .claim_next_at(first_owner, 1, 20, LEASE_MILLIS, 5)
                    .expect("first claim")
            })
        };
        let second = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                second_store
                    .claim_next_at(second_owner, 2, 20, LEASE_MILLIS, 5)
                    .expect("second claim")
            })
        };
        barrier.wait();
        let claimed = [
            first.join().expect("first joins"),
            second.join().expect("second joins"),
        ]
        .into_iter()
        .filter(Option::is_some)
        .count();
        assert_eq!(claimed, 1);
    }

    #[test]
    fn global_running_limit_is_enforced_across_owners() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        for index in 0..7 {
            store
                .enqueue_at(&TaskRequest::new(format!("task-{index}")), index)
                .expect("task enqueued");
        }
        let assignments = (1..=7)
            .filter_map(|worker| {
                store
                    .claim_next_at(owner, worker, 10, LEASE_MILLIS, 5)
                    .expect("claim succeeds")
            })
            .collect::<Vec<_>>();
        assert_eq!(assignments.len(), 5);
        assert_eq!(
            store
                .list_sessions()
                .expect("sessions listed")
                .iter()
                .filter(|session| session.state == TaskSessionState::Queued)
                .count(),
            2
        );
    }

    #[test]
    fn global_running_limit_is_enforced_across_connections() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let first = SchedulerStore::open_at(path.clone()).expect("first store opens");
        let second = SchedulerStore::open_at(path).expect("second store opens");
        let first_owner = first.register_owner().expect("first owner");
        let second_owner = second.register_owner().expect("second owner");
        for index in 0..8 {
            first
                .enqueue_at(&TaskRequest::new(format!("task-{index}")), index)
                .expect("task enqueued");
        }
        let mut claimed = 0;
        for worker in 1..=8 {
            let (store, owner) = if worker % 2 == 0 {
                (&first, first_owner)
            } else {
                (&second, second_owner)
            };
            if store
                .claim_next_at(owner, worker, 10, LEASE_MILLIS, 5)
                .expect("claim succeeds")
                .is_some()
            {
                claimed += 1;
            }
        }
        assert_eq!(claimed, 5);
    }

    #[test]
    fn unexpired_foreign_lease_is_not_stolen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let first = SchedulerStore::open_at(path.clone()).expect("first store opens");
        let second = SchedulerStore::open_at(path).expect("second store opens");
        let first_owner = first.register_owner().expect("first owner");
        let second_owner = second.register_owner().expect("second owner");
        first
            .enqueue_at(&TaskRequest::new("leased"), 1)
            .expect("task enqueued");
        let first_assignment = first
            .claim_next_at(first_owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");

        assert_eq!(
            second.recover_expired_at(1_009).expect("recovery checked"),
            0
        );
        assert!(second
            .claim_next_at(second_owner, 2, 1_009, LEASE_MILLIS, 5)
            .expect("second claim checked")
            .is_none());
        assert_eq!(
            second
                .get_session(first_assignment.fence.session_id)
                .expect("session read")
                .expect("session exists")
                .fencing_token,
            first_assignment.fence.fencing_token
        );
    }

    #[test]
    fn expired_attempt_is_requeued_with_a_new_fencing_token() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("retry"), 1)
            .expect("task enqueued");
        let first = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");
        assert_eq!(store.recover_expired_at(1_010).expect("recovered"), 1);
        let second = store
            .claim_next_at(owner, 1, 1_011, LEASE_MILLIS, 5)
            .expect("second claim")
            .expect("second assignment");
        assert_eq!(second.fence.session_id, session.id);
        assert_eq!(second.fence.attempt, 2);
        assert!(second.fence.fencing_token > first.fence.fencing_token);
        assert!(matches!(
            store
                .resolve_assignment_at(
                    first.fence,
                    DurableOutcome::Succeeded(TaskExecutionOutput::None),
                    1_012,
                )
                .expect("stale finish checked"),
            FinishResult::Stale
        ));
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Running
        );
    }

    #[test]
    fn expired_legacy_opaque_payload_is_still_requeued() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = TaskRequest::with_payload("legacy", "opaque mock executor payload");
        let session = store.enqueue_at(&request, 1).expect("task enqueued");
        store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");

        assert_eq!(store.recover_expired_at(1_010).expect("recovered"), 1);
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Queued
        );
    }

    #[test]
    fn malformed_versioned_envelope_fails_recovery_closed() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request =
            TaskRequest::with_payload("malformed envelope", r#"{"schema_version":1,"session":{}}"#);
        let session = store.enqueue_at(&request, 1).expect("task enqueued");
        store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("claim")
            .expect("assignment");

        assert!(store
            .recover_expired_at(1_010)
            .expect_err("malformed durable envelope must fail closed")
            .contains("Failed to decode Task Session envelope"));
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Running
        );
    }

    #[test]
    fn expired_agent_without_runtime_identity_requires_retry_fresh() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("pre-bind", "conversation-pre-bind", "subject-pre-bind");
        let session = store.enqueue_at(&request, 1).expect("task enqueued");
        let first = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");

        assert_eq!(store.recover_expired_at(1_010).expect("recovered"), 1);
        let recovered = store
            .get_session(session.id)
            .expect("session read")
            .expect("session exists");
        assert_eq!(recovered.state, TaskSessionState::Blocked);
        assert_eq!(
            recovered.error.as_deref(),
            Some(RECOVERY_REQUIRES_RETRY_FRESH)
        );
        assert!(store
            .claim_next_at(owner, 1, 1_011, LEASE_MILLIS, 5)
            .expect("claim checked")
            .is_none());
        let events = store.events_after(session.id, 0).expect("events read");
        let recovery = events.last().expect("recovery event");
        assert_eq!(recovery.attempt_id, Some(first.fence.attempt_id));
        assert_eq!(recovery.payload["state"], "blocked");
        assert_eq!(
            recovery.payload["reason"],
            "recovery_missing_opencode_session"
        );
        assert_eq!(recovery.payload["action"], "retry_fresh_required");
    }

    #[test]
    fn expired_agent_resumes_the_same_durable_runtime_identity() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("resume", "conversation-resume", "subject-resume");
        let session = store.enqueue(&request).expect("task enqueued");
        let first = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("first claim")
            .expect("first assignment");
        store
            .bind_opencode_session(first.fence, "opencode-recovery-session")
            .expect("runtime identity bound");

        assert_eq!(store.abandon_owner(owner).expect("owner abandoned"), 1);
        let queued = store
            .get_session(session.id)
            .expect("session read")
            .expect("session exists");
        assert_eq!(queued.state, TaskSessionState::Queued);
        assert_eq!(
            queued.opencode_session_id.as_deref(),
            Some("opencode-recovery-session")
        );
        let second = store
            .claim_next(owner, 2, Duration::from_secs(5), 5)
            .expect("second claim")
            .expect("second assignment");
        assert_eq!(second.fence.session_id, session.id);
        assert_eq!(second.fence.attempt, 2);
        assert_eq!(
            store
                .assignment_opencode_session(second.fence)
                .expect("runtime identity read")
                .as_deref(),
            Some("opencode-recovery-session")
        );
    }

    #[test]
    fn renewal_prevents_expiry_and_wrong_owner_is_rejected() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        store
            .enqueue_at(&TaskRequest::new("renew"), 1)
            .expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("claim")
            .expect("assignment");
        assert!(store
            .renew_at(assignment.fence, 500, LEASE_MILLIS)
            .expect("lease renewed"));
        assert_eq!(
            store.recover_expired_at(1_010).expect("recovery checked"),
            0
        );
        let wrong_owner = AssignmentFence {
            owner_id: owner + 1,
            ..assignment.fence
        };
        assert!(!store
            .renew_at(wrong_owner, 600, LEASE_MILLIS)
            .expect("wrong owner checked"));
        assert!(!store
            .renew_at(assignment.fence, 1_500, LEASE_MILLIS)
            .expect("expired lease checked"));
        assert!(!store
            .assignment_is_current_at(assignment.fence, 1_500)
            .expect("expired authority checked"));
    }

    #[test]
    fn queued_cancellation_creates_no_attempt() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("cancel"), 1)
            .expect("task enqueued");
        let cancelled = store.cancel_at(session.id, 2).expect("task cancelled");
        assert!(cancelled.changed);
        assert_eq!(cancelled.snapshot.state, TaskSessionState::Cancelled);
        assert!(store
            .claim_next_at(owner, 1, 3, LEASE_MILLIS, 5)
            .expect("claim checked")
            .is_none());
    }

    #[test]
    fn expired_cancelling_session_becomes_cancelled_instead_of_requeued() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("cancel-running"), 1)
            .expect("task enqueued");
        store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("claim")
            .expect("assignment");
        store.cancel_at(session.id, 20).expect("cancel requested");
        assert_eq!(store.recover_expired_at(1_010).expect("recovered"), 1);
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Cancelled
        );
        assert!(store
            .claim_next_at(owner, 1, 1_011, LEASE_MILLIS, 5)
            .expect("claim checked")
            .is_none());
    }

    #[test]
    fn cancelling_agent_preserves_the_cancellation_recovery_reason() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("cancel-agent", "conversation-cancel", "subject-cancel");
        let session = store.enqueue_at(&request, 1).expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("claim")
            .expect("assignment");
        store.cancel_at(session.id, 20).expect("cancel requested");

        assert_eq!(store.recover_expired_at(1_010).expect("recovered"), 1);
        let attempt_error: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT error FROM scheduler_task_attempts WHERE attempt_id = ?1",
                params![assignment.fence.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempt_error, "Assignment lease expired.");
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Cancelled
        );
    }

    #[test]
    fn concurrent_finish_accepts_only_one_fenced_completion() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let first = SchedulerStore::open_at(path.clone()).expect("first store opens");
        let second = SchedulerStore::open_at(path).expect("second store opens");
        let owner = first.register_owner().expect("owner registered");
        first
            .enqueue_at(&TaskRequest::new("finish-once"), 1)
            .expect("task enqueued");
        let assignment = first
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        let barrier = Arc::new(Barrier::new(3));
        let first_finish = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                first
                    .resolve_assignment_at(
                        assignment.fence,
                        DurableOutcome::Succeeded(TaskExecutionOutput::None),
                        20,
                    )
                    .expect("first finish")
            })
        };
        let second_finish = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                second
                    .resolve_assignment_at(
                        assignment.fence,
                        DurableOutcome::Succeeded(TaskExecutionOutput::None),
                        20,
                    )
                    .expect("second finish")
            })
        };
        barrier.wait();
        let results = [
            first_finish.join().expect("first joins"),
            second_finish.join().expect("second joins"),
        ];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, FinishResult::Applied))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, FinishResult::Stale))
                .count(),
            1
        );
    }

    #[test]
    fn event_journal_projects_progress_and_replays_after_cursor() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("journal"), 10)
            .expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 20, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Activity,
                    payload: serde_json::json!({ "message": "working" }),
                    progress: None,
                },
                21,
            )
            .expect("activity appended");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Progress,
                    payload: serde_json::json!({ "message": "building plan" }),
                    progress: Some(TaskProgress {
                        phase: "planning".to_string(),
                        completed: 2,
                        total: Some(5),
                    }),
                },
                22,
            )
            .expect("progress appended");
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .progress,
            Some(TaskProgress {
                phase: "planning".to_string(),
                completed: 2,
                total: Some(5),
            })
        );
        assert!(matches!(
            store
                .resolve_assignment_at(
                    assignment.fence,
                    DurableOutcome::Succeeded(TaskExecutionOutput::None),
                    30,
                )
                .expect("task finished"),
            FinishResult::Applied
        ));
        drop(store);

        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        let events = reopened
            .events_after(session.id, 0)
            .expect("events replayed");
        assert_eq!(events.len(), 5);
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(events[0].kind, TaskSessionEventKind::Lifecycle);
        assert_eq!(events[2].kind, TaskSessionEventKind::Activity);
        assert_eq!(events[3].kind, TaskSessionEventKind::Progress);
        assert_eq!(
            events[3].progress,
            Some(TaskProgress {
                phase: "planning".to_string(),
                completed: 2,
                total: Some(5),
            })
        );
        assert_eq!(
            reopened
                .events_after(session.id, 3)
                .expect("cursor replayed")
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        let first_page = reopened
            .event_page(session.id, 0, 2)
            .expect("first page replayed");
        assert_eq!(first_page.next_cursor, 2);
        assert!(first_page.has_more);
        assert_eq!(
            first_page
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let final_page = reopened
            .event_page(session.id, 4, 2)
            .expect("final page replayed");
        assert_eq!(final_page.next_cursor, 5);
        assert!(!final_page.has_more);
        assert_eq!(final_page.events.len(), 1);
    }

    #[test]
    fn execution_trace_page_is_indexed_bounded_and_sanitized() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("trace"), 10)
            .expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 3, 20, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        for (at, payload) in [
            (
                21,
                json!({ "type": "text_delta", "text": "secret prompt text" }),
            ),
            (
                22,
                json!({
                    "type": "execution_trace_stage",
                    "schema_version": 1,
                    "stage": "runtime_preparation",
                    "duration_us": 420_000,
                    "outcome": "succeeded",
                    "runtime_id": "runtime-1"
                }),
            ),
            (
                23,
                json!({ "type": "usage_updated", "input_tokens": 100, "output_tokens": 12 }),
            ),
        ] {
            store
                .append_assignment_event_at(
                    assignment.fence,
                    TaskSessionEventInput {
                        kind: TaskSessionEventKind::Runtime,
                        payload,
                        progress: None,
                    },
                    at,
                )
                .expect("runtime event appended");
        }
        for (at, payload) in [
            (
                24,
                json!({ "type": "tool_started", "tool_call_id": "call-1", "tool_name": "jira_search" }),
            ),
            (
                25,
                json!({
                    "type": "tool_completed",
                    "tool_call_id": "call-1",
                    "tool_name": "jira_search",
                    "success": false,
                    "error": "secret API response"
                }),
            ),
        ] {
            store
                .append_assignment_event_at(
                    assignment.fence,
                    TaskSessionEventInput {
                        kind: TaskSessionEventKind::Tool,
                        payload,
                        progress: None,
                    },
                    at,
                )
                .expect("tool event appended");
        }
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Runtime,
                    payload: json!({
                        "type": "connector_session_recovered",
                        "provider": "confluence",
                        "connector_id": "secret-connector-configuration",
                        "operation_risk": "read",
                        "connector_attempts": 2
                    }),
                    progress: None,
                },
                26,
            )
            .expect("connector recovery appended");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Runtime,
                    payload: json!({
                        "type": "subtask_contracts_prepared",
                        "subtask_count": 2,
                        "tool_call_budget": 64,
                        "mutation_call_budget": 1,
                        "authority_scope": "parent_subset",
                        "delegation_allowed": false,
                        "execution_enabled": false,
                        "private_contract": "secret-subtask-contract"
                    }),
                    progress: None,
                },
                27,
            )
            .expect("subtask authority appended");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Runtime,
                    payload: json!({ "type": "text_delta", "text": "trailing secret" }),
                    progress: None,
                },
                28,
            )
            .expect("trailing delta appended");

        let first = store
            .execution_trace_page(session.id, 0, 3)
            .expect("first trace page");
        assert_eq!(first.entries.len(), 3);
        assert!(first.has_more);
        assert_eq!(first.entries[2].event_type, "execution_trace_stage");
        assert_eq!(first.entries[2].worker_id, Some(3));
        assert_eq!(first.entries[2].assignment_attempt, Some(1));
        let second = store
            .execution_trace_page(session.id, first.next_cursor, 10)
            .expect("second trace page");
        assert!(!second.has_more);
        assert_eq!(second.next_cursor, 10);
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "usage_updated",
                "tool_started",
                "tool_completed",
                "connector_session_recovered",
                "subtask_contracts_prepared"
            ]
        );
        assert_eq!(
            second
                .entries
                .get(3)
                .and_then(|entry| entry.recovery.as_deref()),
            Some("connector_session_recreated")
        );
        assert_eq!(
            second
                .entries
                .last()
                .and_then(|entry| entry.recovery.as_deref()),
            Some("subtask_authority_prepared")
        );
        let encoded = serde_json::to_string(&(first, second)).expect("trace encoded");
        assert!(!encoded.contains("secret prompt text"));
        assert!(!encoded.contains("secret API response"));
        assert!(!encoded.contains("trailing secret"));
        assert!(!encoded.contains("secret-connector-configuration"));
        assert!(!encoded.contains("secret-subtask-contract"));

        let plan = store
            .connection
            .lock()
            .unwrap()
            .prepare(
                "EXPLAIN QUERY PLAN SELECT sequence FROM scheduler_task_events
                  WHERE session_id = 1 AND sequence > 0 AND event_type IN (
                    'lifecycle', 'tool_started', 'tool_completed',
                    'execution_trace_stage', 'usage_updated', 'opencode_session',
                    'approval_requested', 'runtime_recovery_decision',
                    'objective_checkpointed',
                    'capability_repair_decision', 'connector_session_recovered',
                    'subtask_contracts_prepared'
                  ) ORDER BY sequence LIMIT 100",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(plan.contains("idx_scheduler_events_trace"), "{plan}");

        let tool_state = store.tool_state(session.id).expect("tool state projected");
        assert_eq!(tool_state.calls.len(), 1);
        let tool_plan = store
            .connection
            .lock()
            .unwrap()
            .prepare(
                "EXPLAIN QUERY PLAN SELECT sequence FROM scheduler_task_events
                  WHERE session_id = 1
                    AND event_type IN ('tool_started', 'tool_completed')
                  ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(
            tool_plan.contains("idx_scheduler_events_tool_state"),
            "{tool_plan}"
        );
    }

    #[test]
    fn claim_outcome_reports_only_sessions_changed_by_the_transaction() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let sessions = (0..50)
            .map(|index| {
                store
                    .enqueue_at(&TaskRequest::new(format!("queued-{index}")), index + 1)
                    .expect("task enqueued")
            })
            .collect::<Vec<_>>();

        for (worker, expected) in sessions.iter().take(5).enumerate() {
            let outcome = store
                .claim_next_with_changes_at(owner, worker, 100, LEASE_MILLIS, 5)
                .expect("task claimed");
            assert_eq!(
                outcome
                    .assignment
                    .as_ref()
                    .map(|value| value.fence.session_id),
                Some(expected.id)
            );
            assert_eq!(outcome.changed_session_ids, vec![expected.id]);
        }

        let capacity = store
            .claim_next_with_changes_at(owner, 6, 101, LEASE_MILLIS, 5)
            .expect("capacity checked");
        assert!(capacity.assignment.is_none());
        assert!(capacity.changed_session_ids.is_empty());
    }

    #[test]
    fn legacy_event_insert_is_classified_for_execution_trace() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let session = store
            .enqueue_at(&TaskRequest::new("legacy trace writer"), 1)
            .expect("task enqueued");
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO scheduler_task_events
                       (session_id, attempt_id, fencing_token, sequence, event_kind,
                        payload_json, progress_json, created_at)
                     VALUES (?1, NULL, 0, 2, 'runtime', ?2, NULL, 2)",
                    params![
                        session.id.0,
                        json!({
                            "type": "execution_trace_stage",
                            "schema_version": 1,
                            "stage": "runtime_preparation",
                            "duration_us": 10,
                            "outcome": "succeeded"
                        })
                        .to_string()
                    ],
                )
                .expect("legacy event inserted");
            connection
                .execute(
                    "UPDATE scheduler_task_sessions SET next_event_sequence = 3
                      WHERE session_id = ?1",
                    params![session.id.0],
                )
                .expect("cursor advanced");
            let event_type: String = connection
                .query_row(
                    "SELECT event_type FROM scheduler_task_events
                      WHERE session_id = ?1 AND sequence = 2",
                    params![session.id.0],
                    |row| row.get(0),
                )
                .expect("event type read");
            assert_eq!(event_type, "execution_trace_stage");
        }
        let page = store
            .execution_trace_page(session.id, 0, 100)
            .expect("trace projected");
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[1].event_type, "execution_trace_stage");
    }

    #[test]
    #[ignore = "10,000-event execution trace scale harness; run explicitly"]
    fn execution_trace_page_skips_large_text_delta_history() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("large trace"), 1)
            .expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 2, 20_000, 5)
            .expect("task claimed")
            .expect("assignment");
        for index in 0..10_000_u64 {
            store
                .append_assignment_event_at(
                    assignment.fence,
                    TaskSessionEventInput {
                        kind: TaskSessionEventKind::Runtime,
                        payload: json!({ "type": "text_delta", "text": format!("delta-{index}") }),
                        progress: None,
                    },
                    3 + index,
                )
                .expect("delta appended");
        }
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Runtime,
                    payload: json!({
                        "type": "execution_trace_stage",
                        "schema_version": 1,
                        "stage": "agent_runtime_request",
                        "duration_us": 1_000_000,
                        "outcome": "succeeded"
                    }),
                    progress: None,
                },
                10_003,
            )
            .expect("trace appended");

        let started = Instant::now();
        let page = store
            .execution_trace_page(session.id, 0, 100)
            .expect("trace projected");
        println!("10k execution trace page: {:?}", started.elapsed());
        assert_eq!(page.entries.len(), 3);
        assert_eq!(
            page.entries.last().map(|entry| entry.sequence),
            Some(10_003)
        );
        assert!(serde_json::to_string(&page)
            .expect("page encoded")
            .find("delta-")
            .is_none());
    }

    #[test]
    fn tool_state_projects_from_persisted_event_journal() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("tool-state"), 10)
            .expect("task enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 20, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Tool,
                    payload: serde_json::json!({
                        "type": "tool_started",
                        "tool_call_id": "tool-1",
                        "tool_name": "jira_search",
                        "risk": "low",
                        "arguments_digest": "abc",
                        "display_context": { "issue": "ABC-1" }
                    }),
                    progress: None,
                },
                21,
            )
            .expect("tool start appended");
        store
            .append_assignment_event_at(
                assignment.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Tool,
                    payload: serde_json::json!({
                        "type": "tool_completed",
                        "tool_call_id": "tool-1",
                        "tool_name": "jira_search",
                        "success": true,
                        "risk": "low",
                        "arguments_digest": "abc",
                        "display_context": { "issue": "ABC-1" }
                    }),
                    progress: None,
                },
                22,
            )
            .expect("tool completion appended");
        drop(store);

        let reopened = SchedulerStore::open_query_at(path).expect("query store opens");
        let state = reopened
            .tool_state(session.id)
            .expect("tool state projected");
        assert_eq!(state.session_id, session.id);
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.calls[0].tool_call_id, "tool-1");
        assert_eq!(state.calls[0].completed_sequence, Some(4));
    }

    #[test]
    fn mcp_context_projects_envelope_and_session_scoped_grants() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let first = store
            .enqueue_with_grants_at(
                &TaskRequest::from_envelope("first", &test_agent_envelope())
                    .expect("first envelope"),
                &["external_tools:jira".to_string()],
                "test-approval",
                10,
            )
            .expect("first enqueued");
        let second = store
            .enqueue_with_grants_at(
                &TaskRequest::from_envelope("second", &test_agent_envelope())
                    .expect("second envelope"),
                &[],
                "",
                11,
            )
            .expect("second enqueued");
        let owner = store.register_owner().expect("owner registered");
        let assignment = store
            .claim_next_at(owner, 1, 20, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");

        let first_context = store.mcp_context(first.id).expect("first context");
        let second_context = store.mcp_context(second.id).expect("second context");
        assert_eq!(first_context.session_id, first.id);
        assert_eq!(second_context.session_id, second.id);
        assert_eq!(
            first_context.active_attempt_id,
            Some(assignment.fence.attempt_id)
        );
        assert_eq!(first_context.fencing_token, assignment.fence.fencing_token);
        assert_eq!(second_context.active_attempt_id, None);
        assert_eq!(first_context.connectors.len(), 1);
        assert_eq!(second_context.connectors.len(), 1);
        assert_eq!(first_context.connectors[0].connector_id, "jira");
        assert!(first_context.connectors[0].granted);
        assert!(!second_context.connectors[0].granted);
    }

    #[test]
    fn claims_serialize_matching_ownership_without_blocking_unrelated_sessions() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let first = owned_agent_request("first", "conversation-a", "subject-a");
        let conflicting = owned_agent_request("conflicting", "conversation-a", "subject-b");
        let subject_conflict =
            owned_agent_request("subject-conflict", "conversation-c", "subject-a");
        let unrelated = owned_agent_request("unrelated", "conversation-b", "subject-c");
        store.enqueue_at(&first, 1).expect("first enqueued");
        store
            .enqueue_at(&conflicting, 2)
            .expect("conflicting enqueued");
        store
            .enqueue_at(&subject_conflict, 3)
            .expect("subject conflict enqueued");
        store.enqueue_at(&unrelated, 4).expect("unrelated enqueued");

        let first_assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");
        let unrelated_assignment = store
            .claim_next_at(owner, 2, 11, LEASE_MILLIS, 5)
            .expect("second claim")
            .expect("unrelated assignment");
        assert_eq!(unrelated_assignment.request.label, "unrelated");
        assert!(store
            .claim_next_at(owner, 3, 12, LEASE_MILLIS, 5)
            .expect("conflict checked")
            .is_none());

        store
            .resolve_assignment_at(
                first_assignment.fence,
                DurableOutcome::Succeeded(TaskExecutionOutput::None),
                20,
            )
            .expect("first finished");
        let conflicting_assignment = store
            .claim_next_at(owner, 3, 21, LEASE_MILLIS, 5)
            .expect("conflicting claim")
            .expect("conflicting assignment");
        assert_eq!(conflicting_assignment.request.label, "conflicting");
        let subject_assignment = store
            .claim_next_at(owner, 4, 22, LEASE_MILLIS, 5)
            .expect("subject claim")
            .expect("subject assignment");
        assert_eq!(subject_assignment.request.label, "subject-conflict");
    }

    #[test]
    fn structured_results_survive_staging_and_are_queryable_before_terminal() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        for (label, status, terminal) in [
            (
                "completed",
                AgentTaskCompletionStatus::Completed,
                TaskSessionState::Succeeded,
            ),
            (
                "blocked",
                AgentTaskCompletionStatus::Blocked,
                TaskSessionState::Blocked,
            ),
        ] {
            let request = owned_agent_request(label, &format!("conversation-{label}"), label);
            let session = store.enqueue_at(&request, 1).expect("session enqueued");
            let assignment = store
                .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
                .expect("session claimed")
                .expect("assignment");
            let output = agent_output(status);
            assert!(matches!(
                store
                    .resolve_assignment_at(
                        assignment.fence,
                        DurableOutcome::Succeeded(output.clone()),
                        20,
                    )
                    .expect("result staged"),
                FinishResult::Applied
            ));
            assert_eq!(
                store
                    .get_session(session.id)
                    .expect("session read")
                    .expect("session exists")
                    .state,
                TaskSessionState::Committing
            );
            let result = store
                .task_session_result(session.id)
                .expect("result queried")
                .expect("result exists");
            assert_eq!(result.output, output);
            assert_eq!(result.terminal_state, terminal);
        }
        drop(store);
        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        assert!(reopened
            .task_session_result(TaskSessionId(1))
            .expect("result survives")
            .is_some());
        assert!(reopened
            .task_session_result(TaskSessionId(2))
            .expect("result survives")
            .is_some());
    }

    #[test]
    fn cancellation_atomically_wins_over_structured_worker_completion() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(
                &owned_agent_request("cancel", "conversation-cancel", "subject-cancel"),
                1,
            )
            .expect("session enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("session claimed")
            .expect("assignment");
        store
            .cancel_at(session.id, 20)
            .expect("cancellation requested");

        assert!(matches!(
            store
                .resolve_assignment_at(
                    assignment.fence,
                    DurableOutcome::Succeeded(agent_output(AgentTaskCompletionStatus::Completed)),
                    21,
                )
                .expect("completion resolved"),
            FinishResult::Applied
        ));
        assert_eq!(
            store
                .get_session(session.id)
                .expect("session read")
                .expect("session exists")
                .state,
            TaskSessionState::Cancelled
        );
        assert!(store
            .task_session_result(session.id)
            .expect("result queried")
            .is_none());
    }

    #[test]
    fn projection_retry_backoff_is_exact_and_caps_at_thirty_seconds() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        store
            .enqueue_at(
                &owned_agent_request("retry", "conversation-retry", "subject-retry"),
                1,
            )
            .expect("session enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("session claimed")
            .expect("assignment");
        store
            .resolve_assignment_at(
                assignment.fence,
                DurableOutcome::Succeeded(agent_output(AgentTaskCompletionStatus::Completed)),
                20,
            )
            .expect("completion staged");
        let mut completion = store
            .due_pending_completions(20)
            .expect("pending queried")
            .pop()
            .expect("completion due");

        let mut now = 1_000;
        for attempt in 1..=12 {
            store
                .record_completion_error_at(&completion, "retry", now)
                .expect("error recorded");
            let delay = projection_retry_delay_millis(attempt);
            assert!(store
                .due_pending_completions(now.saturating_add(delay).saturating_sub(1))
                .expect("not-due queried")
                .is_empty());
            completion = store
                .due_pending_completions(now.saturating_add(delay))
                .expect("due queried")
                .pop()
                .expect("completion due exactly");
            now = now.saturating_add(delay);
        }
        assert_eq!(projection_retry_delay_millis(1), 100);
        assert_eq!(projection_retry_delay_millis(2), 200);
        assert_eq!(projection_retry_delay_millis(10), 30_000);
        assert_eq!(projection_retry_delay_millis(u32::MAX), 30_000);
    }

    #[test]
    fn stale_fence_cannot_stage_result_and_committing_keeps_ownership_locked() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let first = owned_agent_request("first", "conversation-a", "subject-a");
        let mut conflicting_envelope = owned_agent_request("second", "conversation-b", "subject-b")
            .envelope()
            .expect("envelope decoded")
            .expect("envelope exists");
        let TaskSessionEnvelope::V1(conflicting_session) = &mut conflicting_envelope else {
            unreachable!();
        };
        conflicting_session.execution_run_id = Some("run-first".to_string());
        let conflicting =
            TaskRequest::from_envelope("second", &conflicting_envelope).expect("request encoded");
        store.enqueue_at(&first, 1).expect("first enqueued");
        store.enqueue_at(&conflicting, 2).expect("second enqueued");
        let assignment = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claimed")
            .expect("assignment");
        let mut stale = assignment.fence;
        stale.fencing_token += 1;
        assert!(matches!(
            store
                .resolve_assignment_at(
                    stale,
                    DurableOutcome::Succeeded(agent_output(AgentTaskCompletionStatus::Completed)),
                    20
                )
                .expect("stale result rejected"),
            FinishResult::Stale
        ));
        store
            .resolve_assignment_at(
                assignment.fence,
                DurableOutcome::Succeeded(agent_output(AgentTaskCompletionStatus::Completed)),
                20,
            )
            .expect("result staged");
        assert!(store
            .claim_next_at(owner, 2, 21, LEASE_MILLIS, 5)
            .expect("ownership checked")
            .is_none());
    }

    #[test]
    fn stale_attempt_cannot_append_events_after_reclaim() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("reclaim"), 1)
            .expect("task enqueued");
        let stale = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("first claim")
            .expect("first assignment");
        assert_eq!(store.recover_expired_at(1_011).expect("recovered"), 1);
        let current = store
            .claim_next_at(owner, 2, 1_012, LEASE_MILLIS, 5)
            .expect("second claim")
            .expect("second assignment");
        let event = || TaskSessionEventInput {
            kind: TaskSessionEventKind::Runtime,
            payload: serde_json::json!({ "message": "attempt event" }),
            progress: None,
        };
        assert!(store
            .append_assignment_event_at(stale.fence, event(), 1_013)
            .is_err());
        store
            .append_assignment_event_at(current.fence, event(), 1_013)
            .expect("current event appended");
        let events = store.events_after(session.id, 0).expect("events read");
        assert!(events.iter().any(|record| {
            record.kind == TaskSessionEventKind::Runtime
                && record.attempt_id == Some(current.fence.attempt_id)
        }));
        assert!(!events.iter().any(|record| {
            record.kind == TaskSessionEventKind::Runtime
                && record.attempt_id == Some(stale.fence.attempt_id)
        }));
        assert!(store
            .append_assignment_event_at(
                current.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Lifecycle,
                    payload: serde_json::json!({ "state": "succeeded" }),
                    progress: None,
                },
                1_014,
            )
            .is_err());
    }

    #[test]
    fn owner_shutdown_is_distinct_from_lease_expiration() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_at(&TaskRequest::new("shutdown"), 1)
            .expect("task enqueued");
        store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        assert_eq!(store.abandon_owner(owner).expect("owner abandoned"), 1);
        let events = store.events_after(session.id, 0).expect("events read");
        assert_eq!(
            events.last().expect("requeue event").payload["reason"],
            "scheduler_owner_shutdown"
        );
    }

    #[test]
    fn capability_grants_are_explicit_and_survive_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let session_id = {
            let store = SchedulerStore::open_at(path.clone()).expect("store opens");
            let without_grants = store
                .enqueue_at(
                    &TaskRequest::with_payload(
                        "requested-only",
                        r#"{"requested_capabilities":["shell"]}"#,
                    ),
                    1,
                )
                .expect("request enqueued");
            assert!(store
                .capability_grants(without_grants.id)
                .expect("grants read")
                .is_empty());
            store
                .enqueue_with_grants_at(
                    &TaskRequest::new("granted"),
                    &["shell".to_string(), "workspace_read".to_string()],
                    "test-approval",
                    2,
                )
                .expect("granted request enqueued")
                .id
        };

        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        let grants = reopened
            .capability_grants(session_id)
            .expect("grants restored");
        assert_eq!(
            grants
                .iter()
                .map(|grant| grant.capability.as_str())
                .collect::<Vec<_>>(),
            vec!["shell", "workspace_read"]
        );
        assert!(grants
            .iter()
            .all(|grant| grant.grant_source == "test-approval" && grant.granted_at == 2));
    }

    #[test]
    fn task_tool_authority_rejects_stale_and_denied_without_cross_session_cancellation() {
        let directory = tempdir().expect("temp directory");
        let store =
            SchedulerStore::open_at(directory.path().join("scheduler.db")).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let first = store
            .enqueue_with_grants(
                &owned_agent_request("tools-1", "conversation-tools-1", "card-tools-1"),
                &["shell".to_string()],
                "test-approval",
            )
            .expect("first task enqueued");
        let second = store
            .enqueue_with_grants(
                &owned_agent_request("tools-2", "conversation-tools-2", "card-tools-2"),
                &["shell".to_string()],
                "test-approval",
            )
            .expect("second task enqueued");
        let first_assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 5)
            .expect("first claim")
            .expect("first assignment");
        let second_assignment = store
            .claim_next(owner, 2, Duration::from_secs(30), 5)
            .expect("second claim")
            .expect("second assignment");
        let root = directory.path().canonicalize().expect("workspace root");
        let first_authority = store
            .task_tool_authority(
                first_assignment.fence,
                "workspace-personal",
                root.clone(),
                &["shell".to_string()],
            )
            .expect("first authority");
        let second_authority = store
            .task_tool_authority(
                second_assignment.fence,
                "workspace-personal",
                root,
                &["shell".to_string()],
            )
            .expect("second authority");

        assert!(
            SchedulerStore::task_tool_authority_is_current(&first_authority, "shell")
                .expect("first authority checked")
        );
        assert!(!SchedulerStore::task_tool_authority_is_current(
            &first_authority,
            "workspace_write"
        )
        .expect("denied capability checked"));
        store.cancel(first.id).expect("first cancelled");
        assert!(
            !SchedulerStore::task_tool_authority_is_current(&first_authority, "shell")
                .expect("stale authority checked")
        );
        assert!(
            SchedulerStore::task_tool_authority_is_current(&second_authority, "shell")
                .expect("second authority remains current")
        );
        assert_eq!(second_assignment.fence.session_id, second.id);
    }

    #[test]
    fn assignment_authority_rejects_wrong_stale_and_cancelling_attempts() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let first_session = store
            .enqueue_at(&TaskRequest::new("authority"), 1)
            .expect("task enqueued");
        let first = store
            .claim_next_at(owner, 1, 10, LEASE_MILLIS, 5)
            .expect("task claimed")
            .expect("assignment");
        assert!(store
            .assignment_is_current_at(first.fence, 20)
            .expect("authority checked"));
        assert!(!store
            .assignment_is_current_at(
                AssignmentFence {
                    fencing_token: first.fence.fencing_token + 1,
                    ..first.fence
                },
                20,
            )
            .expect("wrong fence checked"));
        store
            .cancel_at(first_session.id, 30)
            .expect("cancellation requested");
        assert!(!store
            .assignment_is_current_at(first.fence, 31)
            .expect("cancelling authority checked"));
        assert!(store
            .append_assignment_event_at(
                first.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Progress,
                    payload: serde_json::json!({ "message": "too late" }),
                    progress: Some(TaskProgress {
                        phase: "late".to_string(),
                        completed: 1,
                        total: Some(1),
                    }),
                },
                31,
            )
            .is_err());

        let second_session = store
            .enqueue_at(&TaskRequest::new("reclaimed-authority"), 40)
            .expect("second task enqueued");
        store
            .resolve_assignment_at(first.fence, DurableOutcome::Cancelled, 41)
            .expect("first task cancelled");
        let stale = store
            .claim_next_at(owner, 2, 50, LEASE_MILLIS, 5)
            .expect("second task claimed")
            .expect("second assignment");
        assert_eq!(stale.fence.session_id, second_session.id);
        assert!(store
            .append_assignment_event_at(
                stale.fence,
                TaskSessionEventInput {
                    kind: TaskSessionEventKind::Activity,
                    payload: serde_json::json!({ "message": "expired" }),
                    progress: None,
                },
                1_051,
            )
            .is_err());
        assert!(matches!(
            store
                .resolve_assignment_at(
                    stale.fence,
                    DurableOutcome::Succeeded(TaskExecutionOutput::None),
                    1_051,
                )
                .expect("expired finish checked"),
            FinishResult::Stale
        ));
        assert_eq!(store.recover_expired_at(1_051).expect("recovered"), 1);
        let current = store
            .claim_next_at(owner, 3, 1_052, LEASE_MILLIS, 5)
            .expect("task reclaimed")
            .expect("current assignment");
        assert!(!store
            .assignment_is_current_at(stale.fence, 1_053)
            .expect("stale authority checked"));
        assert!(store
            .assignment_is_current_at(current.fence, 1_053)
            .expect("current authority checked"));
    }

    #[test]
    fn approval_resumes_the_same_durable_opencode_session_across_workers_and_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let initial = owned_agent_request("initial", "conversation-resume", "subject-resume");
        let session = store.enqueue(&initial).expect("task enqueued");
        let first = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("first claim")
            .expect("first assignment");
        let created = store
            .bind_opencode_session(first.fence, "opencode-session-x")
            .expect("OpenCode identity bound");
        assert_eq!(created.payload["action"], json!("created"));
        store
            .resolve_assignment(
                first.fence,
                DurableOutcome::Blocked(
                    "[approval_required] restart deployment needs approval".to_string(),
                ),
            )
            .expect("approval pause persisted");

        let second_request =
            owned_agent_request("approved-once", "conversation-resume", "subject-resume");
        let resumed = store
            .resume_after_approval(session.id, &second_request, &[], "test_approval")
            .expect("approval resumed");
        assert_eq!(resumed.id, session.id);
        assert_eq!(
            resumed.opencode_session_id.as_deref(),
            Some("opencode-session-x")
        );
        drop(store);

        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        let second = reopened
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("second claim")
            .expect("second assignment");
        assert_eq!(second.fence.session_id, session.id);
        assert_eq!(second.fence.attempt, 2);
        assert_eq!(
            reopened
                .assignment_opencode_session(second.fence)
                .expect("identity loaded")
                .as_deref(),
            Some("opencode-session-x")
        );
        assert!(reopened
            .bind_opencode_session(first.fence, "opencode-session-x")
            .is_err());
        let resumed_event = reopened
            .bind_opencode_session(second.fence, "opencode-session-x")
            .expect("same identity resumed on another worker");
        assert_eq!(resumed_event.payload["action"], json!("resumed"));
        assert_eq!(resumed_event.payload["worker_id"], json!(3));
        assert!(reopened
            .bind_opencode_session(second.fence, "opencode-session-y")
            .is_err());

        reopened
            .resolve_assignment(
                second.fence,
                DurableOutcome::Blocked("[approval_required] scale needs approval".to_string()),
            )
            .expect("second approval pause persisted");
        let third_request =
            owned_agent_request("approved-twice", "conversation-resume", "subject-resume");
        reopened
            .resume_after_approval(session.id, &third_request, &[], "test_approval")
            .expect("second approval resumed");
        let third = reopened
            .claim_next(owner, 4, Duration::from_secs(5), 5)
            .expect("third claim")
            .expect("third assignment");
        assert_eq!(third.fence.attempt, 3);
        reopened
            .bind_opencode_session(third.fence, "opencode-session-x")
            .expect("identity resumed for the second approval cycle");
        reopened
            .resolve_assignment(
                third.fence,
                DurableOutcome::Succeeded(TaskExecutionOutput::None),
            )
            .expect("task completed");

        let snapshot = reopened
            .get_session(session.id)
            .expect("session loaded")
            .expect("session exists");
        assert_eq!(snapshot.state, TaskSessionState::Succeeded);
        assert_eq!(
            snapshot.opencode_session_id.as_deref(),
            Some("opencode-session-x")
        );
        let events = reopened
            .events_after(session.id, 0)
            .expect("events replayed");
        let created_count = events
            .iter()
            .filter(|event| event.payload["action"] == json!("created"))
            .count();
        assert_eq!(created_count, 1);
        assert!(events.iter().any(|event| {
            event.payload["action"] == json!("paused")
                && event.payload["opencode_session_id"] == json!("opencode-session-x")
        }));
        assert!(events.iter().any(|event| {
            event.payload["action"] == json!("resumed_after_approval")
                && event.payload["opencode_session_id"] == json!("opencode-session-x")
        }));
        assert!(events.iter().any(|event| {
            event.payload["action"] == json!("terminal")
                && event.payload["opencode_session_id"] == json!("opencode-session-x")
        }));
    }

    #[test]
    fn generic_continuation_reuses_interrupted_session_but_cannot_bypass_approval() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("blocked", "conversation-blocked", "subject-blocked");
        let session = store.enqueue(&request).expect("task enqueued");
        let first = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("task claim")
            .expect("assignment");
        store
            .bind_opencode_session(first.fence, "opencode-session-blocked")
            .expect("session identity bound");
        store
            .resolve_assignment(
                first.fence,
                DurableOutcome::Blocked("operator input required".to_string()),
            )
            .expect("task blocked");

        let continued_request =
            owned_agent_request("continued", "conversation-blocked", "subject-blocked");
        let continued = store
            .continue_interrupted_session(session.id, &continued_request, &[], "test_continuation")
            .expect("task continued");
        assert_eq!(continued.id, session.id);
        assert_eq!(
            continued.opencode_session_id.as_deref(),
            Some("opencode-session-blocked")
        );
        let second = store
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("continued claim")
            .expect("continued assignment");
        assert_eq!(second.fence.attempt, 2);
        assert_eq!(
            store
                .assignment_opencode_session(second.fence)
                .expect("continued identity"),
            Some("opencode-session-blocked".to_string())
        );

        store
            .resolve_assignment(
                second.fence,
                DurableOutcome::Blocked("[approval_required] restart".to_string()),
            )
            .expect("approval pause");
        assert!(store
            .continue_interrupted_session(session.id, &continued_request, &[], "test_continuation",)
            .expect_err("generic continuation must not bypass approval")
            .contains("structured UI approval"));
    }

    #[test]
    fn continuation_without_a_durable_opencode_session_requires_retry_fresh() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("missing", "conversation-missing", "subject-missing");
        let session = store.enqueue(&request).expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("task claim")
            .expect("assignment");
        store
            .resolve_assignment(
                assignment.fence,
                DurableOutcome::Failed("runtime failed before session creation".to_string()),
            )
            .expect("task failed");
        assert!(store
            .continue_interrupted_session(session.id, &request, &[], "test_continuation")
            .expect_err("missing identity must not silently create a session")
            .contains("Retry Fresh"));
    }

    #[test]
    fn failed_task_with_a_durable_opencode_session_can_continue() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request("failed", "conversation-failed", "subject-failed");
        let session = store.enqueue(&request).expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("task claim")
            .expect("assignment");
        store
            .bind_opencode_session(assignment.fence, "opencode-session-failed")
            .expect("session identity bound");
        store
            .resolve_assignment(
                assignment.fence,
                DurableOutcome::Failed("recoverable provider interruption".to_string()),
            )
            .expect("task failed");

        let continued_request =
            owned_agent_request("continued-failure", "conversation-failed", "subject-failed");
        let continued = store
            .continue_interrupted_session(session.id, &continued_request, &[], "test_continuation")
            .expect("failed task continued");
        assert_eq!(continued.id, session.id);
        assert_eq!(continued.state, TaskSessionState::Queued);
        assert_eq!(
            continued.opencode_session_id.as_deref(),
            Some("opencode-session-failed")
        );
    }

    #[test]
    fn concurrent_task_sessions_keep_distinct_opencode_identities() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let task_a = store
            .enqueue(&owned_agent_request("a", "conversation-a", "subject-a"))
            .expect("task A enqueued");
        let task_b = store
            .enqueue(&owned_agent_request("b", "conversation-b", "subject-b"))
            .expect("task B enqueued");
        let assignment_a = store
            .claim_next(owner, 1, Duration::from_secs(5), 5)
            .expect("task A claim")
            .expect("task A assignment");
        let assignment_b = store
            .claim_next(owner, 2, Duration::from_secs(5), 5)
            .expect("task B claim")
            .expect("task B assignment");
        assert_eq!(assignment_a.fence.session_id, task_a.id);
        assert_eq!(assignment_b.fence.session_id, task_b.id);
        store
            .bind_opencode_session(assignment_a.fence, "opencode-session-a")
            .expect("task A identity bound");
        store
            .bind_opencode_session(assignment_b.fence, "opencode-session-b")
            .expect("task B identity bound");
        assert_eq!(
            store
                .assignment_opencode_session(assignment_a.fence)
                .expect("task A identity"),
            Some("opencode-session-a".to_string())
        );
        assert_eq!(
            store
                .assignment_opencode_session(assignment_b.fence)
                .expect("task B identity"),
            Some("opencode-session-b".to_string())
        );
    }

    fn test_execution_manifest_draft(runtime_id: &str) -> ExecutionManifestDraft {
        ExecutionManifestDraft {
            kind: TaskSessionKind::Agent,
            workspace_id: "workspace-personal".to_string(),
            subject_id: None,
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "sha256:contract".to_string(),
            context_revision: Some("context-1".to_string()),
            runtime: "opencode".to_string(),
            runtime_profile_id: "profile-1".to_string(),
            runtime_id: runtime_id.to_string(),
            model: "openai/gpt-5".to_string(),
            model_configuration: ExecutionModelConfiguration {
                provider_id: "openai".to_string(),
                api_style: "responses".to_string(),
                temperature: "0.2".to_string(),
            },
            prompt_template_version: "prompt-v1".to_string(),
            rules_revision: Some("rules-1".to_string()),
            skills_revision: Some("skills-1".to_string()),
            rules: vec![],
            rules_digest: "sha256:rules".to_string(),
            rule_facts: Default::default(),
            task_examination: Default::default(),
            skills_catalog_revision: Some("sha256:skills".to_string()),
            skills: vec![],
            connectors: vec![TaskMcpConnectorContext {
                connector_id: "jira".to_string(),
                capability: "external_tools:jira".to_string(),
                requested: true,
                granted: true,
            }],
            tool_permission_mode: "fenced_tools_only".to_string(),
            unknown_fields: vec!["git_revision".to_string()],
        }
    }

    fn test_execution_manifest_with_prepared_subtasks(runtime_id: &str) -> ExecutionManifestDraft {
        let mut draft = test_execution_manifest_draft(runtime_id);
        let sensitive_evidence = "page exists with sensitive-evidence-never-persist-this";
        let prepared_subtasks = prepare_subtask_contracts(
            &json!({
                "semantic_plan": {"objectives": [
                    {"id": "inspect-page", "summary": "Inspect Jira issue", "success_evidence": sensitive_evidence, "operation_hints": ["read issue"], "resource_hints": ["jira issue"], "mutation_expected": false},
                    {"id": "apply-change", "summary": "Update Jira issue", "success_evidence": "deployment exists", "operation_hints": ["update issue"], "resource_hints": ["jira issue"], "mutation_expected": true}
                ]},
                "capability_plan": {"connectors": [{
                    "connector_id": "jira",
                    "matched_domains": ["jira"],
                    "matched_intents": [],
                    "matched_tools": ["jira_get_issue", "jira_update_issue"]
                }]}
            }),
            &draft.context_digest,
            &["external_tools:jira".to_string()],
        )
        .expect("subtask contracts prepared");
        draft.task_examination = TaskExaminationRecord {
            schema_version: TASK_EXAMINATION_SCHEMA_VERSION,
            examiner_version: TASK_EXAMINER_VERSION.to_string(),
            contract_digest: draft.context_digest.clone(),
            status: TaskExaminationStatus::Ready,
            capability_catalog: vec![TaskCapabilityRecord {
                capability: "external_tools:jira".to_string(),
                provider: "jira".to_string(),
                connector_id: Some("jira".to_string()),
                discovery: "configured".to_string(),
                granted: true,
            }],
            semantic_planner: Some(SemanticPlannerEvidence {
                status: "model".to_string(),
                planner_version: "test-planner-v1".to_string(),
                model: Some("openai/gpt-5".to_string()),
                objective_count: prepared_subtasks.len(),
            }),
            prepared_subtasks,
            required_capabilities: vec!["external_tools:jira".to_string()],
            ..TaskExaminationRecord::default()
        };
        draft
    }

    #[test]
    fn prepared_subtasks_are_atomic_idempotent_dormant_and_restart_safe() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("subtasks", &test_agent_envelope())
                    .expect("request encodes"),
                &["external_tools:jira".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        let draft = test_execution_manifest_with_prepared_subtasks("runtime-safe");

        store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("manifest and dormant subtasks bind atomically");
        let first = store
            .prepared_subtasks_for_session(session.id)
            .expect("prepared subtasks load");
        assert_eq!(first.len(), 2);
        assert_ne!(first[0].fence.subtask_id, first[1].fence.subtask_id);
        assert_ne!(
            first[0].fence.subtask_attempt_id,
            first[1].fence.subtask_attempt_id
        );
        for subtask in &first {
            assert_eq!(subtask.state, "prepared");
            assert_eq!(subtask.fence.attempt, 1);
            assert_eq!(subtask.fence.fencing_token, 1);
            assert_eq!(subtask.tool_calls_used, 0);
            assert_eq!(subtask.mutation_calls_used, 0);
            assert!(!subtask.authority_active);
            assert!(!subtask.contract.execution_enabled);
            assert!(store
                .dormant_subtask_fence_exists(session.id, subtask.fence)
                .expect("exact fence checks"));
            assert!(!store
                .dormant_subtask_fence_exists(
                    session.id,
                    DormantSubtaskFence {
                        fencing_token: subtask.fence.fencing_token + 1,
                        ..subtask.fence
                    },
                )
                .expect("stale fence checks"));
        }
        assert!(!store
            .dormant_subtask_fence_exists(
                session.id,
                DormantSubtaskFence {
                    subtask_attempt_id: first[1].fence.subtask_attempt_id,
                    ..first[0].fence
                },
            )
            .expect("mixed fence checks"));

        store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("identical retry remains idempotent");
        assert_eq!(
            store
                .prepared_subtasks_for_session(session.id)
                .expect("retried subtasks load"),
            first
        );

        let database = fs::read(&path).expect("scheduler database reads");
        assert!(
            !String::from_utf8_lossy(&database).contains("sensitive-evidence-never-persist-this")
        );
        drop(store);
        let reopened = SchedulerStore::open_query_at(path).expect("query store reopens");
        assert_eq!(
            reopened
                .prepared_subtasks_for_session(session.id)
                .expect("reopened subtasks load"),
            first
        );
    }

    #[test]
    fn prepared_subtask_rebind_rejects_incompatible_scheduler_state() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("subtasks", &test_agent_envelope())
                    .expect("request encodes"),
                &["external_tools:jira".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        let draft = test_execution_manifest_with_prepared_subtasks("runtime-safe");
        store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("initial bind succeeds");
        store
            .connection
            .lock()
            .expect("scheduler connection")
            .execute(
                "UPDATE scheduler_subtask_attempts SET max_tool_calls = max_tool_calls + 1
                  WHERE subtask_id = (
                    SELECT MIN(subtask_id) FROM scheduler_prepared_subtasks WHERE session_id = ?1
                  )",
                params![to_i64(session.id.0).expect("session ID fits")],
            )
            .expect("test corrupts dormant allocation");
        assert!(store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect_err("incompatible scheduler state fails closed")
            .contains("changed unexpectedly"));
    }

    #[test]
    fn prepared_subtask_allocation_failure_rolls_back_the_manifest() {
        let store = SchedulerStore::open_in_memory().expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("subtasks", &test_agent_envelope())
                    .expect("request encodes"),
                &["external_tools:jira".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        store
            .connection
            .lock()
            .expect("scheduler connection")
            .execute(
                "INSERT INTO scheduler_prepared_subtasks
                   (session_id, contract_id, objective_id, contract_json, state,
                    execution_enabled, created_from_attempt_id, created_at)
                 VALUES (?1, 'rogue-contract', 'rogue-objective', '{}', 'prepared', 0, ?2, 1)",
                params![
                    to_i64(session.id.0).expect("session ID fits"),
                    to_i64(assignment.fence.attempt_id).expect("attempt ID fits")
                ],
            )
            .expect("test inserts incompatible allocation");

        let draft = test_execution_manifest_with_prepared_subtasks("runtime-safe");
        assert!(store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect_err("allocation mismatch fails")
            .contains("set changed"));
        assert_eq!(
            store
                .latest_execution_manifest(session.id)
                .expect("manifest lookup succeeds"),
            None,
            "the manifest insert must roll back with the failed allocation",
        );
    }

    fn active_subtask_test_context(
        path: PathBuf,
    ) -> (
        SchedulerStore,
        TaskSessionId,
        AssignmentFence,
        Vec<SchedulerPreparedSubtask>,
    ) {
        let store = SchedulerStore::open_at(path).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("subtasks", &test_agent_envelope())
                    .expect("request encodes"),
                &["external_tools:jira".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(30), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        store
            .bind_execution_manifest(
                assignment.fence,
                &test_execution_manifest_with_prepared_subtasks("runtime-safe"),
            )
            .expect("prepared subtasks bind");
        let subtasks = store
            .prepared_subtasks_for_session(session.id)
            .expect("prepared subtasks load");
        (store, session.id, assignment.fence, subtasks)
    }

    fn admit_test_subtask_connector_call(
        authority: &SubtaskToolAuthority,
        capability: &str,
        risk: SubtaskToolRisk,
    ) -> Result<SubtaskToolAdmission, String> {
        let tool_name = authority
            .allowed_connector_tools
            .get(capability)
            .and_then(|tools| tools.first())
            .expect("test connector authority includes an exact tool operation");
        SchedulerStore::admit_subtask_connector_tool_call(authority, capability, tool_name, risk)
    }

    #[test]
    fn narrowed_builtin_subtask_contract_is_enforced_at_scheduler_admission() {
        let directory = tempdir().expect("temp directory");
        let store =
            SchedulerStore::open_at(directory.path().join("scheduler.db")).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let mut envelope = test_agent_envelope();
        let TaskSessionEnvelope::V1(agent) = &mut envelope else {
            unreachable!("test envelope is Agent V1");
        };
        agent.connector_ids.clear();
        let requested_capabilities = vec![
            "workspace_read".to_string(),
            "workspace_write".to_string(),
            "shell".to_string(),
            "git".to_string(),
        ];
        agent.requested_capabilities = requested_capabilities.clone();
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("builtin-subtasks", &envelope)
                    .expect("request encodes"),
                &requested_capabilities,
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(30), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        let prepared_subtasks = prepare_subtask_contracts(
            &json!({"semantic_plan": {"objectives": [
                {
                    "id": "inspect-file",
                    "summary": "Inspect source file",
                    "success_evidence": "Source file is observed",
                    "operation_hints": ["read file"],
                    "resource_hints": ["workspace source"],
                    "mutation_expected": false
                },
                {
                    "id": "modify-file",
                    "summary": "Modify source file",
                    "success_evidence": "Source file is updated",
                    "operation_hints": ["write file"],
                    "resource_hints": ["workspace source"],
                    "mutation_expected": true
                }
            ]}}),
            "sha256:contract",
            &requested_capabilities,
        )
        .expect("subtask contracts prepare");
        let mut draft = test_execution_manifest_draft("runtime-builtins");
        draft.connectors.clear();
        draft.task_examination = TaskExaminationRecord {
            schema_version: TASK_EXAMINATION_SCHEMA_VERSION,
            examiner_version: TASK_EXAMINER_VERSION.to_string(),
            contract_digest: draft.context_digest.clone(),
            status: TaskExaminationStatus::Ready,
            capability_catalog: requested_capabilities
                .iter()
                .map(|capability| TaskCapabilityRecord {
                    capability: capability.clone(),
                    provider: "workspace".to_string(),
                    connector_id: None,
                    discovery: "configured".to_string(),
                    granted: true,
                })
                .collect(),
            semantic_planner: Some(SemanticPlannerEvidence {
                status: "model".to_string(),
                planner_version: "test-planner-v1".to_string(),
                model: Some("openai/gpt-5".to_string()),
                objective_count: prepared_subtasks.len(),
            }),
            prepared_subtasks,
            required_capabilities: requested_capabilities,
            ..TaskExaminationRecord::default()
        };
        store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("manifest binds");
        let subtasks = store
            .prepared_subtasks_for_session(session.id)
            .expect("prepared subtasks load");
        let permit = SchedulerStore::test_subtask_dispatch_permit();
        let read_only = store
            .activate_prepared_subtask(
                assignment.fence,
                subtasks[0].fence,
                Duration::from_secs(20),
                &permit,
            )
            .expect("read-only authority activates");
        assert_eq!(read_only.capabilities, vec!["workspace_read"]);
        SchedulerStore::admit_subtask_tool_call(
            &read_only,
            "workspace_read",
            SubtaskToolRisk::Read,
        )
        .expect("read is admitted");
        assert!(SchedulerStore::admit_subtask_tool_call(
            &read_only,
            "workspace_write",
            SubtaskToolRisk::Mutation,
        )
        .expect_err("read-only objective cannot write")
        .contains("shape or capability is invalid"));

        let mutable = store
            .activate_prepared_subtask(
                assignment.fence,
                subtasks[1].fence,
                Duration::from_secs(20),
                &permit,
            )
            .expect("mutable authority activates");
        assert_eq!(
            mutable.capabilities,
            vec!["workspace_read", "workspace_write"]
        );
        SchedulerStore::admit_subtask_tool_call(
            &mutable,
            "workspace_write",
            SubtaskToolRisk::Mutation,
        )
        .expect("bounded write is admitted");
    }

    #[test]
    fn subtask_activation_is_fenced_idempotent_and_budgeted_across_reopen() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let (store, session_id, parent, subtasks) = active_subtask_test_context(path.clone());
        let permit = SchedulerStore::test_subtask_dispatch_permit();
        let read_only = store
            .activate_prepared_subtask(parent, subtasks[0].fence, Duration::from_secs(20), &permit)
            .expect("read-only subtask activates");
        assert_eq!(
            store
                .activate_prepared_subtask(
                    parent,
                    subtasks[0].fence,
                    Duration::from_secs(20),
                    &permit,
                )
                .expect("identical activation is idempotent"),
            read_only
        );
        assert_eq!(
            read_only.allowed_connector_tools.get("external_tools:jira"),
            Some(&vec!["jira_get_issue".to_string()])
        );
        assert!(SchedulerStore::admit_subtask_connector_tool_call(
            &read_only,
            "external_tools:jira",
            "jira_update_issue",
            SubtaskToolRisk::Mutation,
        )
        .expect_err("unlisted connector operation is rejected before budget admission")
        .contains("not granted by its objective contract"));
        assert!(store
            .activate_prepared_subtask(
                parent,
                DormantSubtaskFence {
                    fencing_token: subtasks[0].fence.fencing_token + 1,
                    ..subtasks[0].fence
                },
                Duration::from_secs(20),
                &permit,
            )
            .expect_err("stale dormant fence fails")
            .contains("stale or incompatible"));

        assert!(admit_test_subtask_connector_call(
            &read_only,
            "external_tools:jira",
            SubtaskToolRisk::Mutation,
        )
        .expect_err("read-only objective has no mutation authority")
        .contains("mutation-call budget is exhausted"));
        let first = admit_test_subtask_connector_call(
            &read_only,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect("first read admitted");
        assert_eq!(first.tool_calls_used, 1);
        assert_eq!(first.mutation_calls_used, 0);

        let mutable = store
            .activate_prepared_subtask(parent, subtasks[1].fence, Duration::from_secs(20), &permit)
            .expect("mutation subtask activates independently");
        assert_eq!(
            mutable.allowed_connector_tools.get("external_tools:jira"),
            Some(&vec!["jira_update_issue".to_string()])
        );
        for expected in 1..=subtasks[1].contract.budget.max_mutation_calls {
            let admitted = admit_test_subtask_connector_call(
                &mutable,
                "external_tools:jira",
                SubtaskToolRisk::Mutation,
            )
            .expect("bounded mutation admitted");
            assert_eq!(admitted.mutation_calls_used, expected);
        }
        assert!(admit_test_subtask_connector_call(
            &mutable,
            "external_tools:jira",
            SubtaskToolRisk::Mutation,
        )
        .expect_err("mutation budget remains a separate hard limit")
        .contains("mutation-call budget is exhausted"));
        let post_mutation_read = admit_test_subtask_connector_call(
            &mutable,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect("remaining general tool budget still admits reads");
        assert_eq!(
            post_mutation_read.tool_calls_used,
            subtasks[1].contract.budget.max_mutation_calls + 1
        );
        assert_eq!(
            post_mutation_read.mutation_calls_used,
            subtasks[1].contract.budget.max_mutation_calls
        );

        drop(store);
        let second = admit_test_subtask_connector_call(
            &read_only,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect("budget survives process reopen");
        assert_eq!(second.tool_calls_used, 2);
        let reopened = SchedulerStore::open_at(path).expect("store reopens");
        let mut expanded = read_only.clone();
        expanded
            .capabilities
            .push("external_tools:other".to_string());
        expanded.capabilities.sort();
        expanded.allowed_connector_tools.insert(
            "external_tools:other".to_string(),
            vec!["other_read".to_string()],
        );
        assert!(admit_test_subtask_connector_call(
            &expanded,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("capability expansion fails")
        .contains("immutable contract"));
        let mut stale_authority = read_only.clone();
        stale_authority.authority_fencing_token += 1;
        assert!(admit_test_subtask_connector_call(
            &stale_authority,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("stale authority fence fails")
        .contains("stale, expired, or incompatible"));
        let mut altered_lease = read_only.clone();
        altered_lease.lease_expires_at += 1;
        assert!(admit_test_subtask_connector_call(
            &altered_lease,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("altered authority lease fails")
        .contains("stale, expired, or incompatible"));
        reopened
            .connection
            .lock()
            .expect("scheduler connection")
            .execute(
                "DELETE FROM scheduler_task_grants
                  WHERE session_id = ?1 AND capability = 'external_tools:jira'",
                params![to_i64(session_id.0).expect("session ID fits")],
            )
            .expect("test revokes parent capability");
        assert!(admit_test_subtask_connector_call(
            &read_only,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("revoked parent grant invalidates retained subtask authority")
        .contains("stale, expired, or incompatible"));
        reopened
            .connection
            .lock()
            .expect("scheduler connection")
            .execute(
                "INSERT INTO scheduler_task_grants
                   (session_id, capability, grant_source, granted_at)
                 VALUES (?1, 'external_tools:jira', 'test-restored', 1)",
                params![to_i64(session_id.0).expect("session ID fits")],
            )
            .expect("test restores parent capability");
        reopened
            .cancel(session_id)
            .expect("parent cancellation starts");
        assert!(admit_test_subtask_connector_call(
            &read_only,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("cancelled parent revokes effective subtask authority")
        .contains("parent assignment is stale"));
    }

    #[test]
    fn concurrent_subtask_admission_never_exceeds_the_contract_budget() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let (store, _session_id, parent, subtasks) = active_subtask_test_context(path);
        let permit = SchedulerStore::test_subtask_dispatch_permit();
        let authority = store
            .activate_prepared_subtask(parent, subtasks[0].fence, Duration::from_secs(30), &permit)
            .expect("subtask activates");
        let expected_budget = subtasks[0].contract.budget.max_tool_calls as usize;
        let barrier = Arc::new(Barrier::new(expected_budget + 9));
        let handles = (0..expected_budget + 8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let authority = authority.clone();
                thread::spawn(move || {
                    barrier.wait();
                    admit_test_subtask_connector_call(
                        &authority,
                        "external_tools:jira",
                        SubtaskToolRisk::Read,
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let admitted = handles
            .into_iter()
            .map(|handle| handle.join().expect("admission thread joins"))
            .filter(Result::is_ok)
            .count();
        assert_eq!(admitted, expected_budget);
        assert!(admit_test_subtask_connector_call(
            &authority,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("budget remains exhausted")
        .contains("budget is exhausted"));
    }

    #[test]
    fn subtask_renewal_and_terminal_resolution_are_exact_and_restart_safe() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let (store, _session_id, parent, subtasks) = active_subtask_test_context(path.clone());
        let permit = SchedulerStore::test_subtask_dispatch_permit();
        let authority = store
            .activate_prepared_subtask(parent, subtasks[0].fence, Duration::from_secs(5), &permit)
            .expect("subtask activates");
        assert_eq!(
            store
                .recover_subtask_authorities()
                .expect("live authority needs no recovery"),
            0
        );
        let renewed = SchedulerStore::renew_subtask_authority(&authority, Duration::from_secs(20))
            .expect("exact lease renews");
        let now = now_millis();
        assert!(renewed.lease_expires_at > authority.lease_expires_at);
        assert!(admit_test_subtask_connector_call(
            &authority,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("old lease descriptor becomes stale")
        .contains("stale, expired, or incompatible"));
        assert!(
            SchedulerStore::renew_subtask_authority_at(&authority, now, 25_000)
                .expect_err("old lease cannot renew twice")
                .contains("exact lease fence")
        );

        let admission = admit_test_subtask_connector_call(
            &renewed,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect("renewed authority admits tools");
        assert_eq!(admission.tool_calls_used, 1);
        let completed =
            SchedulerStore::resolve_subtask_authority(&renewed, SubtaskAuthorityOutcome::Completed)
                .expect("subtask completes");
        assert_eq!(completed.state, "completed");
        assert_eq!(completed.terminal_reason.as_deref(), Some("completed"));
        assert_eq!(completed.tool_calls_used, 1);
        assert_eq!(completed.lease_expires_at, None);
        assert_eq!(
            SchedulerStore::resolve_subtask_authority_at(
                &renewed,
                SubtaskAuthorityOutcome::Completed,
                now + 2,
            )
            .expect("identical completion replay is idempotent"),
            completed
        );
        assert!(SchedulerStore::resolve_subtask_authority_at(
            &renewed,
            SubtaskAuthorityOutcome::Failed,
            now + 2,
        )
        .expect_err("different terminal outcome conflicts")
        .contains("different terminal outcome"));
        let mut forged = renewed.clone();
        forged.subtask_id += 1;
        assert!(SchedulerStore::resolve_subtask_authority_at(
            &forged,
            SubtaskAuthorityOutcome::Completed,
            now + 2,
        )
        .expect_err("forged terminal replay fails")
        .contains("descriptor is stale"));
        assert!(admit_test_subtask_connector_call(
            &renewed,
            "external_tools:jira",
            SubtaskToolRisk::Read,
        )
        .expect_err("completed authority cannot call tools")
        .contains("stale, expired, or incompatible"));

        let cancelled_authority = store
            .activate_prepared_subtask(parent, subtasks[1].fence, Duration::from_secs(10), &permit)
            .expect("second subtask activates");
        let cancelled = SchedulerStore::resolve_subtask_authority(
            &cancelled_authority,
            SubtaskAuthorityOutcome::Cancelled,
        )
        .expect("explicit cancellation revokes authority");
        assert_eq!(cancelled.state, "revoked");
        assert_eq!(cancelled.terminal_reason.as_deref(), Some("cancelled"));

        drop(store);
        let reopened = SchedulerStore::open_query_at(path).expect("query store reopens");
        assert_eq!(
            reopened
                .subtask_authority_status(completed.authority_id)
                .expect("status loads after restart"),
            Some(completed)
        );
    }

    #[test]
    fn subtask_expiry_cancellation_and_parent_resolution_revoke_durably() {
        let expiry_directory = tempdir().expect("expiry directory");
        let (expiry_store, _session_id, expiry_parent, expiry_subtasks) =
            active_subtask_test_context(expiry_directory.path().join("scheduler.db"));
        let permit = SchedulerStore::test_subtask_dispatch_permit();
        let expiring = expiry_store
            .activate_prepared_subtask(
                expiry_parent,
                expiry_subtasks[0].fence,
                Duration::from_secs(5),
                &permit,
            )
            .expect("expiring subtask activates");
        assert!(SchedulerStore::resolve_subtask_authority_at(
            &expiring,
            SubtaskAuthorityOutcome::Completed,
            expiring.lease_expires_at,
        )
        .expect_err("expired authority cannot claim completion")
        .contains("cannot report completion"));
        assert_eq!(
            expiry_store
                .recover_subtask_authorities_at(expiring.lease_expires_at)
                .expect("expired authority recovers"),
            1
        );
        let expired = expiry_store
            .subtask_authority_status(expiring.authority_id)
            .expect("expired status loads")
            .expect("expired status exists");
        assert_eq!(expired.state, "revoked");
        assert_eq!(expired.terminal_reason.as_deref(), Some("lease_expired"));
        assert_eq!(
            expiry_store
                .recover_subtask_authorities_at(expiring.lease_expires_at + 1)
                .expect("recovery is idempotent"),
            0
        );

        let cancel_directory = tempdir().expect("cancel directory");
        let (cancel_store, cancel_session, cancel_parent, cancel_subtasks) =
            active_subtask_test_context(cancel_directory.path().join("scheduler.db"));
        let cancelled = cancel_store
            .activate_prepared_subtask(
                cancel_parent,
                cancel_subtasks[0].fence,
                Duration::from_secs(20),
                &SchedulerStore::test_subtask_dispatch_permit(),
            )
            .expect("cancelled subtask activates");
        cancel_store
            .cancel_at(cancel_session, now_millis())
            .expect("parent cancellation starts");
        let cancelled_status = cancel_store
            .subtask_authority_status(cancelled.authority_id)
            .expect("cancelled status loads")
            .expect("cancelled status exists");
        assert_eq!(cancelled_status.state, "revoked");
        assert_eq!(
            cancelled_status.terminal_reason.as_deref(),
            Some("parent_cancelled")
        );

        let resolved_directory = tempdir().expect("resolved directory");
        let (resolved_store, _resolved_session, resolved_parent, resolved_subtasks) =
            active_subtask_test_context(resolved_directory.path().join("scheduler.db"));
        let resolved = resolved_store
            .activate_prepared_subtask(
                resolved_parent,
                resolved_subtasks[0].fence,
                Duration::from_secs(20),
                &SchedulerStore::test_subtask_dispatch_permit(),
            )
            .expect("resolved subtask activates");
        resolved_store
            .resolve_assignment_at(
                resolved_parent,
                DurableOutcome::Failed("parent failed".to_string()),
                now_millis(),
            )
            .expect("parent resolves");
        let resolved_status = resolved_store
            .subtask_authority_status(resolved.authority_id)
            .expect("resolved status loads")
            .expect("resolved status exists");
        assert_eq!(resolved_status.state, "revoked");
        assert_eq!(
            resolved_status.terminal_reason.as_deref(),
            Some("parent_resolved")
        );

        let direct_cancel_directory = tempdir().expect("direct cancel directory");
        let (
            direct_cancel_store,
            _direct_cancel_session,
            direct_cancel_parent,
            direct_cancel_subtasks,
        ) = active_subtask_test_context(direct_cancel_directory.path().join("scheduler.db"));
        let directly_cancelled = direct_cancel_store
            .activate_prepared_subtask(
                direct_cancel_parent,
                direct_cancel_subtasks[0].fence,
                Duration::from_secs(20),
                &SchedulerStore::test_subtask_dispatch_permit(),
            )
            .expect("directly cancelled subtask activates");
        direct_cancel_store
            .resolve_assignment_at(
                direct_cancel_parent,
                DurableOutcome::Cancelled,
                now_millis(),
            )
            .expect("parent reports cancellation");
        let directly_cancelled_status = direct_cancel_store
            .subtask_authority_status(directly_cancelled.authority_id)
            .expect("direct cancellation status loads")
            .expect("direct cancellation status exists");
        assert_eq!(directly_cancelled_status.state, "revoked");
        assert_eq!(
            directly_cancelled_status.terminal_reason.as_deref(),
            Some("parent_cancelled")
        );

        let owner_directory = tempdir().expect("owner directory");
        let (owner_store, _owner_session, owner_parent, owner_subtasks) =
            active_subtask_test_context(owner_directory.path().join("scheduler.db"));
        let abandoned = owner_store
            .activate_prepared_subtask(
                owner_parent,
                owner_subtasks[0].fence,
                Duration::from_secs(20),
                &SchedulerStore::test_subtask_dispatch_permit(),
            )
            .expect("owner subtask activates");
        owner_store
            .abandon_owner(owner_parent.owner_id)
            .expect("scheduler owner shuts down");
        let abandoned_status = owner_store
            .subtask_authority_status(abandoned.authority_id)
            .expect("abandoned status loads")
            .expect("abandoned status exists");
        assert_eq!(abandoned_status.state, "revoked");
        assert_eq!(
            abandoned_status.terminal_reason.as_deref(),
            Some("parent_inactive")
        );
    }

    #[test]
    fn execution_manifest_is_fenced_idempotent_and_secret_free() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(path.clone()).expect("store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope("manifest", &test_agent_envelope())
                    .expect("request encodes"),
                &["external_tools:jira".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 3, Duration::from_secs(5), 5)
            .expect("claim succeeds")
            .expect("assignment exists");
        let draft = test_execution_manifest_draft("runtime-safe");
        let first = store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("manifest binds");
        let second = store
            .bind_execution_manifest(assignment.fence, &draft)
            .expect("identical bind is idempotent");
        assert_eq!(first, second);
        assert_eq!(
            store
                .latest_execution_manifest(session.id)
                .expect("manifest loads"),
            Some(first.clone())
        );
        let mut conflicting = draft.clone();
        conflicting.model_configuration.temperature = "0.9".to_string();
        assert!(store
            .bind_execution_manifest(assignment.fence, &conflicting)
            .expect_err("conflicting manifest is rejected")
            .contains("different Execution Manifest"));

        store
            .bind_opencode_session(assignment.fence, "opencode-safe")
            .expect("runtime identity binds");
        let encoded = serde_json::to_string(
            &store
                .latest_execution_manifest(session.id)
                .expect("manifest loads")
                .expect("manifest exists"),
        )
        .expect("manifest encodes");
        assert!(!encoded.contains("opencode-safe"));
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("environment"));

        let stale = AssignmentFence {
            fencing_token: assignment.fence.fencing_token + 1,
            ..assignment.fence
        };
        assert!(store.bind_execution_manifest(stale, &draft).is_err());
        let reopened = SchedulerStore::open_query_at(path).expect("query store reopens");
        assert_eq!(
            reopened
                .latest_execution_manifest(session.id)
                .expect("reopened manifest loads"),
            Some(first)
        );
    }

    fn test_agent_envelope() -> TaskSessionEnvelope {
        TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "sha256:contract".to_string(),
            runtime_profile_id: "profile-1".to_string(),
            model: "openai/gpt-5".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec!["external_tools:jira".to_string()],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: Some("context-1".to_string()),
            rules_revision: Some("rules-1".to_string()),
            skills_revision: Some("skills-1".to_string()),
        })
    }

    fn owned_agent_request(label: &str, conversation_id: &str, subject_id: &str) -> TaskRequest {
        let mut envelope = test_agent_envelope();
        let TaskSessionEnvelope::V1(session) = &mut envelope else {
            unreachable!("test envelope is V1");
        };
        session.conversation_id = Some(conversation_id.to_string());
        session.subject_id = Some(subject_id.to_string());
        session.execution_run_id = Some(format!("run-{label}"));
        TaskRequest::from_envelope(label, &envelope).expect("owned request")
    }

    fn scale_identity(replicas: u32) -> ResourceOperationIdentity {
        ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "scale_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("default".to_string()),
                name: "api".to_string(),
            },
            "https://cluster.example:6443",
            &json!({ "replicas": replicas }),
        )
        .expect("scale identity")
    }

    fn restart_identity(token: &str) -> ResourceOperationIdentity {
        ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "restart_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("default".to_string()),
                name: "api".to_string(),
            },
            "https://cluster.example:6443",
            &json!({ "restart_token": token }),
        )
        .expect("restart identity")
    }

    fn scale_evidence(identity: &ResourceOperationIdentity) -> ResourceMutationEvidence {
        ResourceMutationEvidence {
            identity: identity.clone(),
            lookup: ResourceLookupResult {
                status: ResourceLookupStatus::DriftDetected,
                observed_fingerprint: Some(identity.mutation_fingerprint.clone()),
                observed_version: Some("10".to_string()),
            },
            execution: ResourceExecutionResult {
                status: ResourceExecutionStatus::Executed,
                resulting_fingerprint: Some(identity.mutation_fingerprint.clone()),
                resulting_version: Some("11".to_string()),
            },
            retry_resume_status: ResourceRetryResumeStatus::ReconciledAfterDrift,
        }
    }

    fn jira_comment_identity() -> ResourceOperationIdentity {
        crate::infrastructure::jira::jira_comment_operation_identity(
            &"a".repeat(64),
            "jira_add_comment",
            &json!({"issue_key": "OPS-42", "comment": "private completion detail"}),
        )
        .unwrap()
        .unwrap()
    }

    fn jira_comment_mutation_evidence(
        identity: &ResourceOperationIdentity,
    ) -> ResourceMutationEvidence {
        ResourceMutationEvidence {
            identity: identity.clone(),
            lookup: ResourceLookupResult {
                status: ResourceLookupStatus::DriftDetected,
                observed_fingerprint: None,
                observed_version: None,
            },
            execution: ResourceExecutionResult {
                status: ResourceExecutionStatus::Executed,
                resulting_fingerprint: Some(identity.mutation_fingerprint.clone()),
                resulting_version: Some("10042".to_string()),
            },
            retry_resume_status: ResourceRetryResumeStatus::FirstExecution,
        }
    }

    fn external_mutation_assignment(
        path: PathBuf,
        worker_id: usize,
    ) -> (SchedulerStore, TaskSessionId, ExternalAssignmentAuthority) {
        let store = SchedulerStore::open_at(path).expect("persistent store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::new(format!("resource-mutation-{worker_id}")),
                &["external_tools:ocp".to_string()],
                "test-approval",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, worker_id, Duration::from_secs(30), 5)
            .expect("task claimed")
            .expect("assignment exists");
        let authority = store
            .external_authority(
                assignment.fence,
                "external_tools:ocp",
                "ocp",
                &"a".repeat(64),
            )
            .expect("external authority");
        (store, session.id, authority)
    }

    fn agent_external_mutation_assignment(
        path: PathBuf,
        worker_id: usize,
    ) -> (SchedulerStore, TaskSessionId, ExternalAssignmentAuthority) {
        let store = SchedulerStore::open_at(path).expect("persistent store opens");
        let owner = store.register_owner().expect("owner registered");
        let request = owned_agent_request(
            "long-running deployment",
            "conversation-recovery",
            "subject-recovery",
        );
        let session = store
            .enqueue_with_grants(
                &request,
                &["external_tools:ocp".to_string()],
                "test-approval",
            )
            .expect("Agent task enqueued");
        let assignment = store
            .claim_next(owner, worker_id, Duration::from_secs(30), 5)
            .expect("Agent task claimed")
            .expect("assignment exists");
        store
            .bind_opencode_session(assignment.fence, "opencode-long-running-recovery")
            .expect("durable runtime identity bound");
        let authority = store
            .external_authority(
                assignment.fence,
                "external_tools:ocp",
                "ocp",
                &"a".repeat(64),
            )
            .expect("external authority");
        (store, session.id, authority)
    }

    fn assignment_fence(authority: &ExternalAssignmentAuthority) -> AssignmentFence {
        AssignmentFence {
            session_id: authority.session_id,
            attempt_id: authority.attempt_id,
            attempt: authority.attempt,
            owner_id: authority.owner_id,
            fencing_token: authority.fencing_token,
        }
    }

    fn resource_mutation_receipt(
        tool_call_id: &str,
        operation_key: &str,
    ) -> AgentTaskObjectiveToolReceipt {
        AgentTaskObjectiveToolReceipt {
            tool_call_id: tool_call_id.to_string(),
            tool_name: "ocp_scale_deployment".to_string(),
            risk: "mutation".to_string(),
            arguments_digest: "a".repeat(64),
            resource_operation_key: Some(operation_key.to_string()),
            external_resource: None,
        }
    }

    fn restart_mutation_receipt(
        tool_call_id: &str,
        operation_key: &str,
    ) -> AgentTaskObjectiveToolReceipt {
        AgentTaskObjectiveToolReceipt {
            tool_call_id: tool_call_id.to_string(),
            tool_name: "ocp_restart_deployment".to_string(),
            risk: "mutation".to_string(),
            arguments_digest: "b".repeat(64),
            resource_operation_key: Some(operation_key.to_string()),
            external_resource: None,
        }
    }

    fn bamboo_trigger_receipt(result_key: &str) -> AgentTaskObjectiveToolReceipt {
        AgentTaskObjectiveToolReceipt {
            tool_call_id: "bamboo-call-1".to_string(),
            tool_name: "corporate_bamboo_trigger_build".to_string(),
            risk: "mutation".to_string(),
            arguments_digest: "c".repeat(64),
            resource_operation_key: None,
            external_resource: Some(crate::domain::task_session::ExternalResourceReference {
                provider: "bamboo".to_string(),
                resource_kind: "build".to_string(),
                resource_id: result_key.to_string(),
                parent_resource_id: None,
                state_fingerprint: None,
            }),
        }
    }

    fn jira_comment_receipt(comment_id: &str) -> AgentTaskObjectiveToolReceipt {
        AgentTaskObjectiveToolReceipt {
            tool_call_id: "jira-comment-call-1".to_string(),
            tool_name: "corporate_jira_add_comment".to_string(),
            risk: "mutation".to_string(),
            arguments_digest: "d".repeat(64),
            resource_operation_key: None,
            external_resource: Some(crate::domain::task_session::ExternalResourceReference {
                provider: "jira".to_string(),
                resource_kind: "comment".to_string(),
                resource_id: comment_id.to_string(),
                parent_resource_id: Some("OPS-42".to_string()),
                state_fingerprint: Some("e".repeat(64)),
            }),
        }
    }

    #[test]
    fn jira_comment_identity_persists_replays_and_rejects_malformed_receipts() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let receipt = jira_comment_receipt("10042");
        let evidence = vec!["Jira comment 10042 verified".to_string()];
        store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-comment",
                &evidence,
                std::slice::from_ref(&receipt),
            )
            .expect("comment checkpoint recorded");
        let retained = store
            .objective_checkpoints(session_id)
            .expect("checkpoints read");
        assert_eq!(retained[0].tool_receipts, vec![receipt.clone()]);
        let replay = store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-comment",
                &evidence,
                &[receipt],
            )
            .expect("exact replay accepted");
        assert_eq!(replay.payload["new_checkpoint"], false);

        let mut malformed = jira_comment_receipt("10043");
        malformed
            .external_resource
            .as_mut()
            .expect("external resource")
            .state_fingerprint = Some("raw-comment-content".to_string());
        assert!(store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-malformed-comment",
                &evidence,
                &[malformed],
            )
            .expect_err("raw content is not a valid fingerprint")
            .contains("payload is invalid"));

        let mut missing = jira_comment_receipt("10044");
        missing.external_resource = None;
        assert!(store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-missing-comment",
                &evidence,
                &[missing],
            )
            .expect_err("trusted comment tool requires resource evidence")
            .contains("payload is invalid"));
    }

    #[test]
    fn bamboo_trigger_identity_persists_and_replays_with_checkpoint() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let receipt = bamboo_trigger_receipt("PAYROLL-DEPLOY-42");
        let evidence = vec!["Bamboo result PAYROLL-DEPLOY-42 succeeded".to_string()];

        let first = store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-build",
                &evidence,
                std::slice::from_ref(&receipt),
            )
            .expect("Bamboo checkpoint recorded");
        assert_eq!(first.payload["new_checkpoint"], true);
        let retained = store
            .objective_checkpoints(session_id)
            .expect("checkpoints read");
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].tool_receipts, vec![receipt.clone()]);

        let replay = store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-build",
                &evidence,
                &[receipt],
            )
            .expect("exact replay accepted");
        assert_eq!(replay.payload["new_checkpoint"], false);
    }

    #[test]
    fn bamboo_checkpoint_rejects_missing_or_malformed_trigger_identity() {
        let directory = tempdir().expect("temp directory");
        let (store, _session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let evidence = vec!["Build observed".to_string()];
        let mut missing = bamboo_trigger_receipt("PAYROLL-DEPLOY-42");
        missing.external_resource = None;
        assert!(store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-missing",
                &evidence,
                &[missing],
            )
            .expect_err("missing identity rejected")
            .contains("payload is invalid"));

        let malformed = bamboo_trigger_receipt("not a result key");
        assert!(store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-malformed",
                &evidence,
                &[malformed],
            )
            .expect_err("malformed identity rejected")
            .contains("payload is invalid"));
    }

    fn succeeded_scale_mutation(
        authority: &ExternalAssignmentAuthority,
        identity: &ResourceOperationIdentity,
    ) -> ResourceMutationRecord {
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            authority,
            "ocp_scale_deployment",
            identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("mutation was unexpectedly blocked"),
        };
        SchedulerStore::resolve_external_resource_mutation(
            authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(scale_evidence(identity)),
        )
        .expect("mutation succeeded")
    }

    #[test]
    fn objective_checkpoint_atomically_binds_succeeded_resource_receipt_and_replays_exactly() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = scale_identity(3);
        let succeeded = succeeded_scale_mutation(&authority, &identity);
        let fence = assignment_fence(&authority);
        let receipt = resource_mutation_receipt("scale-call-1", &identity.key);
        let evidence = vec!["Deployment api has three replicas".to_string()];

        let checkpoint = store
            .record_objective_checkpoint(
                fence,
                "objective-1",
                &evidence,
                std::slice::from_ref(&receipt),
            )
            .expect("checkpoint recorded");
        assert_eq!(checkpoint.payload["new_checkpoint"], true);
        let bound = store
            .resource_mutation(succeeded.mutation_id)
            .expect("mutation read")
            .expect("mutation exists");
        assert_eq!(
            bound.checkpoint_objective_id.as_deref(),
            Some("objective-1")
        );
        assert_eq!(
            bound.checkpoint_tool_call_id.as_deref(),
            Some("scale-call-1")
        );
        assert!(bound.checkpoint_recorded_at.is_some());
        assert_eq!(bound.revision, 3);
        assert!(matches!(
            SchedulerStore::reserve_external_resource_mutation(
                &authority,
                "ocp_scale_deployment",
                &identity,
            )
            .expect("bound mutation fence checked"),
            ResourceMutationReservation::Blocked(ResourceMutationRecord {
                mutation_id,
                checkpoint_objective_id: Some(_),
                ..
            }) if mutation_id == succeeded.mutation_id
        ));

        let replay = store
            .record_objective_checkpoint(
                fence,
                "objective-1",
                &evidence,
                std::slice::from_ref(&receipt),
            )
            .expect("exact checkpoint replay accepted");
        assert_eq!(replay.payload["new_checkpoint"], false);
        assert_eq!(
            store
                .resource_mutation(succeeded.mutation_id)
                .expect("mutation read")
                .expect("mutation exists")
                .revision,
            3
        );
        assert!(store
            .record_objective_checkpoint(fence, "objective-2", &evidence, &[receipt])
            .expect_err("different objective binding rejected")
            .contains("already bound"));
        assert_eq!(
            store
                .objective_checkpoints(session_id)
                .expect("checkpoints read")
                .len(),
            1
        );
    }

    #[test]
    fn restart_mutation_uses_distinct_token_identity_and_binds_checkpoint() {
        let directory = tempdir().expect("temp directory");
        let (store, _session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = restart_identity("11111111-1111-4111-8111-111111111111");
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_restart_deployment",
            &identity,
        )
        .expect("restart reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("restart unexpectedly blocked"),
        };
        SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(scale_evidence(&identity)),
        )
        .expect("restart succeeded");
        store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-1",
                &["Deployment restart token observed".to_string()],
                &[restart_mutation_receipt("restart-call-1", &identity.key)],
            )
            .expect("restart checkpoint bound");

        let bound = store
            .resource_mutation(reserved.mutation_id)
            .expect("restart mutation read")
            .expect("restart mutation exists");
        assert_eq!(
            bound.checkpoint_objective_id.as_deref(),
            Some("objective-1")
        );
        assert_eq!(bound.tool_name, "ocp_restart_deployment");
        assert_ne!(
            identity.key,
            restart_identity("22222222-2222-4222-8222-222222222222").key
        );
        assert!(matches!(
            SchedulerStore::reserve_external_resource_mutation(
                &authority,
                "ocp_restart_deployment",
                &identity,
            )
            .expect("bound restart fence checked"),
            ResourceMutationReservation::Blocked(_)
        ));
    }

    #[test]
    fn objective_checkpoint_rejects_cross_session_or_non_succeeded_resource_receipts() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let (_first_store, _first_session, first_authority) =
            external_mutation_assignment(path.clone(), 1);
        let (second_store, _second_session, second_authority) =
            external_mutation_assignment(path, 2);
        let succeeded_identity = scale_identity(3);
        succeeded_scale_mutation(&first_authority, &succeeded_identity);
        let evidence = vec!["Deployment state verified".to_string()];
        let cross_session = resource_mutation_receipt("scale-call-cross", &succeeded_identity.key);
        assert!(second_store
            .record_objective_checkpoint(
                assignment_fence(&second_authority),
                "objective-cross",
                &evidence,
                &[cross_session],
            )
            .expect_err("cross-session binding rejected")
            .contains("did not match a succeeded ledger record"));

        let reserved_identity = scale_identity(4);
        SchedulerStore::reserve_external_resource_mutation(
            &second_authority,
            "ocp_scale_deployment",
            &reserved_identity,
        )
        .expect("second mutation reserved");
        let reserved = resource_mutation_receipt("scale-call-reserved", &reserved_identity.key);
        assert!(second_store
            .record_objective_checkpoint(
                assignment_fence(&second_authority),
                "objective-reserved",
                &evidence,
                &[reserved],
            )
            .expect_err("reserved binding rejected")
            .contains("did not match a succeeded ledger record"));
    }

    #[test]
    fn succeeded_resource_mutation_starts_single_state_reconciliation() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = scale_identity(3);
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("first mutation was blocked"),
        };
        let succeeded = SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(scale_evidence(&identity)),
        )
        .expect("mutation succeeded");
        assert_eq!(succeeded.state, ResourceMutationState::Succeeded);
        assert_eq!(succeeded.revision, 2);
        let reconciled = SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("state reconciliation reserved");
        assert!(matches!(
            reconciled,
            ResourceMutationReservation::Reserved(ResourceMutationRecord {
                state: ResourceMutationState::Reserved,
                ..
            })
        ));
        let superseded = store
            .resource_mutation(succeeded.mutation_id)
            .expect("prior mutation read")
            .expect("prior mutation retained");
        assert_eq!(superseded.state, ResourceMutationState::Superseded);
        assert_eq!(superseded.revision, 3);
        assert_eq!(
            superseded.supersede_reason.as_deref(),
            Some("automatic_state_reconciliation")
        );
        let events = store.events_after(session_id, 0).expect("events read");
        assert!(events.iter().any(|event| {
            event.payload["type"] == "resource_mutation_reserved"
                && event.payload["reconciles_mutation_id"] == succeeded.mutation_id
        }));
        assert_eq!(
            store
                .resource_mutations_for_session(session_id)
                .expect("mutation history read")
                .len(),
            2
        );
    }

    #[test]
    fn confirmed_jira_comment_is_replayable_but_unconfirmed_intent_remains_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _, authority) =
            external_mutation_assignment(directory.path().join("jira-ledger.db"), 1);
        let identity = jira_comment_identity();
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "jira_add_comment",
            &identity,
        )
        .unwrap()
        {
            ResourceMutationReservation::Reserved(record) => record,
            other => panic!("unexpected reservation: {other:?}"),
        };
        let succeeded = SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(jira_comment_mutation_evidence(&identity)),
        )
        .unwrap();
        assert_eq!(succeeded.state, ResourceMutationState::Succeeded);
        assert!(matches!(
            SchedulerStore::reserve_external_resource_mutation(
                &authority,
                "jira_add_comment",
                &identity,
            )
            .unwrap(),
            ResourceMutationReservation::Blocked(ResourceMutationRecord {
                state: ResourceMutationState::Succeeded,
                ..
            })
        ));
        let mut receipt = jira_comment_receipt("10042");
        receipt.tool_name = "jira_add_comment".to_string();
        receipt.resource_operation_key = Some(identity.key.clone());
        store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "jira-comment-objective",
                &["Exact Jira comment confirmed.".to_string()],
                &[receipt],
            )
            .unwrap();
        let bound = store
            .resource_mutation(succeeded.mutation_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            bound.checkpoint_objective_id.as_deref(),
            Some("jira-comment-objective")
        );

        let different = crate::infrastructure::jira::jira_comment_operation_identity(
            &"a".repeat(64),
            "jira_add_comment",
            &json!({"issue_key": "OPS-42", "comment": "different detail"}),
        )
        .unwrap()
        .unwrap();
        let pending = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "jira_add_comment",
            &different,
        )
        .unwrap()
        {
            ResourceMutationReservation::Reserved(record) => record,
            other => panic!("unexpected reservation: {other:?}"),
        };
        SchedulerStore::resolve_external_resource_mutation(
            &authority,
            pending.mutation_id,
            ResourceMutationResolution::Uncertain {
                evidence: None,
                kind: "transport".to_string(),
                code: "response_lost".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(
            SchedulerStore::reserve_external_resource_mutation(
                &authority,
                "jira_add_comment",
                &different,
            )
            .unwrap(),
            ResourceMutationReservation::Blocked(ResourceMutationRecord {
                state: ResourceMutationState::Uncertain,
                ..
            })
        ));
        let encoded = serde_json::to_string(
            &store
                .resource_mutations_for_session(authority.session_id)
                .unwrap(),
        )
        .unwrap();
        assert!(!encoded.contains("private completion detail"));
        assert!(!encoded.contains("different detail"));
    }

    #[test]
    fn failed_resource_mutation_releases_fence_but_stale_authority_is_rejected() {
        let directory = tempdir().expect("temp directory");
        let (_store, _session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = scale_identity(3);
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("first mutation was blocked"),
        };
        let failed = SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Failed {
                evidence: None,
                kind: "forbidden".to_string(),
                code: "rbac_denied".to_string(),
            },
        )
        .expect("failure recorded");
        assert_eq!(failed.state, ResourceMutationState::Failed);
        assert!(matches!(
            SchedulerStore::reserve_external_resource_mutation(
                &authority,
                "ocp_scale_deployment",
                &identity
            )
            .expect("retry reservation allowed"),
            ResourceMutationReservation::Reserved(_)
        ));
        let stale = ExternalAssignmentAuthority {
            fencing_token: authority.fencing_token + 1,
            ..authority
        };
        assert!(SchedulerStore::reserve_external_resource_mutation(
            &stale,
            "ocp_scale_deployment",
            &scale_identity(4)
        )
        .is_err());
    }

    #[test]
    fn unresolved_resource_mutation_blocks_recovery_before_reassignment() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            agent_external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = scale_identity(3);
        let mutation_id = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record.mutation_id,
            ResourceMutationReservation::Blocked(_) => panic!("first mutation was blocked"),
        };
        assert_eq!(
            store
                .abandon_owner(authority.owner_id)
                .expect("owner abandoned"),
            1
        );
        let record = store
            .resource_mutation(mutation_id)
            .expect("mutation read")
            .expect("mutation exists");
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert_eq!(record.failure_kind.as_deref(), Some("lifecycle"));
        assert_eq!(
            record.failure_code.as_deref(),
            Some("scheduler_owner_shutdown")
        );
        assert_eq!(
            store
                .resource_mutations_for_session(session_id)
                .expect("session mutations read")
                .len(),
            1
        );
        let recovered = store
            .get_session(session_id)
            .expect("session read")
            .expect("session exists");
        assert_eq!(recovered.state, TaskSessionState::Blocked);
        assert_eq!(
            recovered.opencode_session_id.as_deref(),
            Some("opencode-long-running-recovery")
        );
        assert_eq!(
            recovered.error.as_deref(),
            Some(RECOVERY_REQUIRES_MUTATION_RECONCILIATION)
        );
        assert_eq!(
            store
                .capability_grants(session_id)
                .expect("grants retained")
                .iter()
                .map(|grant| grant.capability.as_str())
                .collect::<Vec<_>>(),
            vec!["external_tools:ocp"]
        );
        let events = store.events_after(session_id, 0).expect("events read");
        let recovery = events.last().expect("recovery event");
        assert_eq!(recovery.payload["state"], "blocked");
        assert_eq!(recovery.payload["reason"], "recovery_uncertain_mutation");
        assert_eq!(recovery.payload["recovery"], "operator_reconciliation");
        assert_eq!(recovery.payload["action"], "operator_reconciliation");
        assert_eq!(recovery.payload["uncertain_mutation_count"], 1);
        let next_owner = store
            .register_owner()
            .expect("replacement owner registered");
        assert!(store
            .claim_next(next_owner, 2, Duration::from_secs(30), 5)
            .expect("reassignment checked")
            .is_none());
    }

    #[test]
    fn expired_lease_with_unresolved_mutation_blocks_before_reassignment() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            agent_external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let mutation_id = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &scale_identity(3),
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record.mutation_id,
            ResourceMutationReservation::Blocked(_) => panic!("first mutation was blocked"),
        };
        {
            let connection = store.connection.lock().expect("store locked");
            connection
                .execute(
                    "UPDATE scheduler_task_attempts SET lease_expires_at = 1 WHERE attempt_id = ?1",
                    params![authority.attempt_id],
                )
                .expect("attempt lease expired");
            connection
                .execute(
                    "UPDATE scheduler_task_sessions SET lease_expires_at = 1 WHERE session_id = ?1",
                    params![session_id.0],
                )
                .expect("session lease expired");
        }

        assert_eq!(store.recover_expired_at(2).expect("lease recovered"), 1);
        let recovered = store
            .get_session(session_id)
            .expect("session read")
            .expect("session exists");
        assert_eq!(recovered.state, TaskSessionState::Blocked);
        assert_eq!(
            recovered.error.as_deref(),
            Some(RECOVERY_REQUIRES_MUTATION_RECONCILIATION)
        );
        let mutation = store
            .resource_mutation(mutation_id)
            .expect("mutation read")
            .expect("mutation exists");
        assert_eq!(mutation.state, ResourceMutationState::Uncertain);
        assert_eq!(
            mutation.failure_code.as_deref(),
            Some("assignment_lease_expired")
        );
        let next_owner = store
            .register_owner()
            .expect("replacement owner registered");
        assert!(store
            .claim_next(next_owner, 2, Duration::from_secs(30), 5)
            .expect("reassignment checked")
            .is_none());
    }

    #[test]
    fn concurrent_equivalent_reservations_create_one_active_fence() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("scheduler.db");
        let (_first_store, _first_session, first) = external_mutation_assignment(path.clone(), 1);
        let (_second_store, _second_session, second) = external_mutation_assignment(path, 2);
        let identity = scale_identity(3);
        let barrier = Arc::new(Barrier::new(2));
        let handles = [first, second].map(|authority| {
            let barrier = barrier.clone();
            let identity = identity.clone();
            thread::spawn(move || {
                barrier.wait();
                SchedulerStore::reserve_external_resource_mutation(
                    &authority,
                    "ocp_scale_deployment",
                    &identity,
                )
                .expect("reservation completes")
            })
        });
        let results = handles.map(|handle| handle.join().expect("reservation thread joins"));
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, ResourceMutationReservation::Reserved(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, ResourceMutationReservation::Blocked(_)))
                .count(),
            1
        );
    }

    #[test]
    fn resource_mutation_storage_contains_only_fingerprints_and_safe_metadata() {
        let directory = tempdir().expect("temp directory");
        let (store, _session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let secret = "sensitive-cluster-token";
        let identity = ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "scale_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("default".to_string()),
                name: "api".to_string(),
            },
            secret,
            &json!({ "replicas": 3, "token": secret }),
        )
        .expect("secret-free identity");
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("mutation was unexpectedly blocked"),
        };
        SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(scale_evidence(&identity)),
        )
        .expect("mutation succeeded");
        store
            .record_objective_checkpoint(
                assignment_fence(&authority),
                "objective-secret-free",
                &["Deployment scale verified".to_string()],
                &[resource_mutation_receipt(
                    "scale-call-secret-free",
                    &identity.key,
                )],
            )
            .expect("checkpoint bound");
        let connection = store.connection.lock().expect("store lock");
        let (identity_json, objective_id, tool_call_id): (String, String, String) = connection
            .query_row(
                "SELECT identity_json, checkpoint_objective_id, checkpoint_tool_call_id
                   FROM scheduler_resource_mutations LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("mutation binding read");
        let receipts_json: String = connection
            .query_row(
                "SELECT tool_receipts_json FROM scheduler_task_objective_checkpoints LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("checkpoint receipts read");
        let encoded = format!("{identity_json}{objective_id}{tool_call_id}{receipts_json}");
        assert!(!encoded.contains(secret));
        assert!(!encoded.contains("cluster.example"));
        assert!(encoded.contains("sha256:"));
    }

    #[test]
    fn terminal_session_removal_cannot_silently_erase_retained_mutation_fence() {
        let directory = tempdir().expect("temp directory");
        let (store, session_id, authority) =
            external_mutation_assignment(directory.path().join("scheduler.db"), 1);
        let identity = scale_identity(3);
        let reserved = match SchedulerStore::reserve_external_resource_mutation(
            &authority,
            "ocp_scale_deployment",
            &identity,
        )
        .expect("mutation reserved")
        {
            ResourceMutationReservation::Reserved(record) => record,
            ResourceMutationReservation::Blocked(_) => panic!("first mutation was blocked"),
        };
        let succeeded = SchedulerStore::resolve_external_resource_mutation(
            &authority,
            reserved.mutation_id,
            ResourceMutationResolution::Succeeded(scale_evidence(&identity)),
        )
        .expect("mutation succeeded");
        store
            .resolve_assignment(
                AssignmentFence {
                    session_id,
                    attempt_id: authority.attempt_id,
                    attempt: authority.attempt,
                    owner_id: authority.owner_id,
                    fencing_token: authority.fencing_token,
                },
                DurableOutcome::Succeeded(TaskExecutionOutput::None),
            )
            .expect("assignment completed");
        assert!(!store
            .remove_terminal(session_id)
            .expect("retained session removal checked"));
        store
            .supersede_resource_mutation(
                session_id,
                succeeded.mutation_id,
                &identity.key,
                succeeded.revision,
                "Operator explicitly released the retained fence.",
            )
            .expect("retained fence superseded");
        assert!(store
            .remove_terminal(session_id)
            .expect("superseded session removed"));
    }

    #[test]
    #[ignore = "repeatable performance harness; run explicitly with --ignored --nocapture"]
    fn performance_baseline_sqlite_task_sessions() {
        crate::infrastructure::performance::reset();
        let store = SchedulerStore::open_in_memory().expect("benchmark store");
        for index in 0..1_000 {
            store
                .enqueue(&owned_agent_request(
                    &format!("benchmark-{index}"),
                    &format!("conversation-{index}"),
                    &format!("subject-{index}"),
                ))
                .expect("benchmark session enqueued");
        }
        let sessions = store.list_sessions().expect("benchmark sessions listed");
        for session in sessions.iter().rev().take(100) {
            store
                .get_session(session.id)
                .expect("benchmark session loaded");
            store
                .event_page(session.id, 0, 100)
                .expect("benchmark journal page loaded");
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::infrastructure::performance::snapshot())
                .expect("benchmark snapshot encoded")
        );
    }

    fn agent_output(status: AgentTaskCompletionStatus) -> TaskExecutionOutput {
        TaskExecutionOutput::Agent(crate::domain::task_session::AgentTaskResult {
            summary: format!("{status:?} result"),
            evidence: vec!["evidence".to_string()],
            details: vec!["details".to_string()],
            next: vec!["next".to_string()],
            completion_status: status,
            blocked_reason: (status == AgentTaskCompletionStatus::Blocked)
                .then(|| "approval required".to_string()),
            objective_results: Vec::new(),
        })
    }
}
