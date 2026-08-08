use super::agent_task_executor::{emit_runtime_event, AgentTaskExecutor};
use super::execution_engine::{TaskExecutionContext, TaskExecutionError, TaskExecutor};
use super::stored_agent_runtime_resolver::StoredAgentRuntimeResolver;
use crate::domain::task_session::{
    ChatTaskResult, EditTaskResult, TaskEditInputV2, TaskExecutionOutput, TaskProgress,
    TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionEventKind, TaskSessionInputV2,
    TaskSessionKind,
};
use crate::infrastructure::ai_worker::{
    chat_ai_worker, chat_ai_worker_streaming, propose_ai_edit, AiEditContextFile, AiEditRequest,
    AiEditResult, AiEditSelection, AiWorkerChatRequest, AiWorkerChatResult, AiWorkerConfig,
    AiWorkerEventCallback, AiWorkerStreamEvent,
};
use crate::infrastructure::execution_store::ChatConversationSnapshot;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const MAX_PROMPT_RESULT_CONTENT_BYTES: usize = 256 * 1024;
const MAX_PROMPT_RESULT_SUMMARY_BYTES: usize = 64 * 1024;

/// Trusted runtime configuration reconstructed for one Chat/Edit assignment attempt.
pub struct ResolvedPromptTask {
    pub runtime_profile_id: String,
    pub config: AiWorkerConfig,
    pub chat_snapshot: Option<ChatConversationSnapshot>,
}

/// Backend authority that resolves trusted Chat/Edit runtime configuration and durable references.
pub trait PromptRuntimeResolver: Send + Sync + 'static {
    /// Resolves trusted runtime state and, for Chat, its exact durable model snapshot.
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        input: &TaskSessionInputV2,
    ) -> Result<ResolvedPromptTask, String>;

    /// Rejects a Chat result if its durable snapshot changed while the model was running.
    fn revalidate_chat(&self, snapshot: &ChatConversationSnapshot) -> Result<(), String>;
}

impl PromptRuntimeResolver for StoredAgentRuntimeResolver {
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        input: &TaskSessionInputV2,
    ) -> Result<ResolvedPromptTask, String> {
        let chat_snapshot = if let TaskSessionInputV2::Chat(input) = input {
            Some(self.resolve_chat_snapshot(
                envelope,
                &input.message_id,
                input.message_sequence,
                &input.message,
            )?)
        } else {
            None
        };
        let mut runtime = self.resolve_prompt_runtime(envelope)?;
        runtime.chat_snapshot = chat_snapshot;
        Ok(ResolvedPromptTask {
            runtime_profile_id: runtime.runtime_profile_id,
            config: runtime.config,
            chat_snapshot: runtime.chat_snapshot,
        })
    }

    fn revalidate_chat(&self, snapshot: &ChatConversationSnapshot) -> Result<(), String> {
        self.revalidate_chat_snapshot(snapshot)
    }
}

/// Chat/Edit runtime invocation boundary used by scheduler orchestration and tests.
pub trait PromptRuntimeRunner: Send + Sync + 'static {
    fn chat(
        &self,
        config: AiWorkerConfig,
        request: AiWorkerChatRequest,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerChatResult, String>;

    fn edit(
        &self,
        config: AiWorkerConfig,
        request: AiEditRequest,
        cancellation: Arc<AtomicBool>,
    ) -> Result<AiEditResult, String>;
}

/// Production Chat/Edit runner backed by the existing AI worker implementation.
pub struct AiWorkerPromptRuntimeRunner;

impl PromptRuntimeRunner for AiWorkerPromptRuntimeRunner {
    fn chat(
        &self,
        config: AiWorkerConfig,
        request: AiWorkerChatRequest,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerChatResult, String> {
        if config.runtime == "api" {
            tauri::async_runtime::block_on(chat_ai_worker_streaming(
                config,
                request,
                cancellation,
                on_event,
            ))
        } else {
            chat_ai_worker(config, request, cancellation, Some(on_event))
        }
    }

    fn edit(
        &self,
        config: AiWorkerConfig,
        request: AiEditRequest,
        cancellation: Arc<AtomicBool>,
    ) -> Result<AiEditResult, String> {
        propose_ai_edit(config, request, cancellation)
    }
}

struct CallbackGate {
    open: Arc<Mutex<bool>>,
}

impl Drop for CallbackGate {
    fn drop(&mut self) {
        if let Ok(mut open) = self.open.lock() {
            *open = false;
        }
    }
}

/// Scheduler executor for immutable Chat and Edit prompt inputs.
pub struct PromptTaskExecutor {
    resolver: Arc<dyn PromptRuntimeResolver>,
    runner: Arc<dyn PromptRuntimeRunner>,
}

impl PromptTaskExecutor {
    /// Creates a shared executor whose per-call state is owned by one assignment context.
    pub fn new(
        resolver: Arc<dyn PromptRuntimeResolver>,
        runner: Arc<dyn PromptRuntimeRunner>,
    ) -> Self {
        Self { resolver, runner }
    }

