use crate::application::execution_engine::CompletionProjector;
use crate::domain::execution::{ExecutionRun, StepRun};
use crate::domain::task_session::{AgentTaskCompletionStatus, TaskExecutionOutput};
use crate::infrastructure::scheduler_store::StagedCompletion;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const CONVERSATION_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS conversations (
       conversation_id TEXT PRIMARY KEY,
       workspace_id TEXT NOT NULL,
       title TEXT NOT NULL,
       created_at INTEGER NOT NULL,
       updated_at INTEGER NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_conversations_workspace
       ON conversations(workspace_id, updated_at DESC);
     CREATE TABLE IF NOT EXISTS conversation_messages (
       message_id TEXT PRIMARY KEY,
       conversation_id TEXT NOT NULL,
       sequence INTEGER NOT NULL,
       role TEXT NOT NULL,
       text TEXT NOT NULL,
       created_at INTEGER NOT NULL,
       UNIQUE(conversation_id, sequence),
       FOREIGN KEY (conversation_id) REFERENCES conversations(conversation_id)
         ON DELETE CASCADE
     );
     CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation
       ON conversation_messages(conversation_id, sequence);";

#[derive(Clone)]
pub struct ExecutionStore {
    connection: Arc<Mutex<Connection>>,
}

impl CompletionProjector for ExecutionStore {
    fn project(&self, completion: &StagedCompletion) -> Result<(), String> {
        self.project_task_completion(completion)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationRecord {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConversationMessageInput {
    pub id: String,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConversationImportInput {
    pub id: String,
    pub title: String,
    pub messages: Vec<ConversationMessageInput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationMessageRecord {
    pub id: String,
    pub conversation_id: String,
    pub sequence: u64,
    pub role: String,
    pub text: String,
    pub created_at: u64,
}

impl ExecutionStore {
    pub fn open() -> Result<Self, String> {
        let path = database_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create execution data directory: {error}"))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("Failed to open execution database: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA cache_size = -8000;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA wal_autocheckpoint = 100;
                 CREATE TABLE IF NOT EXISTS execution_contracts (
                   contract_id TEXT PRIMARY KEY,
                   task_id TEXT NOT NULL,
                   workspace_id TEXT NOT NULL,
                   version INTEGER NOT NULL,
                   payload_json TEXT NOT NULL,
                   created_at TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS execution_runs (
                   run_id TEXT PRIMARY KEY,
                   contract_id TEXT NOT NULL,
                   status TEXT NOT NULL,
                   current_step_ids_json TEXT NOT NULL,
                   started_at TEXT NOT NULL,
                   completed_at TEXT,
                   updated_at TEXT NOT NULL,
                   revision INTEGER NOT NULL DEFAULT 0,
                   FOREIGN KEY (contract_id) REFERENCES execution_contracts(contract_id)
                 );
                 CREATE TABLE IF NOT EXISTS step_runs (
                   run_id TEXT NOT NULL,
                   step_id TEXT NOT NULL,
                   status TEXT NOT NULL,
                   attempt INTEGER NOT NULL,
                   started_at TEXT,
                    completed_at TEXT,
                    summary TEXT,
                    result_json TEXT,
                    task_session_projection_id TEXT,
                    lease_owner TEXT,
                   lease_expires_at INTEGER,
                   updated_at TEXT NOT NULL,
                   PRIMARY KEY (run_id, step_id),
                   FOREIGN KEY (run_id) REFERENCES execution_runs(run_id) ON DELETE CASCADE
                 );
                  CREATE INDEX IF NOT EXISTS idx_execution_runs_status
                    ON execution_runs(status);
                  CREATE TABLE IF NOT EXISTS ai_audit_events (
                    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id TEXT,
                    event_type TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                  );
                   CREATE INDEX IF NOT EXISTS idx_ai_audit_events_run
                     ON ai_audit_events(run_id, event_id);
                  CREATE TABLE IF NOT EXISTS task_completion_projection_receipts (
                    projection_id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    conversation_id TEXT NOT NULL,
                    message_id TEXT NOT NULL UNIQUE,
                    output_json TEXT NOT NULL,
                    projected_at INTEGER NOT NULL
                  );",
            )
            .map_err(|error| format!("Failed to initialize execution database: {error}"))?;
        connection
            .execute_batch(CONVERSATION_SCHEMA)
            .map_err(|error| format!("Failed to initialize conversation database: {error}"))?;
        // Keep databases created by earlier builds usable without destructive migrations.
        let _ = connection.execute(
            "ALTER TABLE execution_runs ADD COLUMN revision INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = connection.execute("ALTER TABLE step_runs ADD COLUMN lease_owner TEXT", []);
        let _ = connection.execute(
            "ALTER TABLE step_runs ADD COLUMN lease_expires_at INTEGER",
            [],
        );
        let _ = connection.execute("ALTER TABLE step_runs ADD COLUMN result_json TEXT", []);
        let _ = connection.execute(
            "ALTER TABLE step_runs ADD COLUMN task_session_projection_id TEXT",
            [],
        );
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.recover_interrupted_runs()?;
        Ok(store)
    }

    pub fn save(&self, run: &ExecutionRun) -> Result<ExecutionRun, String> {
        validate_run(run)?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start execution transaction: {error}"))?;
        let current_revision = transaction
            .query_row(
                "SELECT revision FROM execution_runs WHERE run_id = ?1",
                params![run.run_id],
                |row| row.get::<_, u64>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to read execution revision: {error}"))?;
        if let Some(current_revision) = current_revision {
            if current_revision != run.revision {
                return Err(format!(
                    "Execution run '{}' is stale (expected revision {}, current revision {}). Reload before saving.",
                    run.run_id, run.revision, current_revision
                ));
            }
        } else if run.revision != 0 {
            return Err(format!(
                "New execution run '{}' must start at revision 0.",
                run.run_id
            ));
        }
        save_contract(&transaction, &run.contract)?;
        let contract_id = contract_string(&run.contract, "contract_id")?;
        let now = now_millis()?.to_string();
        let existing_steps = load_step_leases(&transaction, &run.run_id)?;
        transaction
            .execute(
                "INSERT INTO execution_runs
                   (run_id, contract_id, status, current_step_ids_json, started_at, completed_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(run_id) DO UPDATE SET
                   status = excluded.status,
                   current_step_ids_json = excluded.current_step_ids_json,
                   completed_at = excluded.completed_at,
                   updated_at = excluded.updated_at,
                   revision = execution_runs.revision + 1
                 WHERE execution_runs.revision = ?8",
                params![
                    run.run_id,
                    contract_id,
                    run.status,
                    serde_json::to_string(&run.current_step_ids).map_err(|error| error.to_string())?,
                    run.started_at,
                     run.completed_at,
                     now,
                     run.revision,
                 ],
            )
            .map_err(|error| format!("Failed to save execution run: {error}"))?;
        transaction
            .execute(
                "DELETE FROM step_runs WHERE run_id = ?1",
                params![run.run_id],
            )
            .map_err(|error| format!("Failed to replace execution steps: {error}"))?;
        for step in run.step_runs.values() {
            let existing_step = existing_steps.get(&step.step_id);
            transaction
                .execute(
                    "INSERT INTO step_runs
                       (run_id, step_id, status, attempt, started_at, completed_at, summary,
                         lease_owner, lease_expires_at, result_json,
                         task_session_projection_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        run.run_id,
                        step.step_id,
                        step.status,
                        step.attempt,
                        step.started_at,
                        step.completed_at,
                        step.summary,
                        step.lease_owner
                            .clone()
                            .or_else(|| existing_step.and_then(|value| value.0.clone())),
                        step.lease_expires_at
                            .or_else(|| existing_step.and_then(|value| value.1)),
                        existing_step.and_then(|value| value.2.clone()),
                        existing_step.and_then(|value| value.3.clone()),
                        now,
                    ],
                )
                .map_err(|error| format!("Failed to save execution step: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit execution run: {error}"))?;
        drop(connection);
        self.get(&run.run_id)?
            .ok_or_else(|| "Execution run disappeared after save.".to_string())
    }

    /// Idempotently projects one scheduler-staged Agent completion in a single executions.db transaction.
    pub(crate) fn project_task_completion(
        &self,
        completion: &StagedCompletion,
    ) -> Result<(), String> {
        match &completion.output {
            TaskExecutionOutput::Agent(_) => self.project_agent_task_completion(completion),
            TaskExecutionOutput::Chat(result) => {
                if result.conversation_id != completion.conversation_id {
                    return Err("Chat completion conversation ownership changed.".to_string());
                }
                let title = {
                    let connection = self.connection.lock().map_err(|error| error.to_string())?;
                    connection
                        .query_row(
                            "SELECT title FROM conversations
                              WHERE conversation_id = ?1 AND workspace_id = ?2",
                            params![completion.conversation_id, completion.workspace_id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|error| {
                            format!("Failed to validate Chat completion conversation: {error}")
                        })?
                        .ok_or_else(|| {
                            "Chat completion conversation does not belong to this workspace."
                                .to_string()
                        })?
                };
                self.append_conversation_message(
                    &completion.workspace_id,
                    &completion.conversation_id,
                    &title,
                    &ConversationMessageInput {
                        id: format!("{}:assistant", completion.projection_id),
                        role: "agent".to_string(),
                        text: result.message.clone(),
                    },
                )?;
                Ok(())
            }
            // Edit proposals have no conversation projection. The scheduler outbox itself is the
            // durable authoritative query surface; singular editor review state is not recreated.
            TaskExecutionOutput::Edit(_) => Ok(()),
            TaskExecutionOutput::None => {
                Err("Empty task completion cannot be projected.".to_string())
            }
        }
    }

    fn project_agent_task_completion(&self, completion: &StagedCompletion) -> Result<(), String> {
        let TaskExecutionOutput::Agent(result) = &completion.output else {
            unreachable!();
        };
        let output_json = serde_json::to_string(&completion.output)
            .map_err(|error| format!("Failed to encode completion projection: {error}"))?;
        let message_id = format!("{}:agent", completion.projection_id);
        let message_text = serde_json::to_string(result)
            .map_err(|error| format!("Failed to encode Agent conversation result: {error}"))?;
        let step_status = match result.completion_status {
            AgentTaskCompletionStatus::Completed => "completed",
            AgentTaskCompletionStatus::Blocked => "blocked",
        };
        let summary = if result.completion_status == AgentTaskCompletionStatus::Blocked {
            result.blocked_reason.as_deref().unwrap_or(&result.summary)
        } else {
            &result.summary
        };
        let now = now_millis()?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start completion projection: {error}"))?;
        let existing = transaction
            .query_row(
                "SELECT run_id, conversation_id, message_id, output_json
                   FROM task_completion_projection_receipts WHERE projection_id = ?1",
                params![completion.projection_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to check completion projection receipt: {error}"))?;
        if let Some((run_id, conversation_id, stored_message_id, stored_output)) = existing {
            if run_id != completion.execution_run_id
                || conversation_id != completion.conversation_id
                || stored_message_id != message_id
                || stored_output != output_json
            {
                return Err("Completion projection ID is bound to different content.".to_string());
            }
            transaction.commit().map_err(|error| {
                format!("Failed to commit idempotent completion projection: {error}")
            })?;
            return Ok(());
        }
        let run_workspace = transaction
            .query_row(
                "SELECT contracts.workspace_id
                   FROM execution_runs runs
                   JOIN execution_contracts contracts ON contracts.contract_id = runs.contract_id
                  WHERE runs.run_id = ?1",
                params![completion.execution_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to verify completion execution run: {error}"))?
            .ok_or_else(|| "Completion execution run was not found.".to_string())?;
        if run_workspace != completion.workspace_id {
            return Err("Completion execution run does not belong to this workspace.".to_string());
        }
        if !conversation_exists_in(
            &transaction,
            &completion.workspace_id,
            &completion.conversation_id,
        )? {
            return Err("Completion conversation does not belong to this workspace.".to_string());
        }
        let step_projection = transaction
            .query_row(
                "SELECT task_session_projection_id FROM step_runs
                  WHERE run_id = ?1 AND step_id = 'worker.execute'",
                params![completion.execution_run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| format!("Failed to verify worker.execute step: {error}"))?
            .ok_or_else(|| "Completion execution run has no worker.execute step.".to_string())?;
        if step_projection
            .as_deref()
            .is_some_and(|projection| projection != completion.projection_id)
        {
            return Err(
                "worker.execute already has different Task Session provenance.".to_string(),
            );
        }
        let sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM conversation_messages
                  WHERE conversation_id = ?1",
                params![completion.conversation_id],
                |row| row.get::<_, u64>(0),
            )
            .map_err(|error| {
                format!("Failed to allocate Agent result message sequence: {error}")
            })?;
        transaction
            .execute(
                "INSERT INTO conversation_messages
                   (message_id, conversation_id, sequence, role, text, created_at)
                 VALUES (?1, ?2, ?3, 'agent', ?4, ?5)",
                params![
                    message_id,
                    completion.conversation_id,
                    sequence,
                    message_text,
                    now
                ],
            )
            .map_err(|error| format!("Failed to record Agent result message: {error}"))?;
        let updated = transaction
            .execute(
                "UPDATE step_runs
                    SET status = ?3, completed_at = ?4, summary = ?5, result_json = ?6,
                        task_session_projection_id = ?7, lease_owner = NULL,
                        lease_expires_at = NULL, updated_at = ?4
                  WHERE run_id = ?1 AND step_id = ?2
                    AND (task_session_projection_id IS NULL OR task_session_projection_id = ?7)",
                params![
                    completion.execution_run_id,
                    "worker.execute",
                    step_status,
                    now.to_string(),
                    summary,
                    output_json,
                    completion.projection_id
                ],
            )
            .map_err(|error| format!("Failed to project worker.execute result: {error}"))?;
        if updated != 1 {
            return Err("worker.execute result provenance changed during projection.".to_string());
        }
        if result.completion_status == AgentTaskCompletionStatus::Blocked {
            transaction
                .execute(
                    "UPDATE execution_runs SET status = 'blocked', completed_at = ?2,
                        updated_at = ?2, revision = revision + 1 WHERE run_id = ?1",
                    params![completion.execution_run_id, now.to_string()],
                )
                .map_err(|error| format!("Failed to project blocked execution run: {error}"))?;
        }
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE conversation_id = ?1",
                params![completion.conversation_id, now],
            )
            .map_err(|error| format!("Failed to update projected conversation: {error}"))?;
        transaction
            .execute(
                "INSERT INTO task_completion_projection_receipts
                   (projection_id, run_id, conversation_id, message_id, output_json, projected_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    completion.projection_id,
                    completion.execution_run_id,
                    completion.conversation_id,
                    message_id,
                    output_json,
                    now
                ],
            )
            .map_err(|error| format!("Failed to record completion projection receipt: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit completion projection: {error}"))
    }

    pub fn claim_step(
        &self,
        run_id: &str,
        step_id: &str,
        owner: &str,
        lease_ms: u64,
    ) -> Result<(), String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start step claim: {error}"))?;
        let now = now_millis()?;
        let expires = now.saturating_add(lease_ms);
        let updated = transaction
            .execute(
                "UPDATE step_runs
                 SET status = 'running', attempt = attempt + 1, started_at = COALESCE(started_at, ?1),
                     lease_owner = ?2, lease_expires_at = ?3, updated_at = ?1
                 WHERE run_id = ?4 AND step_id = ?5
                   AND (status IN ('pending', 'ready', 'interrupted')
                        OR (status = 'running' AND (lease_expires_at IS NULL OR lease_expires_at < ?1)))",
                params![now.to_string(), owner, expires, run_id, step_id],
            )
            .map_err(|error| format!("Failed to claim execution step: {error}"))?;
        if updated == 0 {
            return Err(format!(
                "Execution step {step_id} is already claimed or unavailable."
            ));
        }
        transaction
            .execute(
                "UPDATE execution_runs SET status = 'running', updated_at = ?1,
                 revision = revision + 1 WHERE run_id = ?2",
                params![now.to_string(), run_id],
            )
            .map_err(|error| format!("Failed to update claimed execution run: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit step claim: {error}"))
    }

    pub fn finish_step(
        &self,
        run_id: &str,
        step_id: &str,
        owner: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<(), String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let updated = connection
            .execute(
                "UPDATE step_runs SET status = ?1, completed_at = ?2, summary = ?3,
                 lease_owner = NULL, lease_expires_at = NULL, updated_at = ?2
                 WHERE run_id = ?4 AND step_id = ?5 AND lease_owner = ?6",
                params![
                    status,
                    now_millis()?.to_string(),
                    summary,
                    run_id,
                    step_id,
                    owner
                ],
            )
            .map_err(|error| format!("Failed to finish execution step: {error}"))?;
        if updated == 0 {
            return Err(format!(
                "Execution step {step_id} lease is not owned by this worker."
            ));
        }
        Ok(())
    }

    pub fn get(&self, run_id: &str) -> Result<Option<ExecutionRun>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        load_run(&connection, run_id)
    }

    pub fn record_ai_audit(
        &self,
        run_id: Option<&str>,
        event_type: &str,
        payload: &Value,
    ) -> Result<(), String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let payload_json = serde_json::to_string(payload)
            .map_err(|error| format!("Failed to encode AI audit payload: {error}"))?;
        connection
            .execute(
                "INSERT INTO ai_audit_events (run_id, event_type, payload_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![run_id, event_type, payload_json, now_millis()?.to_string()],
            )
            .map_err(|error| format!("Failed to record AI audit event: {error}"))?;
        Ok(())
    }

    pub fn list_conversations(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<ConversationRecord>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT conversation_id, workspace_id, title, created_at, updated_at
                 FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at DESC",
            )
            .map_err(|error| format!("Failed to prepare conversation query: {error}"))?;
        let conversations = statement
            .query_map(params![workspace_id], |row| {
                Ok(ConversationRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|error| format!("Failed to query conversations: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode conversation: {error}"))?;
        Ok(conversations)
    }

    pub fn load_conversation_messages(
        &self,
        workspace_id: &str,
        conversation_id: &str,
    ) -> Result<Vec<ConversationMessageRecord>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        if !conversation_exists_in(&connection, workspace_id, conversation_id)? {
            return Err("Conversation does not belong to this workspace.".to_string());
        }
        let mut statement = connection
            .prepare(
                "SELECT message_id, conversation_id, sequence, role, text, created_at
                 FROM conversation_messages WHERE conversation_id = ?1 ORDER BY sequence",
            )
            .map_err(|error| format!("Failed to prepare message query: {error}"))?;
        let messages = statement
            .query_map(params![conversation_id], |row| {
                Ok(ConversationMessageRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    sequence: row.get(2)?,
                    role: row.get(3)?,
                    text: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|error| format!("Failed to query conversation messages: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode conversation message: {error}"))?;
        Ok(messages)
    }

    /// Returns whether a durable conversation belongs to the requested workspace.
    pub fn conversation_exists(
        &self,
        workspace_id: &str,
        conversation_id: &str,
    ) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        conversation_exists_in(&connection, workspace_id, conversation_id)
    }

    /// Returns whether one durable message exactly matches its conversation, role, and text.
    pub fn conversation_message_matches(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        message_id: &str,
        sequence: u64,
        role: &str,
        text: &str,
    ) -> Result<bool, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT 1
                   FROM conversation_messages messages
                   JOIN conversations conversations
                     ON conversations.conversation_id = messages.conversation_id
                  WHERE conversations.workspace_id = ?1
                    AND messages.conversation_id = ?2
                    AND messages.message_id = ?3
                    AND messages.sequence = ?4
                    AND messages.role = ?5
                    AND messages.text = ?6",
                params![
                    workspace_id,
                    conversation_id,
                    message_id,
                    sequence,
                    role,
                    text
                ],
                |_| Ok(()),
            )
            .optional()
            .map(|match_| match_.is_some())
            .map_err(|error| format!("Failed to verify conversation message ownership: {error}"))
    }

    pub fn append_conversation_message(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        title: &str,
        input: &ConversationMessageInput,
    ) -> Result<ConversationMessageRecord, String> {
        validate_conversation_message(input)?;
        let now = now_millis()?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start conversation transaction: {error}"))?;
        transaction
            .execute(
                "INSERT INTO conversations (conversation_id, workspace_id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                   title = excluded.title, updated_at = excluded.updated_at
                 WHERE conversations.workspace_id = excluded.workspace_id",
                params![conversation_id, workspace_id, title.trim(), now],
            )
            .map_err(|error| format!("Failed to save conversation: {error}"))?;
        let conversation_scope = transaction
            .query_row(
                "SELECT workspace_id FROM conversations WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("Failed to verify conversation ownership: {error}"))?;
        if conversation_scope != workspace_id {
            return Err("Conversation does not belong to this workspace.".to_string());
        }
        let existing = transaction
            .query_row(
                "SELECT conversation_id, sequence, role, text, created_at
                 FROM conversation_messages WHERE message_id = ?1",
                params![input.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to check message idempotency: {error}"))?;
        if let Some((existing_conversation, sequence, role, text, created_at)) = existing {
            if existing_conversation != conversation_id || role != input.role || text != input.text
            {
                return Err("Message ID is already bound to different content.".to_string());
            }
            transaction
                .commit()
                .map_err(|error| format!("Failed to commit idempotent message: {error}"))?;
            return Ok(ConversationMessageRecord {
                id: input.id.clone(),
                conversation_id: existing_conversation,
                sequence,
                role,
                text,
                created_at,
            });
        }
        let sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM conversation_messages
                 WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get::<_, u64>(0),
            )
            .map_err(|error| format!("Failed to allocate message sequence: {error}"))?;
        transaction
            .execute(
                "INSERT INTO conversation_messages
                 (message_id, conversation_id, sequence, role, text, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    input.id,
                    conversation_id,
                    sequence,
                    input.role,
                    input.text,
                    now
                ],
            )
            .map_err(|error| format!("Failed to append conversation message: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit conversation message: {error}"))?;
        Ok(ConversationMessageRecord {
            id: input.id.clone(),
            conversation_id: conversation_id.to_string(),
            sequence,
            role: input.role.clone(),
            text: input.text.clone(),
            created_at: now,
        })
    }

    pub fn import_conversations(
        &self,
        workspace_id: &str,
        conversations: &[ConversationImportInput],
    ) -> Result<usize, String> {
        let mut imported = 0;
        for conversation in conversations {
            for message in &conversation.messages {
                self.append_conversation_message(
                    workspace_id,
                    &conversation.id,
                    &conversation.title,
                    message,
                )?;
                imported += 1;
            }
        }
        Ok(imported)
    }

    pub fn prune_conversations(
        &self,
        workspace_id: &str,
        retained_ids: &[String],
    ) -> Result<usize, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare("SELECT conversation_id FROM conversations WHERE workspace_id = ?1")
            .map_err(|error| format!("Failed to prepare conversation retention query: {error}"))?;
        let ids = statement
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Failed to query conversations for retention: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode conversation retention IDs: {error}"))?;
        let mut deleted = 0;
        for conversation_id in ids {
            if retained_ids
                .iter()
                .any(|retained| retained == &conversation_id)
            {
                continue;
            }
            deleted += connection
                .execute(
                    "DELETE FROM conversations WHERE conversation_id = ?1 AND workspace_id = ?2",
                    params![conversation_id, workspace_id],
                )
                .map_err(|error| format!("Failed to prune conversation: {error}"))?;
        }
        Ok(deleted)
    }

    pub fn list_active(&self) -> Result<Vec<ExecutionRun>, String> {
        let connection = self.connection.lock().map_err(|error| error.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT run_id FROM execution_runs
                 WHERE status IN ('pending', 'running', 'blocked', 'failed')
                 ORDER BY updated_at DESC",
            )
            .map_err(|error| format!("Failed to query active execution runs: {error}"))?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("Failed to read active execution runs: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to decode active execution run: {error}"))?;
        ids.into_iter()
            .map(|run_id| {
                load_run(&connection, &run_id)?
                    .ok_or_else(|| format!("Execution run {run_id} disappeared during query."))
            })
            .collect()
    }

    fn recover_interrupted_runs(&self) -> Result<(), String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start execution recovery: {error}"))?;
        let now = now_millis()?.to_string();
        transaction
            .execute(
                "UPDATE step_runs SET status = 'interrupted', completed_at = ?1,
                   summary = COALESCE(summary, 'Spacesly restarted while this step was running.'),
                   updated_at = ?1
                 WHERE status IN ('running', 'ready')",
                params![now],
            )
            .map_err(|error| format!("Failed to recover execution steps: {error}"))?;
        transaction
            .execute(
                "UPDATE execution_runs SET status = 'blocked', completed_at = ?1, updated_at = ?1
                 WHERE status = 'running'",
                params![now],
            )
            .map_err(|error| format!("Failed to recover execution runs: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit execution recovery: {error}"))
    }
}

fn save_contract(transaction: &Transaction<'_>, contract: &Value) -> Result<(), String> {
    let contract_id = contract_string(contract, "contract_id")?;
    let payload = serde_json::to_string(contract).map_err(|error| error.to_string())?;
    let existing = transaction
        .query_row(
            "SELECT payload_json FROM execution_contracts WHERE contract_id = ?1",
            params![contract_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("Failed to read execution contract: {error}"))?;
    if let Some(existing) = existing {
        if existing != payload {
            return Err("Execution Contract is immutable and cannot be replaced.".to_string());
        }
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO execution_contracts
               (contract_id, task_id, workspace_id, version, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                contract_id,
                contract_string(contract, "task_id")?,
                contract_string(contract, "workspace_id")?,
                contract.get("version").and_then(Value::as_u64).unwrap_or(1),
                payload,
                contract_string(contract, "created_at")?,
            ],
        )
        .map_err(|error| format!("Failed to save execution contract: {error}"))?;
    Ok(())
}

fn validate_conversation_message(input: &ConversationMessageInput) -> Result<(), String> {
    if input.id.trim().is_empty() {
        return Err("Conversation message ID is required.".to_string());
    }
    if !matches!(input.role.as_str(), "user" | "agent" | "system") {
        return Err("Conversation message role is invalid.".to_string());
    }
    if input.text.trim().is_empty() {
        return Err("Conversation message text is required.".to_string());
    }
    if input.text.len() > 256 * 1024 {
        return Err("Conversation message exceeds the 256 KiB limit.".to_string());
    }
    Ok(())
}

fn load_run(connection: &Connection, run_id: &str) -> Result<Option<ExecutionRun>, String> {
    let row = connection
        .query_row(
            "SELECT r.run_id, r.status, r.current_step_ids_json, r.started_at, r.completed_at, r.revision,
                    c.payload_json
             FROM execution_runs r
             JOIN execution_contracts c ON c.contract_id = r.contract_id
             WHERE r.run_id = ?1",
            params![run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to load execution run: {error}"))?;
    let Some((run_id, status, current_steps, started_at, completed_at, revision, contract)) = row
    else {
        return Ok(None);
    };
    let mut statement = connection
        .prepare(
            "SELECT step_id, status, attempt, started_at, completed_at, summary, lease_owner, lease_expires_at
             FROM step_runs WHERE run_id = ?1 ORDER BY rowid",
        )
        .map_err(|error| format!("Failed to load execution steps: {error}"))?;
    let steps = statement
        .query_map(params![run_id], |row| {
            Ok(StepRun {
                step_id: row.get(0)?,
                status: row.get(1)?,
                attempt: row.get(2)?,
                started_at: row.get(3)?,
                completed_at: row.get(4)?,
                summary: row.get(5)?,
                lease_owner: row.get(6)?,
                lease_expires_at: row.get(7)?,
            })
        })
        .map_err(|error| format!("Failed to query execution steps: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to decode execution step: {error}"))?;
    Ok(Some(ExecutionRun {
        run_id,
        contract: serde_json::from_str(&contract)
            .map_err(|error| format!("Failed to decode execution contract: {error}"))?,
        status,
        current_step_ids: serde_json::from_str(&current_steps)
            .map_err(|error| format!("Failed to decode current execution steps: {error}"))?,
        step_runs: steps
            .into_iter()
            .map(|step| (step.step_id.clone(), step))
            .collect::<BTreeMap<_, _>>(),
        started_at,
        completed_at,
        revision,
    }))
}

fn load_step_leases(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<BTreeMap<String, (Option<String>, Option<u64>, Option<String>, Option<String>)>, String>
{
    let mut statement = transaction
        .prepare(
            "SELECT step_id, lease_owner, lease_expires_at, result_json,
                    task_session_projection_id
             FROM step_runs WHERE run_id = ?1",
        )
        .map_err(|error| format!("Failed to load execution step leases: {error}"))?;
    let leases = statement
        .query_map(params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<u64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ),
            ))
        })
        .map_err(|error| format!("Failed to query execution step leases: {error}"))?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|error| format!("Failed to decode execution step leases: {error}"))?;
    Ok(leases)
}

fn validate_run(run: &ExecutionRun) -> Result<(), String> {
    if run.run_id.trim().is_empty() {
        return Err("Execution run ID is required.".to_string());
    }
    contract_string(&run.contract, "contract_id")?;
    contract_string(&run.contract, "task_id")?;
    contract_string(&run.contract, "workspace_id")?;
    Ok(())
}

fn contract_string<'a>(contract: &'a Value, field: &str) -> Result<&'a str, String> {
    contract
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Execution Contract field {field} is required."))
}

fn conversation_exists_in(
    connection: &Connection,
    workspace_id: &str,
    conversation_id: &str,
) -> Result<bool, String> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM conversations WHERE conversation_id = ?1 AND workspace_id = ?2",
            params![conversation_id, workspace_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| format!("Failed to verify conversation scope: {error}"))?;
    Ok(exists.is_some())
}

fn database_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
        .ok_or_else(|| "Cannot resolve application data directory.".to_string())?;
    Ok(base.join("spacesly").join("executions.db"))
}

fn now_millis() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before Unix epoch: {error}"))?
        .as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_session::{AgentTaskResult, TaskSessionId, TaskSessionState};

    fn test_store() -> ExecutionStore {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(CONVERSATION_SCHEMA).unwrap();
        ExecutionStore {
            connection: Arc::new(Mutex::new(connection)),
        }
    }

    fn projection_test_store() -> ExecutionStore {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "{CONVERSATION_SCHEMA}
                 CREATE TABLE execution_contracts (
                   contract_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL
                 );
                 CREATE TABLE execution_runs (
                   run_id TEXT PRIMARY KEY, contract_id TEXT NOT NULL, status TEXT NOT NULL,
                   completed_at TEXT, updated_at TEXT NOT NULL, revision INTEGER NOT NULL
                 );
                 CREATE TABLE step_runs (
                   run_id TEXT NOT NULL, step_id TEXT NOT NULL, status TEXT NOT NULL,
                   completed_at TEXT, summary TEXT, result_json TEXT,
                   task_session_projection_id TEXT, lease_owner TEXT,
                   lease_expires_at INTEGER, updated_at TEXT NOT NULL,
                   PRIMARY KEY(run_id, step_id)
                 );
                 CREATE TABLE task_completion_projection_receipts (
                   projection_id TEXT PRIMARY KEY, run_id TEXT NOT NULL,
                   conversation_id TEXT NOT NULL, message_id TEXT NOT NULL UNIQUE,
                   output_json TEXT NOT NULL, projected_at INTEGER NOT NULL
                 );"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO execution_contracts (contract_id, workspace_id) VALUES ('contract-1', 'workspace-a')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO execution_runs
                   (run_id, contract_id, status, updated_at, revision)
                 VALUES ('run-1', 'contract-1', 'running', '1', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO step_runs (run_id, step_id, status, updated_at)
                 VALUES ('run-1', 'worker.execute', 'running', '1')",
                [],
            )
            .unwrap();
        let store = ExecutionStore {
            connection: Arc::new(Mutex::new(connection)),
        };
        store
            .append_conversation_message(
                "workspace-a",
                "conversation-a",
                "Agent",
                &ConversationMessageInput {
                    id: "user-1".to_string(),
                    role: "user".to_string(),
                    text: "Do the work".to_string(),
                },
            )
            .unwrap();
        store
    }

    #[test]
    fn task_completion_projection_is_idempotent_and_records_provenance() {
        let store = projection_test_store();
        let completion = StagedCompletion {
            projection_id: "projection-1".to_string(),
            session_id: TaskSessionId(1),
            attempt_id: 2,
            fencing_token: 3,
            workspace_id: "workspace-a".to_string(),
            conversation_id: "conversation-a".to_string(),
            execution_run_id: "run-1".to_string(),
            output: TaskExecutionOutput::Agent(AgentTaskResult {
                summary: "Work completed".to_string(),
                evidence: vec!["test passed".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AgentTaskCompletionStatus::Completed,
                blocked_reason: None,
            }),
            terminal_state: TaskSessionState::Succeeded,
        };
        store
            .project_task_completion(&completion)
            .expect("first projection succeeds");
        store
            .project_task_completion(&completion)
            .expect("repeated projection succeeds");

        let messages = store
            .load_conversation_messages("workspace-a", "conversation-a")
            .expect("messages loaded");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].id, "projection-1:agent");
        let connection = store.connection.lock().unwrap();
        let (status, provenance, receipts): (String, String, u64) = connection
            .query_row(
                "SELECT steps.status, steps.task_session_projection_id,
                        (SELECT COUNT(*) FROM task_completion_projection_receipts)
                   FROM step_runs steps
                  WHERE steps.run_id = 'run-1' AND steps.step_id = 'worker.execute'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(provenance, "projection-1");
        assert_eq!(receipts, 1);
    }

    #[test]
    fn chat_completion_projection_is_deterministic_and_idempotent() {
        let store = projection_test_store();
        let completion = StagedCompletion {
            projection_id: "projection-chat".to_string(),
            session_id: TaskSessionId(2),
            attempt_id: 3,
            fencing_token: 4,
            workspace_id: "workspace-a".to_string(),
            conversation_id: "conversation-a".to_string(),
            execution_run_id: String::new(),
            output: TaskExecutionOutput::Chat(crate::domain::task_session::ChatTaskResult {
                conversation_id: "conversation-a".to_string(),
                message: "Recovered assistant response".to_string(),
            }),
            terminal_state: TaskSessionState::Succeeded,
        };

        store.project_task_completion(&completion).unwrap();
        store.project_task_completion(&completion).unwrap();
        let messages = store
            .load_conversation_messages("workspace-a", "conversation-a")
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].id, "projection-chat:assistant");
        assert_eq!(messages[1].role, "agent");
        assert_eq!(messages[1].text, "Recovered assistant response");
    }

    #[test]
    fn conversation_messages_are_ordered_and_idempotent() {
        let store = test_store();
        let first = ConversationMessageInput {
            id: "message-1".to_string(),
            role: "user".to_string(),
            text: "Hello".to_string(),
        };
        let second = ConversationMessageInput {
            id: "message-2".to_string(),
            role: "agent".to_string(),
            text: "Hi".to_string(),
        };

        let saved_first = store
            .append_conversation_message("workspace-a", "conversation-a", "Chat", &first)
            .unwrap();
        let repeated = store
            .append_conversation_message("workspace-a", "conversation-a", "Chat", &first)
            .unwrap();
        store
            .append_conversation_message("workspace-a", "conversation-a", "Chat", &second)
            .unwrap();

        assert_eq!(saved_first.sequence, 1);
        assert_eq!(repeated.sequence, 1);
        let messages = store
            .load_conversation_messages("workspace-a", "conversation-a")
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn conversation_scope_rejects_cross_workspace_reads_and_replays() {
        let store = test_store();
        let message = ConversationMessageInput {
            id: "message-1".to_string(),
            role: "user".to_string(),
            text: "Private".to_string(),
        };
        store
            .append_conversation_message("workspace-a", "conversation-a", "Chat", &message)
            .unwrap();

        assert!(store
            .load_conversation_messages("workspace-b", "conversation-a")
            .is_err());
        assert!(store
            .append_conversation_message(
                "workspace-b",
                "conversation-b",
                "Chat",
                &ConversationMessageInput {
                    id: "message-1".to_string(),
                    role: "agent".to_string(),
                    text: "Replay".to_string(),
                },
            )
            .is_err());
        assert!(store
            .conversation_exists("workspace-a", "conversation-a")
            .unwrap());
        assert!(!store
            .conversation_exists("workspace-b", "conversation-a")
            .unwrap());
        assert!(store
            .conversation_message_matches(
                "workspace-a",
                "conversation-a",
                "message-1",
                1,
                "user",
                "Private"
            )
            .unwrap());
        assert!(!store
            .conversation_message_matches(
                "workspace-b",
                "conversation-a",
                "message-1",
                1,
                "user",
                "Private"
            )
            .unwrap());
    }

    #[test]
    fn conversation_retention_is_scoped_to_the_workspace() {
        let store = test_store();
        for (conversation_id, message_id) in [("keep", "keep-message"), ("drop", "drop-message")] {
            store
                .append_conversation_message(
                    "workspace-a",
                    conversation_id,
                    "Chat",
                    &ConversationMessageInput {
                        id: message_id.to_string(),
                        role: "user".to_string(),
                        text: conversation_id.to_string(),
                    },
                )
                .unwrap();
        }
        store
            .append_conversation_message(
                "workspace-b",
                "other",
                "Chat",
                &ConversationMessageInput {
                    id: "other-message".to_string(),
                    role: "user".to_string(),
                    text: "Other workspace".to_string(),
                },
            )
            .unwrap();

        assert_eq!(
            store
                .prune_conversations("workspace-a", &["keep".to_string()])
                .unwrap(),
            1
        );
        assert_eq!(store.list_conversations("workspace-a").unwrap().len(), 1);
        assert_eq!(store.list_conversations("workspace-b").unwrap().len(), 1);
    }
}
