use super::execution_engine::{
    TaskEventReporter, TaskExecutionContext, TaskExecutionError, TaskExecutor,
};
use crate::domain::governance::GovernanceResolutionRecord;
use crate::domain::task_session::{
    AgentTaskCompletionStatus, AgentTaskResult, TaskExecutionOutput, TaskProgress,
    TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionEventKind, TaskSessionKind,
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
    pub governance: GovernanceResolutionRecord,
}

/// Backend authority that resolves profiles, contracts, secrets, and trusted workspace paths.
pub trait AgentRuntimeResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        task_session_id: u64,
        envelope: &TaskSessionEnvelopeV1,
        runtime_attempt_id: &str,
        retained_governance: Option<&GovernanceResolutionRecord>,
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
    /// Creates an Agent executor with a trusted resolver and runtime invocation boundary.
    ///
    /// The executor is shared by scheduler Workers, while each call to `execute` receives a distinct
    /// `TaskExecutionContext` with its own cancellation token, fencing token, MCP authority, and
    /// activity timeline.
    pub fn new(
        resolver: Arc<dyn AgentRuntimeResolver>,
        runner: Arc<dyn AgentRuntimeRunner>,
    ) -> Self {
        Self { resolver, runner }
    }
}

impl TaskExecutor for AgentTaskExecutor {
    fn execute(
        &self,
        context: &TaskExecutionContext,
    ) -> Result<TaskExecutionOutput, TaskExecutionError> {
        context.ensure_current()?;
        let envelope = context
            .request()
            .envelope()
            .map_err(TaskExecutionError::new)?
            .ok_or_else(|| TaskExecutionError::new("Agent task envelope is required."))?;
        let envelope = match envelope {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => {
                return Err(TaskExecutionError::new(
                    "AgentTaskExecutor requires a V1 Agent envelope.",
                ));
            }
        };
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
        let runtime_preparation =
            crate::infrastructure::performance::span("runtime_preparation_ms", "agent_runtime")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("execution_attempt", context.attempt_id().to_string())
                .with_context("worker_id", context.worker_id().to_string())
                .with_context("runtime_id", runtime_attempt_id.clone());
        let retained_governance = context.governance_resolution()?;
        let mut resolved = self
            .resolver
            .resolve(
                context.session_id().0,
                &envelope,
                &runtime_attempt_id,
                retained_governance.as_ref(),
            )
            .map_err(TaskExecutionError::new)?;
        validate_resolved_task(&envelope, &resolved).map_err(TaskExecutionError::new)?;
        if retained_governance.is_none() {
            context.bind_governance_resolution(&resolved.governance)?;
        }
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "governance_resolved",
                "task_session_id": resolved.governance.task_session_id,
                "status": resolved.governance.status,
                "rules_revision": envelope.rules_revision,
                "skill_catalog_revision": resolved.governance.skills.catalog_revision,
                "selected_skill_ids": resolved.governance.skills.selected_skill_ids,
                "selected_skill_count": resolved.governance.skills.selected_skill_ids.len(),
                "resolved_rule_count": resolved.governance.rules.entries.len(),
                "rules_prompt_bytes": resolved.governance.rules.snapshot.len(),
                "skills_prompt_bytes": resolved.governance.skills.snapshot.len(),
                "reused": retained_governance.is_some(),
            }),
        )?;
        let opencode_resolution = crate::infrastructure::performance::span(
            "opencode_session_resolution_ms",
            "agent_runtime",
        );
        let opencode_session_id = context.opencode_session_id()?;
        let opencode_resolution = if let Some(session_id) = opencode_session_id.as_deref() {
            opencode_resolution.with_context("opencode_session_id", session_id)
        } else {
            opencode_resolution
        };
        resolved.task.opencode_session_id = opencode_session_id;
        opencode_resolution.finish();
        if resolved.config.runtime != "opencode" {
            return Err(TaskExecutionError::new(
                "Scheduler Agent execution requires the isolated fenced OpenCode runtime.",
            ));
        }
        resolved.task.session_key = Some(format!("task-session:{}", context.session_id().0));
        resolved.config.opencode_auto_approve = false;
        resolved.config.restrict_tools = false;
        resolved.config.fenced_tools_only = true;
        resolved.config.isolated_opencode_process = true;
        let workspace_root =
            resolved.config.opencode_workdir.as_deref().ok_or_else(|| {
                TaskExecutionError::new("Trusted Agent workspace root is required.")
            })?;
        resolved.config.task_tool_authority = context.task_tool_authority(
            &envelope.workspace_id,
            std::path::PathBuf::from(workspace_root),
            &envelope.requested_capabilities,
        )?;
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
        runtime_preparation.finish();

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
            if let AiWorkerStreamEvent::OpenCodeSession { session_id, .. } = &event {
                reporter
                    .bind_opencode_session(session_id)
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
            if let AiWorkerStreamEvent::ToolCompleted {
                tool_name,
                success: false,
                error: Some(error),
                arguments_digest,
                ..
            } = &event
            {
                if error.contains("[approval_required]") {
                    reporter
                        .emit_event(
                            TaskSessionEventKind::Runtime,
                            json!({
                                "type": "approval_requested",
                                "operation": tool_name,
                                "arguments_digest": arguments_digest,
                            }),
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
            emit_runtime_event(&reporter, event)
        });
        let provider_request =
            crate::infrastructure::performance::span("provider_or_runtime_request_ms", "provider")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("runtime_id", runtime_attempt_id);
        let result = self.runner.execute(
            resolved.config,
            resolved.task,
            context.cancellation().shared_flag(),
            callback,
        );
        provider_request.finish();
        drop(callback_guard);
        let result = result.map_err(|error| {
            if error.contains("[approval_required]") {
                TaskExecutionError::blocked(error)
            } else {
                TaskExecutionError::new(error)
            }
        })?;
        context.ensure_current()?;
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "agent_result_candidate",
                "authoritative": false,
                "result": result,
            }),
        )?;
        let completion_status = match result.completion_status {
            AiWorkerCompletionStatus::Completed => AgentTaskCompletionStatus::Completed,
            AiWorkerCompletionStatus::Blocked => AgentTaskCompletionStatus::Blocked,
        };
        Ok(TaskExecutionOutput::Agent(AgentTaskResult {
            summary: result.summary,
            evidence: result.evidence,
            details: result.details,
            next: result.next,
            completion_status,
            blocked_reason: result.blocked_reason,
        }))
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
    if resolved.config.agent_rules != resolved.governance.rules.snapshot
        || resolved.config.agent_skills != resolved.governance.skills.snapshot
    {
        return Err(
            "Runtime Rules or Skills do not match the authoritative governance snapshot."
                .to_string(),
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

pub(crate) fn emit_runtime_event(
    reporter: &TaskEventReporter,
    event: AiWorkerStreamEvent,
) -> Result<(), String> {
    let (kind, payload, failure) = match event {
        AiWorkerStreamEvent::OpenCodeSession { session_id, action } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "opencode_session",
                "opencode_session_id": session_id,
                "action": action,
            }),
            None,
        ),
        AiWorkerStreamEvent::TextDelta(text) => (
            TaskSessionEventKind::Runtime,
            json!({ "type": "text_delta", "text": text }),
            None,
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
            None,
        ),
        AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            error,
            risk,
            arguments_digest,
            display_context,
        } => {
            let failure = (!success).then(|| {
                format!(
                    "External tool '{tool_name}' failed. Cause: {}",
                    error
                        .as_deref()
                        .unwrap_or("The runtime did not provide an error detail.")
                )
            });
            (
                TaskSessionEventKind::Tool,
                json!({
                    "type": "tool_completed",
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "success": success,
                    "error": error,
                    "risk": risk,
                    "arguments_digest": arguments_digest,
                    "display_context": display_context,
                }),
                failure,
            )
        }
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
            None,
        ),
    };
    reporter
        .emit_event(kind, payload)
        .map_err(|error| error.to_string())?;
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::execution_engine::ExecutionEngine;
    use crate::domain::execution::{ExecutionRun, StepRun};
    use crate::domain::task_session::{TaskSessionEnvelope, TaskSessionState, TaskToolState};
    use crate::infrastructure::ai_worker::AiWorkerMcpServer;
    use crate::infrastructure::tool_broker::ToolDisplayContext;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    struct FakeResolver {
        attempts: Arc<Mutex<Vec<String>>>,
        runtime_profile_id: String,
    }

    impl AgentRuntimeResolver for FakeResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            _envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
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
                    opencode_session_id: None,
                },
                governance: retained_governance
                    .cloned()
                    .unwrap_or_else(|| test_governance(task_session_id)),
            })
        }
    }

    struct FakeRunner {
        executions: Arc<AtomicUsize>,
    }

    type IsolationObservation = Arc<Mutex<Vec<(String, u64, u64, u64)>>>;

    struct IsolationRunner {
        barrier: Arc<Barrier>,
        seen: IsolationObservation,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        origin: Instant,
        intervals: Arc<Mutex<Vec<(u64, u128, u128)>>>,
    }

    struct BlockedRunner;

    struct ApprovalThenCompleteRunner {
        executions: AtomicUsize,
        seen_session_ids: Mutex<Vec<Option<String>>>,
    }

    struct BlockedThenCompleteRunner {
        executions: AtomicUsize,
        seen_session_ids: Mutex<Vec<Option<String>>>,
    }

    struct ToolFailureRunner;

    impl AgentRuntimeRunner for ToolFailureRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-1".to_string(),
                tool_name: "jira_search".to_string(),
                success: false,
                error: Some("Connection refused while reading stdout.".to_string()),
                risk: "read".to_string(),
                arguments_digest: "digest".to_string(),
                display_context: ToolDisplayContext {
                    label: "Reading from external tool".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
            })?;
            unreachable!("failed tool callback must terminate execution")
        }
    }

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

    impl AgentRuntimeRunner for ApprovalThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .push(task.opencode_session_id.clone());
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-resume".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            if execution == 0 {
                let approval_error =
                    "[approval_required] OpenShift restart requires explicit operator approval";
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-approval".to_string(),
                    tool_name: "ocp_restart_deployment".to_string(),
                    success: false,
                    error: Some(approval_error.to_string()),
                    risk: "write".to_string(),
                    arguments_digest: "approval-digest".to_string(),
                    display_context: ToolDisplayContext {
                        label: "Restart deployment".to_string(),
                        category: "external".to_string(),
                        target: Some("deployment/clbo".to_string()),
                    },
                })?;
                return Err(approval_error.to_string());
            }
            Ok(AiWorkerTaskResult {
                summary: "continued after approval".to_string(),
                evidence: vec!["same OpenCode session".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
            })
        }
    }

    impl AgentRuntimeRunner for BlockedThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .push(task.opencode_session_id.clone());
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-continued".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            Ok(AiWorkerTaskResult {
                summary: if execution == 0 {
                    "operator input required"
                } else {
                    "continued successfully"
                }
                .to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: if execution == 0 {
                    AiWorkerCompletionStatus::Blocked
                } else {
                    AiWorkerCompletionStatus::Completed
                },
                blocked_reason: (execution == 0).then(|| "operator input required".to_string()),
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
                .is_some_and(|value| value.starts_with("task-session:")));
            assert!(task.opencode_session_id.is_none());
            let authority = config.mcp_servers[0]
                .proxy_authority
                .as_ref()
                .expect("fenced proxy authority");
            assert_eq!(authority.connector_id, "jira");
            assert_eq!(authority.capability, "external_tools:jira");
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-fake".to_string(),
                action: "created".to_string(),
            })?;
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

    impl AgentRuntimeRunner for IsolationRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let started_at = self.origin.elapsed().as_millis();
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(current, Ordering::SeqCst);
            let authority = config.mcp_servers[0]
                .proxy_authority
                .as_ref()
                .expect("fenced proxy authority");
            self.seen.lock().expect("seen lock").push((
                task.session_key.expect("session key"),
                authority.session_id.0,
                authority.attempt_id,
                authority.fencing_token,
            ));
            self.barrier.wait();
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: format!("tool-{}", authority.session_id.0),
                tool_name: "jira_search".to_string(),
                risk: "low".to_string(),
                arguments_digest: "abc".to_string(),
                display_context: ToolDisplayContext {
                    label: "jira_search".to_string(),
                    category: "external".to_string(),
                    target: Some(authority.session_id.0.to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: format!("tool-{}", authority.session_id.0),
                tool_name: "jira_search".to_string(),
                success: true,
                error: None,
                risk: "low".to_string(),
                arguments_digest: "abc".to_string(),
                display_context: ToolDisplayContext {
                    label: "jira_search".to_string(),
                    category: "external".to_string(),
                    target: Some(authority.session_id.0.to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::TextDelta(format!(
                "session:{}",
                authority.session_id.0
            )))?;
            std::thread::sleep(Duration::from_millis(20));
            let completed_at = self.origin.elapsed().as_millis();
            self.intervals.lock().expect("interval lock").push((
                authority.session_id.0,
                started_at,
                completed_at,
            ));
            self.active.fetch_sub(1, Ordering::SeqCst);
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
        assert_eq!(
            completed.opencode_session_id.as_deref(),
            Some("opencode-session-fake")
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.lock().expect("attempt lock").len(), 1);
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.kind == TaskSessionEventKind::Runtime && event.payload["type"] == "text_delta"
            }));
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.kind == TaskSessionEventKind::Runtime
                    && event.payload["type"] == "agent_result_candidate"
                    && event.payload["authoritative"] == false
                    && event.payload["result"]["completion_status"] == "completed"
            }));
    }

    #[test]
    fn five_agent_sessions_execute_simultaneously_with_isolated_runtime_and_tool_authority() {
        let directory = tempdir().expect("temp directory");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let intervals = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(IsolationRunner {
                barrier: Arc::new(Barrier::new(5)),
                seen: seen.clone(),
                active: active.clone(),
                maximum: maximum.clone(),
                origin: Instant::now(),
                intervals: intervals.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let sessions = (1..=5)
            .map(|index| {
                let mut envelope = test_envelope();
                let TaskSessionEnvelope::V1(session) = &mut envelope else {
                    unreachable!();
                };
                session.subject_id = Some(format!("card-{index}"));
                session.conversation_id = Some(format!("conversation-{index}"));
                session.execution_run_id = Some(format!("run-{index}"));
                engine
                    .submit_envelope_with_grants(
                        format!("isolated-agent-{index}"),
                        &envelope,
                        vec!["external_tools:jira".to_string()],
                        "test-approval",
                    )
                    .expect("task submitted")
            })
            .collect::<Vec<_>>();

        for session in &sessions {
            engine
                .wait_for_terminal(session.id, Duration::from_secs(5))
                .expect("task completes");
        }
        let seen = seen.lock().expect("seen lock").clone();
        assert_eq!(seen.len(), 5);
        assert_eq!(
            seen.iter()
                .map(|entry| entry.0.as_str())
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        assert_eq!(
            seen.iter()
                .map(|entry| entry.1)
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        assert_eq!(maximum.load(Ordering::SeqCst), 5);
        let intervals = intervals.lock().expect("interval lock").clone();
        assert_eq!(intervals.len(), 5);
        let latest_start = intervals.iter().map(|entry| entry.1).max().unwrap();
        let earliest_finish = intervals.iter().map(|entry| entry.2).min().unwrap();
        assert!(
            latest_start < earliest_finish,
            "all Agent runtime intervals must overlap"
        );
        assert_eq!(
            sessions
                .iter()
                .map(|session| {
                    engine
                        .session(session.id)
                        .expect("session read")
                        .expect("session exists")
                        .worker_id
                        .expect("worker assigned")
                })
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        for session in sessions {
            let events = engine.events_after(session.id, 0).expect("events");
            assert!(events.iter().all(|event| event.session_id == session.id));
            let tools = TaskToolState::from_events(session.id, &events);
            assert_eq!(tools.calls.len(), 1);
            assert!(events.iter().any(|event| {
                event.kind == TaskSessionEventKind::Runtime && event.payload["type"] == "text_delta"
            }));
        }
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
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.payload["type"] == "agent_result_candidate"
                    && event.payload["result"]["completion_status"] == "blocked"
                    && event.payload["result"]["blocked_reason"] == "operator approval required"
            }));
    }

    #[test]
    fn approval_continuation_resumes_the_same_opencode_session_end_to_end() {
        let directory = tempdir().expect("temp directory");
        let runner = Arc::new(ApprovalThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "approval-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let paused = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task pauses");
        assert_eq!(paused.state, TaskSessionState::Blocked);
        assert_eq!(
            paused.opencode_session_id.as_deref(),
            Some("opencode-session-resume")
        );

        let resumed = engine
            .resume_after_approval(
                session.id,
                "approval-agent-resumed",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("same task resumed");
        assert_eq!(resumed.id, session.id);
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("resumed task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(
            runner
                .seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .as_slice(),
            &[None, Some("opencode-session-resume".to_string())]
        );
        let created_count = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .filter(|event| event.payload["action"] == json!("created"))
            .count();
        assert_eq!(created_count, 1);
        let governance_events = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .into_iter()
            .filter(|event| event.payload["type"] == "governance_resolved")
            .collect::<Vec<_>>();
        assert_eq!(governance_events.len(), 2);
        assert_eq!(governance_events[0].payload["reused"], false);
        assert_eq!(governance_events[1].payload["reused"], true);
        assert_eq!(
            governance_events[0].payload["selected_skill_ids"],
            governance_events[1].payload["selected_skill_ids"]
        );
    }

    #[test]
    fn generic_continuation_resumes_the_same_opencode_session_end_to_end() {
        let directory = tempdir().expect("temp directory");
        let runner = Arc::new(BlockedThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let continued = engine
            .continue_interrupted_session(
                session.id,
                "continued-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        assert_eq!(continued.id, session.id);
        assert_eq!(
            continued.opencode_session_id.as_deref(),
            Some("opencode-session-continued")
        );
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(
            runner
                .seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .as_slice(),
            &[None, Some("opencode-session-continued".to_string())]
        );
    }

    #[test]
    fn blocked_continuation_projects_onto_reused_execution_run() {
        let directory = tempdir().expect("temp directory");
        let executions = crate::infrastructure::execution_store::ExecutionStore::open_at(
            directory.path().join("executions.db"),
        )
        .expect("execution store opens");
        let workspace_id = "workspace-personal";
        let conversation_id = "conversation-1";
        let run_id = "run-1";
        executions
            .save(&ExecutionRun {
                run_id: run_id.to_string(),
                contract: serde_json::json!({
                    "contract_id": format!("contract-{run_id}"),
                    "task_id": "task-1",
                    "workspace_id": workspace_id,
                    "version": 1,
                    "created_at": "2026-07-31T00:00:00Z"
                }),
                status: "running".to_string(),
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
            })
            .expect("execution run saved");
        executions
            .append_conversation_message(
                workspace_id,
                conversation_id,
                "Agent card",
                &crate::infrastructure::execution_store::ConversationMessageInput {
                    id: "agent-context".to_string(),
                    role: "user".to_string(),
                    text: "Execute the contract".to_string(),
                },
            )
            .expect("conversation seeded");

        let runner = Arc::new(BlockedThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor_and_projector(
            Arc::new(executor),
            Arc::new(executions.clone()),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");

        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let continued = engine
            .continue_interrupted_session(
                session.id,
                "continued-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        assert_eq!(continued.id, session.id);
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        let step = executions
            .get(run_id)
            .expect("run loaded")
            .expect("run exists")
            .step_runs
            .remove("worker.execute")
            .expect("worker.execute step projected");
        assert_eq!(step.status, "completed");
    }

    #[test]
    fn scheduler_agent_executor_terminalizes_failed_tool_with_original_cause() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(ToolFailureRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "failed-tool-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");

        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(
            completed.error.as_deref(),
            Some(
                "External tool 'jira_search' failed. Cause: Connection refused while reading stdout."
            )
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let terminal = events
            .iter()
            .find(|event| {
                event.kind == TaskSessionEventKind::Lifecycle && event.payload["state"] == "failed"
            })
            .expect("terminal lifecycle event");
        assert_eq!(completed.attempt_id, terminal.attempt_id);
        assert_eq!(completed.fencing_token, terminal.fencing_token);
        assert_eq!(completed.last_event_sequence, terminal.sequence);
        assert_eq!(
            completed.progress,
            Some(TaskProgress {
                phase: "failed".to_string(),
                completed: 1,
                total: Some(1),
            })
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[1].sequence == pair[0].sequence + 1));
        assert!(engine
            .task_session_result(session.id)
            .expect("authoritative result query")
            .is_none());
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Tool
                && event.payload["type"] == "tool_completed"
                && event.payload["success"] == false
                && event.payload["error"] == "Connection refused while reading stdout."
        }));
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
        let envelope = match test_envelope() {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => panic!("expected V1 Agent envelope"),
        };
        let mut config = test_config();
        config.opencode_model = "other/model".to_string();
        let resolved = ResolvedAgentTask {
            runtime_profile_id: envelope.runtime_profile_id.clone(),
            config,
            task: AiWorkerTask {
                execution_contract: Some(json!({ "contract": "test" })),
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
        };
        assert!(validate_resolved_task(&envelope, &resolved)
            .expect_err("model mismatch")
            .contains("model"));
    }

    #[test]
    fn runtime_governance_must_match_the_persisted_resolution() {
        let envelope = match test_envelope() {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => panic!("expected V1 Agent envelope"),
        };
        let mut config = test_config();
        config.agent_skills = "unpersisted skill instructions".to_string();
        let resolved = ResolvedAgentTask {
            runtime_profile_id: envelope.runtime_profile_id.clone(),
            config,
            task: AiWorkerTask {
                execution_contract: Some(json!({ "contract": "test" })),
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
        };
        assert!(validate_resolved_task(&envelope, &resolved)
            .expect_err("governance mismatch")
            .contains("authoritative governance snapshot"));
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
            governance_schema_version: 0,
            skill_catalog: Vec::new(),
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: vec![AiWorkerMcpServer {
                name: "jira".to_string(),
                secret_id: "jira".to_string(),
                command: vec!["jira-mcp".to_string()],
                environment: HashMap::from([("JIRA_URL".to_string(), "test".to_string())]),
                proxy_authority: None,
            }],
        }
    }

    fn test_governance(task_session_id: u64) -> GovernanceResolutionRecord {
        use crate::domain::governance::{
            GovernanceResolutionStatus, RulesResolutionRecord, SkillResolutionRecord,
            GOVERNANCE_RESOLUTION_VERSION,
        };
        GovernanceResolutionRecord {
            schema_version: GOVERNANCE_RESOLUTION_VERSION,
            task_session_id,
            resolved_at: 1,
            status: GovernanceResolutionStatus::LegacyUnavailable,
            rules: RulesResolutionRecord {
                normalization_version: "legacy_unavailable".to_string(),
                final_digest: crate::infrastructure::runtime_profile_store::content_revision(""),
                entries: Vec::new(),
                snapshot: String::new(),
            },
            skills: SkillResolutionRecord {
                catalog_revision: None,
                selected_skill_ids: Vec::new(),
                entries: Vec::new(),
                snapshot: String::new(),
            },
        }
    }

    fn test_envelope() -> TaskSessionEnvelope {
        let contract = json!({ "contract": "test" });
        TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: Some("card-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
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
