use super::execution_engine::{
    TaskEventReporter, TaskExecutionContext, TaskExecutionError, TaskExecutor,
};
use crate::domain::task_session::{
    TaskProgress, TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionEventKind, TaskSessionKind,
};
use crate::infrastructure::ai_worker::{
    execute_ai_worker_task, AiWorkerCompletionStatus, AiWorkerConfig, AiWorkerEventCallback,
    AiWorkerStreamEvent, AiWorkerTask, AiWorkerTaskResult,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

struct RuntimeCallbackGate {
    open: Arc<Mutex<bool>>,
}

impl Drop for RuntimeCallbackGate {
    fn drop(&mut self) {
        if let Ok(mut open) = self.open.lock() {
            *open = false;
        }
    }
}

/// Trusted configuration and contract reconstructed from one durable Task Session envelope.
pub struct ResolvedAgentTask {
    pub runtime_profile_id: String,
    pub config: AiWorkerConfig,
    pub task: AiWorkerTask,
}

/// Backend authority that resolves profiles, contracts, secrets, and trusted workspace paths.
pub trait AgentRuntimeResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        runtime_attempt_id: &str,
    ) -> Result<ResolvedAgentTask, String>;
}

/// Provider/OpenCode invocation boundary used to test orchestration without external processes.
pub trait AgentRuntimeRunner: Send + Sync + 'static {
    fn execute(
        &self,
        config: AiWorkerConfig,
        task: AiWorkerTask,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerTaskResult, String>;
}

/// Production runner backed by the existing provider/OpenCode implementation.
pub struct AiWorkerRuntimeRunner;

impl AgentRuntimeRunner for AiWorkerRuntimeRunner {
    fn execute(
        &self,
        config: AiWorkerConfig,
        task: AiWorkerTask,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerTaskResult, String> {
        execute_ai_worker_task(config, task, cancellation, Some(on_event))
    }
}

/// Scheduler executor that applies assignment fencing before invoking a real Agent runtime.
pub struct AgentTaskExecutor {
    resolver: Arc<dyn AgentRuntimeResolver>,
    runner: Arc<dyn AgentRuntimeRunner>,
}

impl AgentTaskExecutor {
    pub fn new(
        resolver: Arc<dyn AgentRuntimeResolver>,
        runner: Arc<dyn AgentRuntimeRunner>,
    ) -> Self {
        Self { resolver, runner }
    }
}

impl TaskExecutor for AgentTaskExecutor {
    fn execute(&self, context: &TaskExecutionContext) -> Result<(), TaskExecutionError> {
        context.ensure_current()?;
        let envelope = context
            .request()
            .envelope()
            .map_err(TaskExecutionError::new)?
            .ok_or_else(|| TaskExecutionError::new("Agent task envelope is required."))?;
        let TaskSessionEnvelope::V1(envelope) = envelope;
        if envelope.kind != TaskSessionKind::Agent {
            return Err(TaskExecutionError::new(
                "AgentTaskExecutor only accepts Agent Task Sessions.",
            ));
        }

        context.report_progress(
            TaskProgress {
                phase: "resolving_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_profile_id": envelope.runtime_profile_id }),
        )?;
        let runtime_attempt_id = context.runtime_attempt_id();
        let mut resolved = self
            .resolver
            .resolve(&envelope, &runtime_attempt_id)
            .map_err(TaskExecutionError::blocked)?;
        validate_resolved_task(&envelope, &resolved).map_err(TaskExecutionError::new)?;
        let requested = envelope
            .requested_capabilities
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if requested
            .iter()
            .any(|capability| matches!(*capability, "workspace_write" | "shell" | "git"))
        {
            return Err(TaskExecutionError::new(
                "Scheduler Agent runtime does not enable unfenced workspace, shell, or Git mutations.",
            ));
        }
        if resolved.config.runtime != "opencode" {
            return Err(TaskExecutionError::new(
                "Scheduler Agent execution requires the isolated fenced OpenCode runtime.",
            ));
        }
        resolved.task.session_key = Some(runtime_attempt_id.clone());
        resolved.config.opencode_auto_approve = false;
        resolved.config.fenced_tools_only = true;
        resolved.config.isolated_opencode_process = true;
        for server in &mut resolved.config.mcp_servers {
            server.name = format!("spacesly-{}", server.secret_id);
            server.proxy_authority = Some(context.external_authority(
                &server.secret_id,
                &server.command,
                &server.environment,
            )?);
        }
        context.ensure_current()?;
        context.report_progress(
            TaskProgress {
                phase: "executing_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_attempt_id": runtime_attempt_id }),
        )?;

