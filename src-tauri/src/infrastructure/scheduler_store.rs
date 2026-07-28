//! SQLite persistence for the concurrent execution Scheduler.
//!
//! The store owns queue ordering, assignment attempts, leases, and fencing tokens. Every lifecycle
//! mutation is transactional so the Scheduler can keep only process-local Worker handles and
//! cancellation tokens in memory.

use crate::domain::task_session::{
    TaskRequest, TaskSessionId, TaskSessionSnapshot, TaskSessionState,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite-backed authority for Scheduler queue and Task Session lifecycle state.
#[derive(Clone)]
pub struct SchedulerStore {
    connection: Arc<Mutex<Connection>>,
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

    /// Opens an isolated in-memory Scheduler database.
    pub fn open_in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("Failed to open in-memory scheduler database: {error}"))?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> Result<Self, String> {
        connection
            .busy_timeout(STORE_BUSY_TIMEOUT)
            .map_err(|error| format!("Failed to configure scheduler database timeout: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA wal_autocheckpoint = 100;
                 CREATE TABLE IF NOT EXISTS scheduler_metadata (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   next_enqueue_sequence INTEGER NOT NULL,
                   next_dispatch_sequence INTEGER NOT NULL
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
                   error TEXT,
                   created_at INTEGER NOT NULL,
                   started_at INTEGER,
                   completed_at INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS idx_scheduler_sessions_fifo
                   ON scheduler_task_sessions(state, enqueue_sequence);
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
                   ON scheduler_task_attempts(owner_id, state);",
            )
            .map_err(|error| format!("Failed to initialize scheduler database: {error}"))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
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
        self.enqueue_at(request, now_millis())
    }

    fn enqueue_at(&self, request: &TaskRequest, now: u64) -> Result<TaskSessionSnapshot, String> {
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
                    AND owner_id = ?4 AND fencing_token = ?5 AND state = 'running'",
                params![
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.session_id.0)?,
                    i64::from(fence.attempt),
                    to_i64(fence.owner_id)?,
                    to_i64(fence.fencing_token)?,
                    to_i64(lease_expires_at)?
                ],
            )
            .map_err(|error| format!("Failed to renew scheduler attempt: {error}"))?;
        if updated == 1 {
            transaction
                .execute(
                    "UPDATE scheduler_task_sessions SET lease_expires_at = ?2
                      WHERE session_id = ?1 AND active_attempt_id = ?3
                        AND fencing_token = ?4 AND state IN ('running', 'cancelling')",
                    params![
                        to_i64(fence.session_id.0)?,
                        to_i64(lease_expires_at)?,
                        to_i64(fence.attempt_id)?,
                        to_i64(fence.fencing_token)?
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
                  WHERE session_id = ?1 AND active_attempt_id = ?2 AND fencing_token = ?3",
                params![
                    to_i64(fence.session_id.0)?,
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.fencing_token)?
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
                    AND owner_id = ?4 AND fencing_token = ?5 AND state = 'running'",
                params![
                    to_i64(fence.attempt_id)?,
                    to_i64(fence.session_id.0)?,
                    i64::from(fence.attempt),
                    to_i64(fence.owner_id)?,
                    to_i64(fence.fencing_token)?
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
     completed_at";
const SESSION_SELECT_ALL: &str =
    "SELECT session_id, label, payload, state, worker_id, dispatch_sequence, attempt_count,
            active_attempt_id, fencing_token, lease_expires_at, error, created_at, started_at,
            completed_at
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
    )
}

fn recover_matching_attempts<P: rusqlite::Params>(
    transaction: &Transaction<'_>,
    predicate: &str,
    parameters: P,
    now: u64,
) -> Result<usize, String> {
    let query = format!(
        "SELECT attempt_id, session_id FROM scheduler_task_attempts
          WHERE state = 'running' AND {predicate}"
    );
    let attempts = {
        let mut statement = transaction
            .prepare(&query)
            .map_err(|error| format!("Failed to prepare scheduler recovery: {error}"))?;
        let rows = statement
            .query_map(parameters, |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| format!("Failed to query scheduler recovery: {error}"))?;
        let decoded = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode scheduler recovery: {error}"))?;
        decoded
    };
    for (attempt_id, session_id) in &attempts {
        transaction
            .execute(
                "UPDATE scheduler_task_attempts
                    SET state = 'interrupted', lease_expires_at = NULL,
                        completed_at = ?2, error = 'Assignment lease expired.'
                  WHERE attempt_id = ?1 AND state = 'running'",
                params![attempt_id, to_i64(now)?],
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
    }
    Ok(attempts.len())
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
}
