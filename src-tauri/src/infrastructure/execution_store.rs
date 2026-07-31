use crate::application::execution_engine::CompletionProjector;
use crate::domain::execution::{ExecutionRun, StepRun};
use crate::domain::task_session::{AgentTaskCompletionStatus, TaskExecutionOutput};
use crate::infrastructure::scheduler_store::StagedCompletion;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
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
        authority TEXT NOT NULL DEFAULT 'legacy_renderer',
        UNIQUE(conversation_id, sequence),
       FOREIGN KEY (conversation_id) REFERENCES conversations(conversation_id)
         ON DELETE CASCADE
     );
      CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation
        ON conversation_messages(conversation_id, sequence);";

const MESSAGE_AUTHORITY_RENDERER: &str = "renderer";
const MESSAGE_AUTHORITY_BACKEND: &str = "backend";

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

/// One durable user or agent message visible to the Chat model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChatConversationMessage {
    pub id: String,
    pub sequence: u64,
    pub role: String,
    pub text: String,
}

/// Backend-owned, immutable model context for one exact durable Chat head.
///
/// `revision` is the durable conversation head sequence. `digest` covers only
/// stable ownership fields and ordered model-visible messages; presentation
/// metadata and system messages are deliberately excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatConversationSnapshot {
    pub workspace_id: String,
    pub conversation_id: String,
    pub revision: u64,
    pub digest: String,
    pub messages: Vec<ChatConversationMessage>,
}

impl ChatConversationSnapshot {
    /// Returns the exact final durable user message owned by this snapshot.
    pub fn final_user_message(&self) -> Result<&ChatConversationMessage, String> {
        self.messages
            .last()
            .filter(|message| message.role == "user" && message.sequence == self.revision)
            .ok_or_else(|| "Chat snapshot does not end with its durable user message.".to_string())
    }

    /// Renders prior durable model-visible messages in deterministic JSON form.
    pub fn prior_model_context(&self) -> Result<String, String> {
        self.final_user_message()?;
        serde_json::to_string(&self.messages[..self.messages.len().saturating_sub(1)])
            .map(|history| format!("Durable conversation history (authoritative):\n{history}"))
            .map_err(|error| format!("Failed to render durable Chat history: {error}"))
    }
}