        let reporter = context.event_reporter();
        let callback_open = Arc::new(Mutex::new(true));
        let callback_guard = RuntimeCallbackGate {
            open: callback_open.clone(),
        };
        let callback_gate = callback_open.clone();
        let callback: AiWorkerEventCallback = Box::new(move |event| {
            let open = callback_gate.lock().map_err(|error| error.to_string())?;
            if !*open {
                return Err("Agent runtime callback is closed.".to_string());
            }
            emit_runtime_event(&reporter, event)
        });
        let result = self.runner.execute(
            resolved.config,
            resolved.task,
            context.cancellation().shared_flag(),
            callback,
        );
        drop(callback_guard);
        let result = result.map_err(TaskExecutionError::new)?;
        if result.completion_status != AiWorkerCompletionStatus::Completed {
            return Err(TaskExecutionError::blocked(
                result
                    .blocked_reason
                    .unwrap_or_else(|| "Agent runtime reported a blocked result.".to_string()),
            ));
        }
        Ok(())
    }
}

fn validate_resolved_task(
    envelope: &TaskSessionEnvelopeV1,
    resolved: &ResolvedAgentTask,
) -> Result<(), String> {
    if resolved.runtime_profile_id != envelope.runtime_profile_id {
        return Err("Resolved Agent runtime profile did not match the envelope.".to_string());
    }
    if resolved.config.workspace_id != envelope.workspace_id {
        return Err("Resolved Agent workspace did not match the envelope.".to_string());
    }
    let resolved_model = if resolved.config.runtime == "opencode" {
        resolved.config.opencode_model.as_str()
    } else {
        resolved.config.model.as_str()
    };
    if resolved_model != envelope.model {
        return Err("Resolved Agent model did not match the envelope.".to_string());
    }
    let resolved_connectors = resolved
        .config
        .mcp_servers
        .iter()
        .map(|server| server.secret_id.as_str())
        .collect::<HashSet<_>>();
    let resolved_names = resolved
        .config
        .mcp_servers
        .iter()
        .map(|server| server.name.trim())
        .collect::<HashSet<_>>();
    let requested_connectors = envelope
        .connector_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if resolved_connectors.len() != resolved.config.mcp_servers.len()
        || resolved_names.len() != resolved.config.mcp_servers.len()
        || resolved.config.mcp_servers.iter().any(|server| {
            server.name.trim().is_empty()
                || !server
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || server.secret_id.trim().is_empty()
                || server.secret_id != server.secret_id.trim()
                || server.command.is_empty()
                || server.command[0].trim().is_empty()
        })
        || resolved_connectors != requested_connectors
    {
        return Err("Resolved Agent connectors did not match the envelope.".to_string());
    }
    let contract = resolved
        .task
        .execution_contract
        .as_ref()
        .ok_or_else(|| "Resolved Agent execution contract is required.".to_string())?;
    if execution_contract_digest(contract)? != envelope.context_digest {
        return Err(
            "Resolved Agent execution contract digest did not match the envelope.".to_string(),
        );
    }
    Ok(())
}

