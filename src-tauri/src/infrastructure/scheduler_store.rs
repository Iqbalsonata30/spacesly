//! SQLite persistence for the concurrent execution Scheduler.
//!
//! The store owns queue ordering, assignment attempts, leases, and fencing tokens. Every lifecycle
//! mutation is transactional so the Scheduler can keep only process-local Worker handles and
//! cancellation tokens in memory.

use crate::domain::task_session::{
    TaskCapabilityGrant, TaskProgress, TaskRequest, TaskSessionEvent, TaskSessionEventInput,
    TaskSessionEventKind, TaskSessionEventPage, TaskSessionId, TaskSessionSnapshot,
    TaskSessionState,
};
use rusqlite::{
    params, Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite-backed authority for Scheduler queue and Task Session lifecycle state.
#[derive(Clone)]
pub struct SchedulerStore {
    connection: Arc<Mutex<Connection>>,
    instance_id: Arc<str>,
}

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

/// Terminal outcome accepted by a fenced assignment completion.
pub(crate) enum DurableOutcome {
    Succeeded,
    Failed(String),
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
        let connection = Connection::open(path)
            .map_err(|error| format!("Failed to open scheduler database: {error}"))?;
        Self::initialize(connection)
    }

    fn open_read_only_at(path: PathBuf) -> Result<Self, String> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
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
        })
    }

    /// Opens an isolated in-memory Scheduler database.
    pub fn open_in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("Failed to open in-memory scheduler database: {error}"))?;
        Self::initialize(connection)
    }

    fn initialize(mut connection: Connection) -> Result<Self, String> {
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
                   state TEXT NOT NULL,
                   worker_id INTEGER,
                   dispatch_sequence INTEGER,
                   attempt_count INTEGER NOT NULL DEFAULT 0,
                   active_attempt_id INTEGER,
                   fencing_token INTEGER NOT NULL DEFAULT 0,
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
                 CREATE TABLE IF NOT EXISTS scheduler_task_events (
                   event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                   session_id INTEGER NOT NULL,
                   attempt_id INTEGER,
                   fencing_token INTEGER NOT NULL DEFAULT 0,
                    sequence INTEGER NOT NULL,
                    event_kind TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    progress_json TEXT,
                    created_at INTEGER NOT NULL,
                   UNIQUE(session_id, sequence),
                   FOREIGN KEY(session_id) REFERENCES scheduler_task_sessions(session_id)
                     ON DELETE CASCADE
                 );
                  CREATE INDEX IF NOT EXISTS idx_scheduler_events_cursor
                    ON scheduler_task_events(session_id, sequence);",
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
        })
    }

    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn register_owner(&self) -> Result<u64, String> {
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
        let capabilities = validate_capability_grants(capabilities, grant_source)?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler enqueue transaction: {error}"))?;
        let enqueue_sequence = next_sequence(&transaction, "next_enqueue_sequence")?;
        transaction
            .execute(
                "INSERT INTO scheduler_task_sessions
                   (enqueue_sequence, label, payload, state, attempt_count, fencing_token, created_at)
                 VALUES (?1, ?2, ?3, 'queued', 0, 0, ?4)",
                params![
                    to_i64(enqueue_sequence)?,
                    request.label,
                    request.payload,
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

    pub(crate) fn get_session(
        &self,
        id: TaskSessionId,
    ) -> Result<Option<TaskSessionSnapshot>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        load_session(&connection, id)
    }

    pub(crate) fn list_sessions(&self) -> Result<Vec<TaskSessionSnapshot>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(SESSION_SELECT_ALL)
            .map_err(|error| format!("Failed to prepare scheduler session query: {error}"))?;
        let rows = statement
            .query_map([], stored_session_from_row)
            .map_err(|error| format!("Failed to query scheduler sessions: {error}"))?;
        rows.map(|row| {
            row.map_err(|error| format!("Failed to decode scheduler session: {error}"))?
                .into_snapshot()
        })
        .collect()
    }

    pub(crate) fn append_assignment_event(
        &self,
        fence: AssignmentFence,
        input: TaskSessionEventInput,
    ) -> Result<TaskSessionEvent, String> {
        self.append_assignment_event_at(fence, input, now_millis())
    }

    fn append_assignment_event_at(
        &self,
        fence: AssignmentFence,
        input: TaskSessionEventInput,
        now: u64,
    ) -> Result<TaskSessionEvent, String> {
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
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
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
        if !(1..=500).contains(&limit) {
            return Err("Task Session event page limit must be between 1 and 500.".to_string());
        }
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
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

    #[cfg(test)]
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

    fn assignment_is_current_at(&self, fence: AssignmentFence, now: u64) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        assignment_is_current_on(&connection, fence, now)
    }

    pub(crate) fn claim_next(
        &self,
        owner_id: u64,
        worker_id: usize,
        lease_duration: Duration,
        global_limit: usize,
    ) -> Result<Option<DurableAssignment>, String> {
        self.claim_next_at(
            owner_id,
            worker_id,
            now_millis(),
            duration_millis(lease_duration)?,
            global_limit,
        )
    }

    fn claim_next_at(
        &self,
        owner_id: u64,
        worker_id: usize,
        now: u64,
        lease_millis: u64,
        global_limit: usize,
    ) -> Result<Option<DurableAssignment>, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler claim transaction: {error}"))?;
        recover_expired_in_transaction(&transaction, now)?;
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
            return Ok(None);
        }

        let candidate = transaction
            .query_row(
                "SELECT session_id, label, payload, attempt_count, fencing_token
                   FROM scheduler_task_sessions
                  WHERE state = 'queued'
                  ORDER BY enqueue_sequence
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
            return Ok(None);
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
        Ok(Some(DurableAssignment {
            fence: AssignmentFence {
                session_id,
                attempt_id,
                attempt,
                owner_id,
                fencing_token,
            },
            request: TaskRequest { label, payload },
            grants,
        }))
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
        let state = transaction
            .query_row(
                "SELECT state FROM scheduler_task_sessions WHERE session_id = ?1",
                params![to_i64(id.0)?],
                |row| row.get::<_, String>(0),
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

    pub(crate) fn finish(
        &self,
        fence: AssignmentFence,
        outcome: DurableOutcome,
    ) -> Result<FinishResult, String> {
        self.finish_at(fence, outcome, now_millis())
    }

    fn finish_at(
        &self,
        fence: AssignmentFence,
        outcome: DurableOutcome,
        now: u64,
    ) -> Result<FinishResult, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start scheduler completion: {error}"))?;
        let session_state = transaction
            .query_row(
                "SELECT state FROM scheduler_task_sessions
                  WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3
                    AND state IN ('running', 'cancelling') AND lease_expires_at > ?4",
                params![
                    to_i64(fence.session_id.0)?,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(now)?
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to validate scheduler fence: {error}"))?;
        let Some(session_state) = session_state else {
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

        let (state, attempt_state, error) = if session_state == "cancelling" {
            ("cancelled", "cancelled", None)
        } else {
            match outcome {
                DurableOutcome::Succeeded => ("succeeded", "succeeded", None),
                DurableOutcome::Failed(error) => ("failed", "failed", Some(error)),
                DurableOutcome::Cancelled => ("cancelled", "cancelled", None),
            }
        };
        transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = ?2, lease_expires_at = NULL, completed_at = ?3, error = ?4
                  WHERE attempt_id = ?1",
                params![
                    to_i64(fence.attempt_id)?,
                    attempt_state,
                    to_i64(now)?,
                    error
                ],
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
            &transaction,
            fence.session_id,
            Some(fence.attempt_id),
            fence.fencing_token,
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
            .map_err(|error| format!("Failed to commit scheduler completion: {error}"))?;
        drop(connection);
        Ok(FinishResult::Applied)
    }

    pub(crate) fn recover_expired(&self) -> Result<usize, String> {
        self.recover_expired_at(now_millis())
    }

    fn recover_expired_at(&self, now: u64) -> Result<usize, String> {
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
            "owner_id = ?1",
            params![to_i64(owner_id)?],
            now,
            "Scheduler owner shut down.",
            "scheduler_owner_shutdown",
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit scheduler owner cleanup: {error}"))?;
        Ok(recovered)
    }

    pub(crate) fn remove_terminal(&self, id: TaskSessionId) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM scheduler_task_sessions
                  WHERE session_id = ?1 AND state IN ('succeeded', 'failed', 'cancelled')",
                params![to_i64(id.0)?],
            )
            .map(|updated| updated == 1)
            .map_err(|error| format!("Failed to remove terminal task session: {error}"))
    }
}

const SESSION_COLUMNS: &str =
    "session_id, label, payload, state, worker_id, dispatch_sequence, attempt_count,
     active_attempt_id, fencing_token, lease_expires_at, error, created_at, started_at,
     completed_at, progress_phase, progress_completed, progress_total, next_event_sequence";
const SESSION_SELECT_ALL: &str =
    "SELECT session_id, label, payload, state, worker_id, dispatch_sequence, attempt_count,
            active_attempt_id, fencing_token, lease_expires_at, error, created_at, started_at,
            completed_at, progress_phase, progress_completed, progress_total, next_event_sequence
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
) -> Result<usize, String> {
    recover_matching_attempts(
        transaction,
        "lease_expires_at <= ?1",
        params![to_i64(now)?],
        now,
        "Assignment lease expired.",
        "assignment_lease_expired",
    )
}