#[derive(Serialize)]
struct CanonicalChatSnapshot<'a> {
    version: u8,
    workspace_id: &'a str,
    conversation_id: &'a str,
    revision: u64,
    messages: &'a [ChatConversationMessage],
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
        migrate_conversation_authority(&connection)?;
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
        let current = transaction
            .query_row(
                "SELECT revision, status FROM execution_runs WHERE run_id = ?1",
                params![run.run_id],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| format!("Failed to read execution revision: {error}"))?;
        if let Some((current_revision, current_status)) = current {
            if current_revision != run.revision {
                return Err(format!(
                    "Execution run '{}' is stale (expected revision {}, current revision {}). Reload before saving.",
                    run.run_id, run.revision, current_revision
                ));
            }
            if is_terminal_status(&current_status) && run.status != current_status {
                return Err(format!(
                    "Execution run '{}' terminal status '{}' is immutable.",
                    run.run_id, current_status
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
                self.append_conversation_message_with_authority(
                    &completion.workspace_id,
                    &completion.conversation_id,
                    &title,
                    &ConversationMessageInput {
                        id: format!("{}:assistant", completion.projection_id),
                        role: "agent".to_string(),
                        text: result.message.clone(),
                    },
                    MESSAGE_AUTHORITY_BACKEND,
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
                   (message_id, conversation_id, sequence, role, text, created_at, authority)
                 VALUES (?1, ?2, ?3, 'agent', ?4, ?5, ?6)",
                params![
                    message_id,
                    completion.conversation_id,
                    sequence,
                    message_text,
                    now,
                    MESSAGE_AUTHORITY_BACKEND
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
                         OR (status = 'running' AND (lease_expires_at IS NULL OR lease_expires_at < ?1)))
                    AND EXISTS (
                      SELECT 1 FROM execution_runs
                       WHERE execution_runs.run_id = step_runs.run_id
                         AND execution_runs.status NOT IN ('blocked', 'failed', 'completed', 'cancelled')
                    )",
                params![now.to_string(), owner, expires, run_id, step_id],
            )
            .map_err(|error| format!("Failed to claim execution step: {error}"))?;
        if updated == 0 {
            return Err(format!(
                "Execution step {step_id} is already claimed or unavailable."
            ));
        }
        let run_updated = transaction
            .execute(
                "UPDATE execution_runs SET status = 'running', updated_at = ?1,
                 revision = revision + 1 WHERE run_id = ?2
                   AND status NOT IN ('blocked', 'failed', 'completed', 'cancelled')",
                params![now.to_string(), run_id],
            )
            .map_err(|error| format!("Failed to update claimed execution run: {error}"))?;
        if run_updated != 1 {
            return Err("Execution run is terminal or unavailable.".to_string());
        }
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
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start step completion: {error}"))?;
        let now = now_millis()?.to_string();
        let updated = transaction
            .execute(
                "UPDATE step_runs SET status = ?1, completed_at = ?2, summary = ?3,
                 lease_owner = NULL, lease_expires_at = NULL, updated_at = ?2
                 WHERE run_id = ?4 AND step_id = ?5 AND lease_owner = ?6",
                params![status, now, summary, run_id, step_id, owner],
            )
            .map_err(|error| format!("Failed to finish execution step: {error}"))?;
        if updated == 0 {
            return Err(format!(
                "Execution step {step_id} lease is not owned by this worker."
            ));
        }
        if matches!(status, "blocked" | "failed" | "cancelled") {
            let run_updated = transaction
                .execute(
                    "UPDATE execution_runs
                        SET status = ?1, current_step_ids_json = '[]', completed_at = ?2,
                            updated_at = ?2, revision = revision + 1
                      WHERE run_id = ?3 AND status = 'running'",
                    params![status, now, run_id],
                )
                .map_err(|error| format!("Failed to terminalize execution run: {error}"))?;
            if run_updated != 1 {
                return Err(
                    "Execution run was not running during terminal step completion.".to_string(),
                );
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit step completion: {error}"))
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

    /// Resolves the authoritative model context for an exact durable Chat user message.
    ///
    /// Ownership, head equality, contiguous rows, and final message identity are checked
    /// while holding one store lock. System rows count toward the revision and continuity
    /// check. System rows and agent rows without backend authority are presentation-only
    /// and never enter model history or its digest.
    pub fn resolve_chat_snapshot(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        message_id: &str,
        message_sequence: u64,
        message_text: &str,
    ) -> Result<ChatConversationSnapshot, String> {
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("Failed to start durable Chat snapshot read: {error}"))?;
        let snapshot = resolve_chat_snapshot_in(
            &transaction,
            workspace_id,
            conversation_id,
            message_id,
            message_sequence,
            message_text,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to complete durable Chat snapshot read: {error}"))?;
        Ok(snapshot)
    }

    /// Revalidates that a Chat snapshot still represents the current durable head and digest.
    pub fn revalidate_chat_snapshot(
        &self,
        snapshot: &ChatConversationSnapshot,
    ) -> Result<(), String> {
        let snapshot_digest = chat_snapshot_digest(
            &snapshot.workspace_id,
            &snapshot.conversation_id,
            snapshot.revision,
            &snapshot.messages,
        )?;
        if snapshot_digest != snapshot.digest {
            return Err("Resolved Chat snapshot digest is invalid.".to_string());
        }
        let final_message = snapshot.final_user_message()?;
        let current = self.resolve_chat_snapshot(
            &snapshot.workspace_id,
            &snapshot.conversation_id,
            &final_message.id,
            snapshot.revision,
            &final_message.text,
        )?;
        if current.digest != snapshot.digest {
            return Err("Durable Chat context changed after runtime resolution.".to_string());
        }
        Ok(())
    }

    /// Atomically appends a backend-owned assistant result if the supplied Chat snapshot
    /// is still current. The existing conversation title is never changed, and retries
    /// with the same message ID and content return the original durable row.
    pub(crate) fn append_chat_assistant_if_current(
        &self,
        snapshot: &ChatConversationSnapshot,
        message_id: &str,
        text: &str,
    ) -> Result<ConversationMessageRecord, String> {
        let input = ConversationMessageInput {
            id: message_id.to_string(),
            role: "agent".to_string(),
            text: text.to_string(),
        };
        validate_conversation_message(&input)?;
        let snapshot_digest = chat_snapshot_digest(
            &snapshot.workspace_id,
            &snapshot.conversation_id,
            snapshot.revision,
            &snapshot.messages,
        )?;
        if snapshot_digest != snapshot.digest {
            return Err("Resolved Chat snapshot digest is invalid.".to_string());
        }
        let final_message = snapshot.final_user_message()?;
        let now = now_millis()?;
        let mut connection = self.connection.lock().map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start assistant append transaction: {error}"))?;

        let existing = transaction
            .query_row(
                "SELECT messages.conversation_id, messages.sequence, messages.role, messages.text,
                        messages.created_at, conversations.workspace_id, messages.authority
                   FROM conversation_messages messages
                   JOIN conversations conversations
                     ON conversations.conversation_id = messages.conversation_id
                  WHERE messages.message_id = ?1",
                params![message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to check assistant message idempotency: {error}"))?;
        if let Some((
            conversation_id,
            sequence,
            role,
            stored_text,
            created_at,
            workspace_id,
            authority,
        )) = existing
        {
            if conversation_id != snapshot.conversation_id
                || workspace_id != snapshot.workspace_id
                || sequence != snapshot.revision + 1
                || role != "agent"
                || stored_text != text
                || authority != MESSAGE_AUTHORITY_BACKEND
            {
                return Err("Message ID is already bound to different content.".to_string());
            }
            transaction.commit().map_err(|error| {
                format!("Failed to commit idempotent assistant message: {error}")
            })?;
            return Ok(ConversationMessageRecord {
                id: message_id.to_string(),
                conversation_id,
                sequence,
                role,
                text: stored_text,
                created_at,
            });
        }

        let current = resolve_chat_snapshot_in(
            &transaction,
            &snapshot.workspace_id,
            &snapshot.conversation_id,
            &final_message.id,
            snapshot.revision,
            &final_message.text,
        )?;
        if current.digest != snapshot.digest {
            return Err("Durable Chat context changed after runtime resolution.".to_string());
        }
        let sequence = snapshot.revision + 1;
        transaction
            .execute(
                "INSERT INTO conversation_messages
                   (message_id, conversation_id, sequence, role, text, created_at, authority)
                 VALUES (?1, ?2, ?3, 'agent', ?4, ?5, ?6)",
                params![
                    message_id,
                    snapshot.conversation_id,
                    sequence,
                    text,
                    now,
                    MESSAGE_AUTHORITY_BACKEND
                ],
            )
            .map_err(|error| format!("Failed to append assistant message: {error}"))?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1
                  WHERE conversation_id = ?2 AND workspace_id = ?3",
                params![now, snapshot.conversation_id, snapshot.workspace_id],
            )
            .map_err(|error| format!("Failed to update assistant conversation: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit assistant message: {error}"))?;
        Ok(ConversationMessageRecord {
            id: message_id.to_string(),
            conversation_id: snapshot.conversation_id.clone(),
            sequence,
            role: "agent".to_string(),
            text: text.to_string(),
            created_at: now,
        })
    }

    pub fn append_conversation_message(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        title: &str,
        input: &ConversationMessageInput,
    ) -> Result<ConversationMessageRecord, String> {
        self.append_conversation_message_with_authority(
            workspace_id,
            conversation_id,
            title,
            input,
            MESSAGE_AUTHORITY_RENDERER,
        )
    }

    fn append_conversation_message_with_authority(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        title: &str,
        input: &ConversationMessageInput,
        authority: &str,
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
                "SELECT conversation_id, sequence, role, text, created_at, authority
                  FROM conversation_messages WHERE message_id = ?1",
                params![input.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to check message idempotency: {error}"))?;
        if let Some((existing_conversation, sequence, role, text, created_at, stored_authority)) =
            existing
        {
            if existing_conversation != conversation_id
                || role != input.role
                || text != input.text
                || (stored_authority != authority
                    && !(authority == MESSAGE_AUTHORITY_RENDERER
                        && stored_authority == "legacy_renderer"))
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
                 (message_id, conversation_id, sequence, role, text, created_at, authority)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    input.id,
                    conversation_id,
                    sequence,
                    input.role,
                    input.text,
                    now,
                    authority
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

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "blocked" | "failed" | "completed" | "cancelled")
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

fn migrate_conversation_authority(connection: &Connection) -> Result<(), String> {
    let has_authority = connection
        .query_row(
            "SELECT 1 FROM pragma_table_info('conversation_messages') WHERE name = 'authority'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| format!("Failed to inspect conversation message schema: {error}"))?
        .is_some();
    if !has_authority {
        // Historical rows must remain presentation-only after migration.
        connection
            .execute(
                "ALTER TABLE conversation_messages
                 ADD COLUMN authority TEXT NOT NULL DEFAULT 'legacy_renderer'",
                [],
            )
            .map_err(|error| {
                format!("Failed to migrate conversation message authority: {error}")
            })?;
    }
    Ok(())
}

fn resolve_chat_snapshot_in(
    connection: &Connection,
    workspace_id: &str,
    conversation_id: &str,
    message_id: &str,
    message_sequence: u64,
    message_text: &str,
) -> Result<ChatConversationSnapshot, String> {
    if !conversation_exists_in(connection, workspace_id, conversation_id)? {
        return Err("Conversation does not belong to this workspace.".to_string());
    }
    let head = connection
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM conversation_messages
             WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("Failed to read durable Chat head: {error}"))?;
    if head > message_sequence {
        return Err(
            "Chat message is stale because the durable conversation head advanced.".to_string(),
        );
    }
    if head != message_sequence || head == 0 {
        return Err(
            "Chat message sequence does not match the durable conversation head.".to_string(),
        );
    }

    let mut statement = connection
        .prepare(
            "SELECT message_id, sequence, role, text, authority FROM conversation_messages
             WHERE conversation_id = ?1 ORDER BY sequence",
        )
        .map_err(|error| format!("Failed to prepare durable Chat snapshot: {error}"))?;
    let rows = statement
        .query_map(params![conversation_id], |row| {
            Ok((
                ChatConversationMessage {
                    id: row.get(0)?,
                    sequence: row.get(1)?,
                    role: row.get(2)?,
                    text: row.get(3)?,
                },
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| format!("Failed to query durable Chat snapshot: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to decode durable Chat snapshot: {error}"))?;
    if rows.len() as u64 != head
        || rows
            .iter()
            .enumerate()
            .any(|(index, (row, _))| row.sequence != index as u64 + 1)
    {
        return Err("Durable Chat message sequence is not contiguous.".to_string());
    }
    let final_row = &rows.last().expect("non-empty durable Chat rows").0;
    if final_row.id != message_id || final_row.role != "user" || final_row.text != message_text {
        return Err("Chat request does not match the final durable user message.".to_string());
    }
    let messages = rows
        .into_iter()
        .filter(|(message, authority)| {
            message.role == "user"
                || (message.role == "agent" && authority == MESSAGE_AUTHORITY_BACKEND)
        })
        .map(|(message, _)| message)
        .collect::<Vec<_>>();
    let digest = chat_snapshot_digest(workspace_id, conversation_id, head, &messages)?;
    Ok(ChatConversationSnapshot {
        workspace_id: workspace_id.to_string(),
        conversation_id: conversation_id.to_string(),
        revision: head,
        digest,
        messages,
    })
}

fn chat_snapshot_digest(
    workspace_id: &str,
    conversation_id: &str,
    revision: u64,
    messages: &[ChatConversationMessage],
) -> Result<String, String> {
    let canonical = serde_json::to_vec(&CanonicalChatSnapshot {
        version: 1,
        workspace_id,
        conversation_id,
        revision,
        messages,
    })
    .map_err(|error| format!("Failed to serialize canonical Chat snapshot: {error}"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
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
        connection
            .execute_batch(&format!(
                "{CONVERSATION_SCHEMA}
                 CREATE TABLE execution_contracts (
                   contract_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
                   version INTEGER NOT NULL, payload_json TEXT NOT NULL, created_at TEXT NOT NULL
                 );
                 CREATE TABLE execution_runs (
                   run_id TEXT PRIMARY KEY, contract_id TEXT NOT NULL, status TEXT NOT NULL,
                   current_step_ids_json TEXT NOT NULL, started_at TEXT NOT NULL, completed_at TEXT,
                   updated_at TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE step_runs (
                   run_id TEXT NOT NULL, step_id TEXT NOT NULL, status TEXT NOT NULL,
                   attempt INTEGER NOT NULL, started_at TEXT, completed_at TEXT, summary TEXT,
                   lease_owner TEXT, lease_expires_at INTEGER, updated_at TEXT NOT NULL,
                   PRIMARY KEY (run_id, step_id)
                 );"
            ))
            .unwrap();
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
        let (status, provenance, authority, receipts): (String, String, String, u64) = connection
            .query_row(
                "SELECT steps.status, steps.task_session_projection_id,
                        (SELECT authority FROM conversation_messages
                          WHERE message_id = 'projection-1:agent'),
                        (SELECT COUNT(*) FROM task_completion_projection_receipts)
                   FROM step_runs steps
                  WHERE steps.run_id = 'run-1' AND steps.step_id = 'worker.execute'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(provenance, "projection-1");
        assert_eq!(authority, MESSAGE_AUTHORITY_BACKEND);
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
        let authority: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT authority FROM conversation_messages
                  WHERE message_id = 'projection-chat:assistant'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(authority, MESSAGE_AUTHORITY_BACKEND);
    }

    #[test]
    fn terminal_step_completion_atomically_terminalizes_execution_run() {
        let store = test_store();
        for status in ["failed", "blocked", "cancelled"] {
            let run_id = format!("run-{status}");
            let contract_id = format!("contract-{status}");
            let run = ExecutionRun {
                run_id: run_id.clone(),
                contract: serde_json::json!({
                    "contract_id": contract_id,
                    "task_id": format!("task-{status}"),
                    "workspace_id": "workspace-1",
                    "version": 1,
                    "created_at": "2026-07-31T00:00:00Z"
                }),
                status: "pending".to_string(),
                current_step_ids: vec!["worker.execute".to_string()],
                step_runs: BTreeMap::from([(
                    "worker.execute".to_string(),
                    StepRun {
                        step_id: "worker.execute".to_string(),
                        status: "ready".to_string(),
                        attempt: 0,
                        started_at: None,
                        completed_at: None,
                        summary: None,
                        lease_owner: None,
                        lease_expires_at: None,
                    },
                )]),
                started_at: "2026-07-31T00:00:00Z".to_string(),
                completed_at: None,
                revision: 0,
            };
            store.save(&run).unwrap();
            store
                .claim_step(&run_id, "worker.execute", "worker-1", 60_000)
                .unwrap();
            store
                .finish_step(
                    &run_id,
                    "worker.execute",
                    "worker-1",
                    status,
                    Some("External tool failed. Cause: connection refused."),
                )
                .unwrap();

            let completed = store.get(&run_id).unwrap().unwrap();
            assert_eq!(completed.status, status);
            assert!(completed.completed_at.is_some());
            assert!(completed.current_step_ids.is_empty());
            let step = &completed.step_runs["worker.execute"];
            assert_eq!(step.status, status);
            assert_eq!(
                step.summary.as_deref(),
                Some("External tool failed. Cause: connection refused.")
            );
            assert!(step.lease_owner.is_none());
            assert!(step.lease_expires_at.is_none());

            let mut restarted = completed.clone();
            restarted.status = "running".to_string();
            assert!(store.save(&restarted).is_err());
            assert!(store
                .claim_step(&run_id, "worker.execute", "worker-2", 60_000)
                .is_err());
        }
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
        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "message-2", 2, "Hi")
            .unwrap_err();
        assert!(snapshot.contains("final durable user"));
    }

    fn append(store: &ExecutionStore, conversation: &str, id: &str, role: &str, text: &str) {
        store
            .append_conversation_message(
                "workspace-a",
                conversation,
                "Chat title",
                &ConversationMessageInput {
                    id: id.to_string(),
                    role: role.to_string(),
                    text: text.to_string(),
                },
            )
            .unwrap();
    }

    #[test]
    fn chat_snapshot_digest_is_deterministic_and_ignores_presentation_metadata() {
        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        append(&store, "conversation-a", "agent-1", "agent", "Hi");
        append(&store, "conversation-a", "user-2", "user", "Continue");
        let first = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-2", 3, "Continue")
            .unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE conversations SET title = 'Renamed', created_at = 99, updated_at = 100
                     WHERE conversation_id = 'conversation-a'",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE conversation_messages SET created_at = created_at + 1000
                     WHERE conversation_id = 'conversation-a'",
                    [],
                )
                .unwrap();
        }
        let second = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-2", 3, "Continue")
            .unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.revision, 3);
        assert_eq!(
            first
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-1", "user-2"]
        );
        store.revalidate_chat_snapshot(&first).unwrap();
    }

    #[test]
    fn chat_snapshot_is_conversation_scoped_and_rejects_stale_head() {
        let store = test_store();
        append(&store, "conversation-a", "a-user", "user", "A");
        append(&store, "conversation-b", "b-user", "user", "B");
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "b-user", 1, "B")
            .is_err());

        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "a-user", 1, "A")
            .unwrap();
        append(&store, "conversation-a", "a-agent", "agent", "new head");
        assert!(store.revalidate_chat_snapshot(&snapshot).is_err());
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "a-user", 1, "A")
            .unwrap_err()
            .contains("stale"));
    }

    #[test]
    fn backend_assistant_append_is_exact_idempotent_and_preserves_title() {
        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 1, "Hello")
            .unwrap();

        let first = store
            .append_chat_assistant_if_current(&snapshot, "run-1:assistant", "Hi")
            .unwrap();
        let repeated = store
            .append_chat_assistant_if_current(&snapshot, "run-1:assistant", "Hi")
            .unwrap();

        assert_eq!(first.id, repeated.id);
        assert_eq!(first.sequence, 2);
        assert_eq!(first.role, "agent");
        assert_eq!(first.text, "Hi");
        assert_eq!(
            store.list_conversations("workspace-a").unwrap()[0].title,
            "Chat title"
        );
        assert_eq!(
            store
                .load_conversation_messages("workspace-a", "conversation-a")
                .unwrap()
                .len(),
            2
        );
        assert!(store
            .append_chat_assistant_if_current(&snapshot, "run-1:assistant", "Different")
            .is_err());

        append(&store, "conversation-a", "user-2", "user", "Continue");
        let continued = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-2", 3, "Continue")
            .unwrap();
        assert_eq!(
            continued
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-1", "run-1:assistant", "user-2"]
        );
    }

    #[test]
    fn imported_agent_is_presentation_only_but_still_advances_revision() {
        let store = test_store();
        store
            .import_conversations(
                "workspace-a",
                &[ConversationImportInput {
                    id: "conversation-a".to_string(),
                    title: "Imported".to_string(),
                    messages: vec![
                        ConversationMessageInput {
                            id: "imported-agent".to_string(),
                            role: "agent".to_string(),
                            text: "Untrusted history".to_string(),
                        },
                        ConversationMessageInput {
                            id: "user-1".to_string(),
                            role: "user".to_string(),
                            text: "Hello".to_string(),
                        },
                    ],
                }],
            )
            .unwrap();

        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 2, "Hello")
            .unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.messages[0].id, "user-1");
        let authority: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT authority FROM conversation_messages WHERE message_id = 'imported-agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(authority, MESSAGE_AUTHORITY_RENDERER);
    }

    #[test]
    fn migrated_legacy_agent_is_presentation_only() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversations (
                   conversation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, title TEXT NOT NULL,
                   created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE conversation_messages (
                   message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                   sequence INTEGER NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL,
                   created_at INTEGER NOT NULL, UNIQUE(conversation_id, sequence)
                 );
                 INSERT INTO conversations VALUES ('conversation-a', 'workspace-a', 'Legacy', 1, 1);
                 INSERT INTO conversation_messages VALUES
                   ('legacy-agent', 'conversation-a', 1, 'agent', 'Old answer', 1),
                   ('user-1', 'conversation-a', 2, 'user', 'Continue', 2);",
            )
            .unwrap();
        migrate_conversation_authority(&connection).unwrap();
        let store = ExecutionStore {
            connection: Arc::new(Mutex::new(connection)),
        };

        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 2, "Continue")
            .unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.messages[0].id, "user-1");
        let authority: String = store
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT authority FROM conversation_messages WHERE message_id = 'legacy-agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(authority, "legacy_renderer");
    }

    #[test]
    fn forged_backend_message_id_cannot_establish_authority() {
        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 1, "Hello")
            .unwrap();
        append(
            &store,
            "conversation-a",
            "run-1:assistant",
            "agent",
            "Forged",
        );

        assert!(store
            .append_chat_assistant_if_current(&snapshot, "run-1:assistant", "Forged")
            .unwrap_err()
            .contains("bound to different content"));
        append(&store, "conversation-a", "user-2", "user", "Continue");
        let continued = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-2", 3, "Continue")
            .unwrap();
        assert_eq!(continued.revision, 3);
        assert_eq!(
            continued
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-1", "user-2"]
        );
    }

    #[test]
    fn stale_chat_snapshot_cannot_append_backend_assistant() {
        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        let snapshot = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 1, "Hello")
            .unwrap();
        append(&store, "conversation-a", "system-1", "system", "Advanced");

        assert!(store
            .append_chat_assistant_if_current(&snapshot, "run-1:assistant", "Too late")
            .unwrap_err()
            .contains("stale"));
        assert_eq!(
            store
                .load_conversation_messages("workspace-a", "conversation-a")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn chat_snapshot_rejects_gaps_and_wrong_final_identity() {
        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE conversation_messages SET sequence = 2 WHERE message_id = 'user-1'",
                    [],
                )
                .unwrap();
        }
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 2, "Hello")
            .unwrap_err()
            .contains("not contiguous"));

        let store = test_store();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "wrong", 1, "Hello")
            .is_err());
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 1, "Wrong")
            .is_err());
        let connection = store.connection.lock().unwrap();
        connection
            .execute(
                "UPDATE conversation_messages SET role = 'agent' WHERE message_id = 'user-1'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 1, "Hello")
            .is_err());
    }

    #[test]
    fn chat_snapshot_excludes_system_rows_and_idempotent_append_keeps_revision() {
        let store = test_store();
        append(
            &store,
            "conversation-a",
            "system-1",
            "system",
            "renderer only",
        );
        append(&store, "conversation-a", "user-1", "user", "Hello");
        let first = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 2, "Hello")
            .unwrap();
        append(&store, "conversation-a", "user-1", "user", "Hello");
        let second = store
            .resolve_chat_snapshot("workspace-a", "conversation-a", "user-1", 2, "Hello")
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.messages.len(), 1);
        assert_eq!(first.messages[0].id, "user-1");
        assert!(!first
            .prior_model_context()
            .unwrap()
            .contains("renderer only"));
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
            .resolve_chat_snapshot("workspace-a", "conversation-a", "message-1", 1, "Private")
            .is_ok());
        assert!(store
            .resolve_chat_snapshot("workspace-b", "conversation-a", "message-1", 1, "Private")
            .is_err());
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