    fn execute_v2(
        &self,
        context: &TaskExecutionContext,
        envelope: crate::domain::task_session::TaskSessionEnvelopeV2,
    ) -> Result<TaskExecutionOutput, TaskExecutionError> {
        envelope.validate().map_err(TaskExecutionError::new)?;
        if prompt_input_digest(&envelope.prompt_input).map_err(TaskExecutionError::new)?
            != envelope.session.context_digest
        {
            return Err(TaskExecutionError::new(
                "Prompt input digest did not match the Task Session envelope.",
            ));
        }
        context.report_progress(
            TaskProgress {
                phase: "resolving_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_profile_id": envelope.session.runtime_profile_id }),
        )?;
        let runtime_preparation =
            crate::infrastructure::performance::span("runtime_preparation_ms", "agent_runtime")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("execution_attempt", context.attempt_id().to_string())
                .with_context("worker_id", context.worker_id().to_string());
        let mut resolved = self
            .resolver
            .resolve(&envelope.session, &envelope.prompt_input)
            .map_err(TaskExecutionError::blocked)?;
        if resolved.runtime_profile_id != envelope.session.runtime_profile_id
            || resolved.config.workspace_id != envelope.session.workspace_id
            || resolved.config.opencode_model != envelope.session.model
        {
            return Err(TaskExecutionError::new(
                "Resolved prompt runtime did not match the Task Session envelope.",
            ));
        }
        resolved.config.opencode_auto_approve = false;
        resolved.config.restrict_tools = true;
        resolved.config.fenced_tools_only = true;
        resolved.config.isolated_opencode_process = true;
        resolved.config.mcp_servers.clear();
        runtime_preparation.finish();
        context.ensure_current()?;
        context.report_progress(
            TaskProgress {
                phase: "executing_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_attempt_id": context.runtime_attempt_id() }),
        )?;
        match envelope.prompt_input {
            TaskSessionInputV2::Chat(input) => {
                let snapshot = resolved.chat_snapshot.ok_or_else(|| {
                    TaskExecutionError::new("Resolved Chat runtime is missing durable context.")
                })?;
                let final_message = snapshot
                    .final_user_message()
                    .map_err(TaskExecutionError::new)?;
                if final_message.id != input.message_id
                    || final_message.sequence != input.message_sequence
                    || final_message.text != input.message
                {
                    return Err(TaskExecutionError::new(
                        "Resolved Chat snapshot did not match the requested durable identity.",
                    ));
                }
                let authoritative_message = final_message.text.clone();
                let authoritative_context = snapshot
                    .prior_model_context()
                    .map_err(TaskExecutionError::new)?;
                let authoritative_revision = format!("{}:{}", snapshot.revision, snapshot.digest);
                let reporter = context.event_reporter();
                let buffered_events = Arc::new(Mutex::new(Vec::new()));
                let callback_open = Arc::new(Mutex::new(true));
                let callback_guard = CallbackGate {
                    open: callback_open.clone(),
                };
                let callback_events = buffered_events.clone();
                let callback: AiWorkerEventCallback = Box::new(move |event| {
                    let open = callback_open.lock().map_err(|error| error.to_string())?;
                    if !*open {
                        return Err("Chat runtime callback is closed.".to_string());
                    }
                    callback_events
                        .lock()
                        .map_err(|error| error.to_string())?
                        .push(event);
                    Ok(())
                });
                let provider_request =
                    crate::infrastructure::performance::span("provider_request_ms", "provider")
                        .with_context("task_session_id", context.session_id().0.to_string());
                let result = self.runner.chat(
                    resolved.config,
                    AiWorkerChatRequest {
                        run_id: None,
                        conversation_id: Some(snapshot.conversation_id.clone()),
                        message_id: Some(final_message.id.clone()),
                        message_sequence: Some(final_message.sequence),
                        message: authoritative_message,
                        terminal_context: None,
                        context_revision: Some(authoritative_revision),
                        session_context: Some(authoritative_context),
                        session_key: Some(context.runtime_attempt_id()),
                    },
                    context.cancellation().shared_flag(),
                    callback,
                );
                provider_request.finish();
                drop(callback_guard);
                let result = result.map_err(TaskExecutionError::new)?;
                self.resolver
                    .revalidate_chat(&snapshot)
                    .map_err(TaskExecutionError::blocked)?;
                let events = buffered_events
                    .lock()
                    .map_err(|error| TaskExecutionError::new(error.to_string()))?
                    .drain(..)
                    .collect();
                for event in coalesce_runtime_events(events) {
                    emit_runtime_event(&reporter, event).map_err(TaskExecutionError::new)?;
                }
                if result.message.len() > MAX_PROMPT_RESULT_CONTENT_BYTES {
                    return Err(TaskExecutionError::new(
                        "Chat Task Session result exceeds the durable message limit.",
                    ));
                }
                context.ensure_current()?;
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "chat_result_candidate",
                        "authoritative": false,
                        "conversation_id": envelope.session.conversation_id,
                        "message": result.message,
                    }),
                )?;
                Ok(TaskExecutionOutput::Chat(ChatTaskResult {
                    conversation_id: envelope.session.conversation_id.ok_or_else(|| {
                        TaskExecutionError::new("Chat Task Session conversation is required.")
                    })?,
                    message: result.message,
                }))
            }
            TaskSessionInputV2::Edit(input) => {
                let file_path = input.file_path.clone();
                let provider_request =
                    crate::infrastructure::performance::span("provider_request_ms", "provider")
                        .with_context("task_session_id", context.session_id().0.to_string());
                let result = self
                    .runner
                    .edit(
                        resolved.config,
                        edit_request(input),
                        context.cancellation().shared_flag(),
                    )
                    .map_err(TaskExecutionError::new)?;
                provider_request.finish();
                if result.content.len() > MAX_PROMPT_RESULT_CONTENT_BYTES
                    || result.summary.len() > MAX_PROMPT_RESULT_SUMMARY_BYTES
                {
                    return Err(TaskExecutionError::new(
                        "Edit Task Session result exceeds its durable result limit.",
                    ));
                }
                context.ensure_current()?;
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "edit_result_candidate",
                        "authoritative": false,
                        "file_path": file_path,
                        "summary": result.summary,
                        "content": result.content,
                    }),
                )?;
                Ok(TaskExecutionOutput::Edit(EditTaskResult {
                    file_path,
                    summary: result.summary,
                    content: result.content,
                }))
            }
        }
    }
}