fn recover_matching_attempts<P: rusqlite::Params>(
    transaction: &Transaction<'_>,
    predicate: &str,
    parameters: P,
    now: u64,
    attempt_error: &str,
    event_reason: &str,
) -> Result<usize, String> {
    let query = format!(
        "SELECT attempt_id, session_id, fencing_token FROM scheduler_task_attempts
          WHERE state = 'running' AND {predicate}"
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
                ))
            })
            .map_err(|error| format!("Failed to query scheduler recovery: {error}"))?;
        let decoded = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode scheduler recovery: {error}"))?;
        decoded
    };
    for (attempt_id, session_id, fencing_token) in &attempts {
        let previous_state = transaction
            .query_row(
                "SELECT state FROM scheduler_task_sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to read recovered session state: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = 'interrupted', lease_expires_at = NULL,
                        completed_at = ?2, error = ?3
                  WHERE attempt_id = ?1 AND state = 'running'",
                params![attempt_id, to_i64(now)?, attempt_error],
            )
            .map_err(|error| format!("Failed to interrupt scheduler attempt: {error}"))?;
        transaction
            .execute(
                "UPDATE scheduler_task_sessions
                    SET state = CASE WHEN state = 'cancelling' THEN 'cancelled' ELSE 'queued' END,
                        active_attempt_id = NULL, lease_expires_at = NULL,
                        completed_at = CASE WHEN state = 'cancelling' THEN ?2 ELSE NULL END
                  WHERE session_id = ?1 AND active_attempt_id = ?3
                    AND state IN ('running', 'cancelling')",
                params![session_id, to_i64(now)?, attempt_id],
            )
            .map_err(|error| format!("Failed to recover scheduler session: {error}"))?;
        let next_state = if previous_state == "cancelling" {
            "cancelled"
        } else {
            "queued"
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
                    "reason": event_reason
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
    Ok(attempts.len())
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
    let progress_json = input
        .progress
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("Failed to serialize task event progress: {error}"))?;
    transaction
        .execute(
            "INSERT INTO scheduler_task_events
               (session_id, attempt_id, fencing_token, sequence, event_kind, payload_json,
                 progress_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                to_i64(session_id.0)?,
                attempt_id.map(to_i64).transpose()?,
                to_i64(fencing_token)?,
                sequence,
                event_kind_name(input.kind),
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
        "succeeded" => Ok(TaskSessionState::Succeeded),
        "failed" => Ok(TaskSessionState::Failed),
        "cancelled" => Ok(TaskSessionState::Cancelled),
        _ => Err(format!("Unknown task session state '{value}'.")),
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

#[cfg(test)]
mod tests {
    use super::*;
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
                .finish_at(first.fence, DurableOutcome::Succeeded, 1_012)
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
                    .finish_at(assignment.fence, DurableOutcome::Succeeded, 20)
                    .expect("first finish")
            })
        };
        let second_finish = {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                second
                    .finish_at(assignment.fence, DurableOutcome::Succeeded, 20)
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
                .finish_at(assignment.fence, DurableOutcome::Succeeded, 30)
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
            .finish_at(first.fence, DurableOutcome::Cancelled, 41)
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
                .finish_at(stale.fence, DurableOutcome::Succeeded, 1_051)
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
}
