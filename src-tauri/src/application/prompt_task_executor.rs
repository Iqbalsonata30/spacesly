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
    AiWorkerEventCallback,
};
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
}

/// Backend authority that resolves trusted Chat/Edit runtime configuration and durable references.
pub trait PromptRuntimeResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        input: &TaskSessionInputV2,
    ) -> Result<ResolvedPromptTask, String>;
}

impl PromptRuntimeResolver for StoredAgentRuntimeResolver {
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        input: &TaskSessionInputV2,
    ) -> Result<ResolvedPromptTask, String> {
        if let TaskSessionInputV2::Chat(input) = input {
            self.verify_chat_message(
                envelope,
                &input.message_id,
                input.message_sequence,
                &input.message,
            )?;
        }
        let runtime = self.resolve_prompt_runtime(envelope)?;
        Ok(ResolvedPromptTask {
            runtime_profile_id: runtime.runtime_profile_id,
            config: runtime.config,
        })
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
                let reporter = context.event_reporter();
                let callback_open = Arc::new(Mutex::new(true));
                let callback_guard = CallbackGate {
                    open: callback_open.clone(),
                };
                let callback: AiWorkerEventCallback = Box::new(move |event| {
                    let open = callback_open.lock().map_err(|error| error.to_string())?;
                    if !*open {
                        return Err("Chat runtime callback is closed.".to_string());
                    }
                    emit_runtime_event(&reporter, event)
                });
                let result = self.runner.chat(
                    resolved.config,
                    AiWorkerChatRequest {
                        run_id: None,
                        message: input.message,
                        terminal_context: input.terminal_context,
                        context_revision: envelope.session.context_revision.clone(),
                        session_context: input.session_context,
                        session_key: Some(context.runtime_attempt_id()),
                    },
                    context.cancellation().shared_flag(),
                    callback,
                );
                drop(callback_guard);
                let result = result.map_err(TaskExecutionError::new)?;
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
                let result = self
                    .runner
                    .edit(
                        resolved.config,
                        edit_request(input),
                        context.cancellation().shared_flag(),
                    )
                    .map_err(TaskExecutionError::new)?;
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
    use crate::infrastructure::ai_worker::AiWorkerStreamEvent;
    use std::sync::Barrier;
    use std::time::Duration;
    use tempfile::tempdir;

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
            })
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
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: Vec::new(),
        }
    }
}