fn coalesce_runtime_events(events: Vec<AiWorkerStreamEvent>) -> Vec<AiWorkerStreamEvent> {
    let mut coalesced = Vec::with_capacity(events.len());
    let mut text = String::new();
    let mut usage = None;
    for event in events {
        match event {
            AiWorkerStreamEvent::TextDelta(delta) => text.push_str(&delta),
            event @ AiWorkerStreamEvent::UsageUpdated { .. } if !text.is_empty() => {
                usage = Some(event);
            }
            event => {
                if !text.is_empty() {
                    coalesced.push(AiWorkerStreamEvent::TextDelta(std::mem::take(&mut text)));
                }
                if let Some(usage) = usage.take() {
                    coalesced.push(usage);
                }
                coalesced.push(event);
            }
        }
    }
    if !text.is_empty() {
        coalesced.push(AiWorkerStreamEvent::TextDelta(text));
    }
    if let Some(usage) = usage {
        coalesced.push(usage);
    }
    coalesced
}

impl TaskExecutor for PromptTaskExecutor {
    fn execute(
        &self,
        context: &TaskExecutionContext,
    ) -> Result<TaskExecutionOutput, TaskExecutionError> {
        context.ensure_current()?;
        let envelope = context
            .request()
            .envelope()
            .map_err(TaskExecutionError::new)?
            .ok_or_else(|| TaskExecutionError::new("Prompt task envelope is required."))?;
        match envelope {
            TaskSessionEnvelope::V2(envelope) => self.execute_v2(context, envelope),
            TaskSessionEnvelope::V1(_) => Err(TaskExecutionError::new(
                "PromptTaskExecutor requires a V2 Chat/Edit envelope.",
            )),
        }
    }
}