pub fn execution_contract_digest(contract: &serde_json::Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(contract)
        .map_err(|error| format!("Failed to encode Agent execution contract: {error}"))?;
    Ok(format!(
        "sha256:{}",
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn emit_runtime_event(
    reporter: &TaskEventReporter,
    event: AiWorkerStreamEvent,
) -> Result<(), String> {
    let (kind, payload) = match event {
        AiWorkerStreamEvent::TextDelta(text) => (
            TaskSessionEventKind::Runtime,
            json!({ "type": "text_delta", "text": text }),
        ),
        AiWorkerStreamEvent::ToolStarted {
            tool_call_id,
            tool_name,
            risk,
            arguments_digest,
            display_context,
        } => (
            TaskSessionEventKind::Tool,
            json!({
                "type": "tool_started",
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "risk": risk,
                "arguments_digest": arguments_digest,
                "display_context": display_context,
            }),
        ),
        AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            risk,
            arguments_digest,
            display_context,
        } => (
            TaskSessionEventKind::Tool,
            json!({
                "type": "tool_completed",
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "success": success,
                "risk": risk,
                "arguments_digest": arguments_digest,
                "display_context": display_context,
            }),
        ),
        AiWorkerStreamEvent::UsageUpdated {
            input_tokens,
            output_tokens,
        } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "usage_updated",
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            }),
        ),
    };
    reporter
        .emit_event(kind, payload)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::execution_engine::ExecutionEngine;
    use crate::domain::task_session::{TaskSessionEnvelope, TaskSessionState};
    use crate::infrastructure::ai_worker::AiWorkerMcpServer;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tempfile::tempdir;

    struct FakeResolver {
        attempts: Arc<Mutex<Vec<String>>>,
        runtime_profile_id: String,
    }

    impl AgentRuntimeResolver for FakeResolver {
        fn resolve(
            &self,
            _envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
        ) -> Result<ResolvedAgentTask, String> {
            self.attempts
                .lock()
                .expect("attempt lock")
                .push(runtime_attempt_id.to_string());
            Ok(ResolvedAgentTask {
                runtime_profile_id: self.runtime_profile_id.clone(),
                config: test_config(),
                task: AiWorkerTask {
                    execution_contract: Some(json!({ "contract": "test" })),
                    session_key: None,
                },
            })
        }
    }

    struct FakeRunner {
        executions: Arc<AtomicUsize>,
    }

    struct BlockedRunner;

    impl AgentRuntimeRunner for BlockedRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            _on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            Ok(AiWorkerTaskResult {
                summary: "approval required".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Blocked,
                blocked_reason: Some("operator approval required".to_string()),
            })
        }
    }

    struct DetachedCallbackRunner {
        callback: Arc<Mutex<Option<AiWorkerEventCallback>>>,
        panic_after_detach: bool,
    }

    impl AgentRuntimeRunner for DetachedCallbackRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            *self.callback.lock().expect("callback lock") = Some(on_event);
            assert!(!self.panic_after_detach, "expected detached runner panic");
            Ok(AiWorkerTaskResult {
                summary: "complete".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
            })
        }
    }

    impl AgentRuntimeRunner for FakeRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            assert!(!config.opencode_auto_approve);
            assert!(config.fenced_tools_only);
            assert!(config.isolated_opencode_process);
            assert!(!cancellation.load(Ordering::Acquire));
            assert!(task
                .session_key
                .as_deref()
                .is_some_and(|value| value.contains("-attempt-")));
            let authority = config.mcp_servers[0]
                .proxy_authority
                .as_ref()
                .expect("fenced proxy authority");
            assert_eq!(authority.connector_id, "jira");
            assert_eq!(authority.capability, "external_tools:jira");
            on_event(AiWorkerStreamEvent::TextDelta("working".to_string()))?;
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(AiWorkerTaskResult {
                summary: "complete".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
            })
        }
    }

    #[test]
    fn scheduler_agent_executor_fences_connectors_and_journals_runtime_events() {
        let directory = tempdir().expect("temp directory");
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: attempts.clone(),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "real-agent-boundary",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.lock().expect("attempt lock").len(), 1);
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.kind == TaskSessionEventKind::Runtime && event.payload["type"] == "text_delta"
            }));
    }

    #[test]
    fn scheduler_agent_executor_fails_before_runner_without_connector_grant() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope("missing-grant", &test_envelope())
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task terminates");
        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("lacks capability")));
    }

    #[test]
    fn scheduler_agent_executor_preserves_blocked_terminal_outcome() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(BlockedRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(
            completed.error.as_deref(),
            Some("operator approval required")
        );
    }

    #[test]
    fn scheduler_agent_executor_rejects_resolver_profile_mismatch() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "stale-profile".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mismatched-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");
        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("profile")));
    }

    #[test]
    fn scheduler_agent_executor_closes_detached_callbacks_before_completion() {
        let directory = tempdir().expect("temp directory");
        let callback = Arc::new(Mutex::new(None));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(DetachedCallbackRunner {
                callback: callback.clone(),
                panic_after_detach: false,
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "detached-callback",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task completes");
        let event_count = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .len();
        let mut detached = callback
            .lock()
            .expect("callback lock")
            .take()
            .expect("detached callback");
        assert!(detached(AiWorkerStreamEvent::TextDelta("late".to_string())).is_err());
        assert_eq!(
            engine
                .events_after(session.id, 0)
                .expect("events replayed")
                .len(),
            event_count
        );
    }

    #[test]
    fn resolved_opencode_model_must_match_the_envelope() {
        let TaskSessionEnvelope::V1(envelope) = test_envelope();
        let mut config = test_config();
        config.opencode_model = "other/model".to_string();
        let resolved = ResolvedAgentTask {
            runtime_profile_id: envelope.runtime_profile_id.clone(),
            config,
            task: AiWorkerTask {
                execution_contract: Some(json!({ "contract": "test" })),
                session_key: None,
            },
        };
        assert!(validate_resolved_task(&envelope, &resolved)
            .expect_err("model mismatch")
            .contains("model"));
    }

    #[test]
    fn scheduler_agent_executor_closes_detached_callback_when_runner_panics() {
        let directory = tempdir().expect("temp directory");
        let callback = Arc::new(Mutex::new(None));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(DetachedCallbackRunner {
                callback: callback.clone(),
                panic_after_detach: true,
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "panic-callback",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");
        assert_eq!(completed.state, TaskSessionState::Failed);
        let mut detached = callback
            .lock()
            .expect("callback lock")
            .take()
            .expect("detached callback");
        assert!(detached(AiWorkerStreamEvent::TextDelta("late".to_string())).is_err());
    }

    fn test_config() -> AiWorkerConfig {
        AiWorkerConfig {
            workspace_id: "workspace-personal".to_string(),
            runtime: "opencode".to_string(),
            provider_name: "OpenCode".to_string(),
            provider_id: "openai".to_string(),
            base_url: String::new(),
            api_style: String::new(),
            api_key: String::new(),
            model: "gpt-5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_model: "openai/gpt-5".to_string(),
            opencode_workdir: Some("/tmp".to_string()),
            opencode_auto_approve: true,
            agent_rules: String::new(),
            agent_skills: String::new(),
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            mcp_servers: vec![AiWorkerMcpServer {
                name: "jira".to_string(),
                secret_id: "jira".to_string(),
                command: vec!["jira-mcp".to_string()],
                environment: HashMap::from([("JIRA_URL".to_string(), "test".to_string())]),
                proxy_authority: None,
            }],
        }
    }

    fn test_envelope() -> TaskSessionEnvelope {
        let contract = json!({ "contract": "test" });
        TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: Some("run-1".to_string()),
            context_digest: execution_contract_digest(&contract).expect("contract digest"),
            runtime_profile_id: "profile-1".to_string(),
            model: "openai/gpt-5".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec!["external_tools:jira".to_string()],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        })
    }
}