/// Routes scheduler assignments to kind-specific executors without sharing task-local state.
pub struct TaskSessionExecutor {
    agent: Arc<AgentTaskExecutor>,
    prompt: Arc<PromptTaskExecutor>,
}

impl TaskSessionExecutor {
    /// Creates a router over long-lived stateless executor adapters.
    pub fn new(agent: Arc<AgentTaskExecutor>, prompt: Arc<PromptTaskExecutor>) -> Self {
        Self { agent, prompt }
    }
}

impl TaskExecutor for TaskSessionExecutor {
    fn execute(
        &self,
        context: &TaskExecutionContext,
    ) -> Result<TaskExecutionOutput, TaskExecutionError> {
        let envelope = context
            .request()
            .envelope()
            .map_err(TaskExecutionError::new)?
            .ok_or_else(|| TaskExecutionError::new("Task Session envelope is required."))?;
        match envelope.session().kind {
            TaskSessionKind::Agent => self.agent.execute(context),
            TaskSessionKind::Chat | TaskSessionKind::Edit => self.prompt.execute(context),
        }
    }
}

/// Returns the canonical integrity digest for immutable Chat/Edit prompt input.
pub fn prompt_input_digest(input: &TaskSessionInputV2) -> Result<String, String> {
    let encoded = serde_json::to_vec(input)
        .map_err(|error| format!("Failed to encode prompt Task Session input: {error}"))?;
    Ok(format!(
        "sha256:{}",
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn edit_request(input: TaskEditInputV2) -> AiEditRequest {
    AiEditRequest {
        run_id: None,
        file_path: input.file_path,
        instruction: input.instruction,
        content: input.content,
        selection: input.selection.map(|selection| AiEditSelection {
            start_line: selection.start_line,
            start_character: selection.start_character,
            end_line: selection.end_line,
            end_character: selection.end_character,
            text: selection.text,
        }),
        context_files: input
            .context_files
            .into_iter()
            .map(|file| AiEditContextFile {
                file_path: file.file_path,
                content: file.content,
            })
            .collect(),
        diagnostics: input.diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::execution_engine::ExecutionEngine;
    use crate::domain::task_session::{
        TaskChatInputV2, TaskEditInputV2, TaskSessionEnvelopeV2, TaskSessionState,
    };
    use std::sync::Barrier;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn prompt_text_deltas_are_coalesced_with_latest_usage_retained() {
        let events = coalesce_runtime_events(vec![
            AiWorkerStreamEvent::TextDelta("one ".to_string()),
            AiWorkerStreamEvent::TextDelta("two".to_string()),
            AiWorkerStreamEvent::UsageUpdated {
                input_tokens: 10,
                output_tokens: 2,
            },
            AiWorkerStreamEvent::TextDelta(" three".to_string()),
        ]);

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            AiWorkerStreamEvent::TextDelta(text) if text == "one two three"
        ));
        assert!(matches!(
            &events[1],
            AiWorkerStreamEvent::UsageUpdated {
                input_tokens: 10,
                output_tokens: 2
            }
        ));
    }

    struct FakeResolver;

    impl PromptRuntimeResolver for FakeResolver {
        fn resolve(
            &self,
            envelope: &TaskSessionEnvelopeV1,
            _input: &TaskSessionInputV2,
        ) -> Result<ResolvedPromptTask, String> {
            Ok(ResolvedPromptTask {
                runtime_profile_id: envelope.runtime_profile_id.clone(),
                config: test_config(envelope),
                chat_snapshot: match _input {
                    TaskSessionInputV2::Chat(input) => Some(ChatConversationSnapshot {
                        workspace_id: envelope.workspace_id.clone(),
                        conversation_id: envelope.conversation_id.clone().unwrap(),
                        revision: input.message_sequence,
                        digest: "sha256:test".to_string(),
                        messages: vec![
                            crate::infrastructure::execution_store::ChatConversationMessage {
                                id: input.message_id.clone(),
                                sequence: input.message_sequence,
                                role: "user".to_string(),
                                text: input.message.clone(),
                            },
                        ],
                    }),
                    TaskSessionInputV2::Edit(_) => None,
                },
            })
        }

        fn revalidate_chat(&self, _snapshot: &ChatConversationSnapshot) -> Result<(), String> {
            Ok(())
        }
    }

    struct ConcurrentRunner {
        barrier: Arc<Barrier>,
        cancellation_ids: Arc<Mutex<Vec<usize>>>,
        chat_session_keys: Arc<Mutex<Vec<String>>>,
    }

    impl PromptRuntimeRunner for ConcurrentRunner {
        fn chat(
            &self,
            config: AiWorkerConfig,
            request: AiWorkerChatRequest,
            cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerChatResult, String> {
            assert!(config.isolated_opencode_process);
            assert!(config.mcp_servers.is_empty());
            assert_eq!(request.message, "hello");
            assert_eq!(request.terminal_context, None);
            assert_eq!(request.conversation_id.as_deref(), Some("conversation-1"));
            assert_eq!(request.message_id.as_deref(), Some("message-1"));
            assert_eq!(request.message_sequence, Some(1));
            assert!(request.session_context.as_deref().unwrap().contains("[]"));
            assert!(!request
                .session_context
                .as_deref()
                .unwrap()
                .contains("prior turns"));
            assert!(!request
                .context_revision
                .as_deref()
                .unwrap()
                .contains("workspace context"));
            self.cancellation_ids
                .lock()
                .expect("cancellation lock")
                .push(Arc::as_ptr(&cancellation) as usize);
            self.chat_session_keys
                .lock()
                .expect("session key lock")
                .push(request.session_key.expect("session key"));
            self.barrier.wait();
            on_event(AiWorkerStreamEvent::TextDelta("chat delta".to_string()))?;
            Ok(AiWorkerChatResult {
                run_id: String::new(),
                message: "assistant response".to_string(),
            })
        }

        fn edit(
            &self,
            config: AiWorkerConfig,
            request: AiEditRequest,
            cancellation: Arc<AtomicBool>,
        ) -> Result<AiEditResult, String> {
            assert!(config.isolated_opencode_process);
            assert!(config.mcp_servers.is_empty());
            assert_eq!(request.file_path, "src/main.rs");
            self.cancellation_ids
                .lock()
                .expect("cancellation lock")
                .push(Arc::as_ptr(&cancellation) as usize);
            self.barrier.wait();
            Ok(AiEditResult {
                run_id: String::new(),
                summary: "updated".to_string(),
                content: "fn main() {}".to_string(),
            })
        }
    }

    #[test]
    fn chat_and_edit_sessions_execute_concurrently_without_state_leaks() {
        let directory = tempdir().expect("temp directory");
        let cancellation_ids = Arc::new(Mutex::new(Vec::new()));
        let chat_session_keys = Arc::new(Mutex::new(Vec::new()));
        let executor = PromptTaskExecutor::new(
            Arc::new(FakeResolver),
            Arc::new(ConcurrentRunner {
                barrier: Arc::new(Barrier::new(2)),
                cancellation_ids: cancellation_ids.clone(),
                chat_session_keys: chat_session_keys.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let chat = engine
            .submit_envelope("chat", &chat_envelope())
            .expect("chat submitted");
        let edit = engine
            .submit_envelope("edit", &edit_envelope())
            .expect("edit submitted");

        let chat = engine
            .wait_for_terminal(chat.id, Duration::from_secs(5))
            .expect("chat completes");
        let edit = engine
            .wait_for_terminal(edit.id, Duration::from_secs(5))
            .expect("edit completes");
        assert_eq!(chat.state, TaskSessionState::Succeeded);
        assert_eq!(edit.state, TaskSessionState::Succeeded);
        assert_eq!(
            engine
                .task_session_result(chat.id)
                .expect("chat result query")
                .expect("chat result")
                .output,
            TaskExecutionOutput::Chat(ChatTaskResult {
                conversation_id: "conversation-1".to_string(),
                message: "assistant response".to_string(),
            })
        );
        assert_eq!(
            engine
                .task_session_result(edit.id)
                .expect("edit result query")
                .expect("edit result")
                .output,
            TaskExecutionOutput::Edit(EditTaskResult {
                file_path: "src/main.rs".to_string(),
                summary: "updated".to_string(),
                content: "fn main() {}".to_string(),
            })
        );
        let cancellation_ids = cancellation_ids.lock().expect("cancellation lock");
        assert_eq!(cancellation_ids.len(), 2);
        assert_ne!(cancellation_ids[0], cancellation_ids[1]);
        let chat_session_keys = chat_session_keys.lock().expect("session key lock");
        assert_eq!(chat_session_keys.len(), 1);
        assert!(chat_session_keys[0].contains("-attempt-"));
        assert!(engine
            .events_after(chat.id, 0)
            .expect("chat events")
            .iter()
            .any(|event| event.payload["type"] == "chat_result_candidate"));
        assert!(engine
            .events_after(edit.id, 0)
            .expect("edit events")
            .iter()
            .any(|event| event.payload["type"] == "edit_result_candidate"));
    }

    struct CapturingChatRunner {
        barrier: Option<Arc<Barrier>>,
        requests: Arc<Mutex<Vec<AiWorkerChatRequest>>>,
    }

    impl PromptRuntimeRunner for CapturingChatRunner {
        fn chat(
            &self,
            _config: AiWorkerConfig,
            request: AiWorkerChatRequest,
            _cancellation: Arc<AtomicBool>,
            _on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerChatResult, String> {
            self.requests.lock().unwrap().push(request);
            if let Some(barrier) = &self.barrier {
                barrier.wait();
            }
            Ok(AiWorkerChatResult {
                run_id: String::new(),
                message: "ok".to_string(),
            })
        }

        fn edit(
            &self,
            _config: AiWorkerConfig,
            _request: AiEditRequest,
            _cancellation: Arc<AtomicBool>,
        ) -> Result<AiEditResult, String> {
            unreachable!()
        }
    }

    struct ConversationResolver {
        reject_revalidation: bool,
    }

    impl PromptRuntimeResolver for ConversationResolver {
        fn resolve(
            &self,
            envelope: &TaskSessionEnvelopeV1,
            input: &TaskSessionInputV2,
        ) -> Result<ResolvedPromptTask, String> {
            let TaskSessionInputV2::Chat(input) = input else {
                unreachable!()
            };
            let conversation_id = envelope.conversation_id.clone().unwrap();
            Ok(ResolvedPromptTask {
                runtime_profile_id: envelope.runtime_profile_id.clone(),
                config: test_config(envelope),
                chat_snapshot: Some(ChatConversationSnapshot {
                    workspace_id: envelope.workspace_id.clone(),
                    conversation_id: conversation_id.clone(),
                    revision: input.message_sequence,
                    digest: format!("sha256:{conversation_id}"),
                    messages: vec![
                        crate::infrastructure::execution_store::ChatConversationMessage {
                            id: format!("{conversation_id}-prior"),
                            sequence: 1,
                            role: "user".to_string(),
                            text: format!("history:{conversation_id}"),
                        },
                        crate::infrastructure::execution_store::ChatConversationMessage {
                            id: input.message_id.clone(),
                            sequence: input.message_sequence,
                            role: "user".to_string(),
                            text: input.message.clone(),
                        },
                    ],
                }),
            })
        }

        fn revalidate_chat(&self, _snapshot: &ChatConversationSnapshot) -> Result<(), String> {
            if self.reject_revalidation {
                Err("durable head advanced".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn concurrent_chat_conversations_receive_only_their_own_backend_history() {
        let directory = tempdir().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executor = PromptTaskExecutor::new(
            Arc::new(ConversationResolver {
                reject_revalidation: false,
            }),
            Arc::new(CapturingChatRunner {
                barrier: Some(Arc::new(Barrier::new(2))),
                requests: requests.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .unwrap();
        let first = engine
            .submit_envelope(
                "first",
                &chat_envelope_for("conversation-a", "message-a", 2),
            )
            .unwrap();
        let second = engine
            .submit_envelope(
                "second",
                &chat_envelope_for("conversation-b", "message-b", 2),
            )
            .unwrap();
        engine
            .wait_for_terminal(first.id, Duration::from_secs(5))
            .unwrap();
        engine
            .wait_for_terminal(second.id, Duration::from_secs(5))
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            let conversation = request.conversation_id.as_deref().unwrap();
            let other = if conversation == "conversation-a" {
                "conversation-b"
            } else {
                "conversation-a"
            };
            let context = request.session_context.as_deref().unwrap();
            assert!(context.contains(&format!("history:{conversation}")));
            assert!(!context.contains(&format!("history:{other}")));
        }
    }

    #[test]
    fn chat_result_is_rejected_when_backend_snapshot_turns_stale() {
        let directory = tempdir().unwrap();
        let executor = PromptTaskExecutor::new(
            Arc::new(ConversationResolver {
                reject_revalidation: true,
            }),
            Arc::new(CapturingChatRunner {
                barrier: None,
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .unwrap();
        let chat = engine
            .submit_envelope("chat", &chat_envelope_for("conversation-a", "message-a", 2))
            .unwrap();
        let chat = engine
            .wait_for_terminal(chat.id, Duration::from_secs(5))
            .unwrap();
        assert_ne!(chat.state, TaskSessionState::Succeeded);
        assert!(engine.task_session_result(chat.id).unwrap().is_none());
    }

    fn chat_envelope() -> TaskSessionEnvelope {
        let prompt_input = TaskSessionInputV2::Chat(TaskChatInputV2 {
            message_id: "message-1".to_string(),
            message_sequence: 1,
            message: "hello".to_string(),
            terminal_context: Some("workspace context".to_string()),
            session_context: Some("prior turns".to_string()),
        });
        TaskSessionEnvelope::V2(TaskSessionEnvelopeV2 {
            session: base_session(TaskSessionKind::Chat, &prompt_input),
            prompt_input,
        })
    }

    fn chat_envelope_for(
        conversation_id: &str,
        message_id: &str,
        sequence: u64,
    ) -> TaskSessionEnvelope {
        let prompt_input = TaskSessionInputV2::Chat(TaskChatInputV2 {
            message_id: message_id.to_string(),
            message_sequence: sequence,
            message: format!("message:{conversation_id}"),
            terminal_context: Some("malicious terminal context".to_string()),
            session_context: Some("malicious renderer history".to_string()),
        });
        let mut session = base_session(TaskSessionKind::Chat, &prompt_input);
        session.conversation_id = Some(conversation_id.to_string());
        TaskSessionEnvelope::V2(TaskSessionEnvelopeV2 {
            session,
            prompt_input,
        })
    }

    fn edit_envelope() -> TaskSessionEnvelope {
        let prompt_input = TaskSessionInputV2::Edit(TaskEditInputV2 {
            file_path: "src/main.rs".to_string(),
            instruction: "format main".to_string(),
            content: "fn main(){ }".to_string(),
            selection: None,
            context_files: Vec::new(),
            diagnostics: Vec::new(),
        });
        TaskSessionEnvelope::V2(TaskSessionEnvelopeV2 {
            session: base_session(TaskSessionKind::Edit, &prompt_input),
            prompt_input,
        })
    }

    fn base_session(kind: TaskSessionKind, input: &TaskSessionInputV2) -> TaskSessionEnvelopeV1 {
        TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind,
            subject_id: None,
            conversation_id: (kind == TaskSessionKind::Chat).then(|| "conversation-1".to_string()),
            execution_run_id: None,
            context_digest: prompt_input_digest(input).expect("input digest"),
            runtime_profile_id: "profile-1".to_string(),
            model: "openai/gpt-5".to_string(),
            connector_ids: Vec::new(),
            requested_capabilities: Vec::new(),
            prompt_template_version: "prompt-v2".to_string(),
            context_revision: Some("1".to_string()),
            rules_revision: Some("rules-v1".to_string()),
            skills_revision: Some("skills-v1".to_string()),
        }
    }

    fn test_config(envelope: &TaskSessionEnvelopeV1) -> AiWorkerConfig {
        AiWorkerConfig {
            workspace_id: envelope.workspace_id.clone(),
            runtime: "opencode".to_string(),
            provider_name: "OpenAI".to_string(),
            provider_id: "openai".to_string(),
            base_url: String::new(),
            api_style: String::new(),
            api_key: String::new(),
            model: "gpt-5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_model: envelope.model.clone(),
            opencode_workdir: Some("/tmp".to_string()),
            opencode_auto_approve: true,
            agent_rules: String::new(),
            agent_skills: String::new(),
            governance_schema_version: 0,
            skill_catalog: Vec::new(),
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: Vec::new(),
        }
    }
}
