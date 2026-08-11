use super::execution_engine::{
    TaskEventReporter, TaskExecutionContext, TaskExecutionError, TaskExecutor,
};
use crate::domain::execution_manifest::{ExecutionManifestDraft, ExecutionModelConfiguration};
use crate::domain::governance::{GovernanceResolutionRecord, RepositoryRuleFact, RuleFactsRecord};
use crate::domain::task_examination::{examine_task, RepositoryResolutionRecord};
use crate::domain::task_recovery::{
    decide_capability_repair, decide_runtime_recovery, RuntimeFailureClass, RuntimeRecoveryAction,
    RuntimeRecoveryContext,
};
use crate::domain::task_session::{
    AgentTaskCompletionStatus, AgentTaskObjectiveResult, AgentTaskObjectiveToolReceipt,
    AgentTaskResult, TaskExecutionOutput, TaskMcpConnectorContext, TaskProgress,
    TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionEventKind, TaskSessionKind,
};
use crate::infrastructure::ai_worker::{
    execute_ai_worker_task, AiWorkerCompletionStatus, AiWorkerConfig, AiWorkerEventCallback,
    AiWorkerStreamEvent, AiWorkerTask, AiWorkerTaskResult,
};
use crate::infrastructure::git::repository_root_at;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const MAX_AUTOMATIC_RUNTIME_RETRIES: u8 = 1;
const MAX_REPOSITORY_DISCOVERY_DEPTH: usize = 4;
const MAX_REPOSITORY_DISCOVERY_DIRECTORIES: usize = 4_096;

struct RepositoryPreflight {
    record: Option<RepositoryResolutionRecord>,
    repository_root: Option<PathBuf>,
    blocker: Option<String>,
}

fn resolve_repository_preflight(
    contract: &Value,
    rule_facts: &RuleFactsRecord,
    workspace_root: &Path,
) -> RepositoryPreflight {
    if rule_facts.repositories.is_empty() {
        return RepositoryPreflight {
            record: None,
            repository_root: None,
            blocker: None,
        };
    }
    let contract_text = serde_json::to_string(contract)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let matching = rule_facts
        .repositories
        .iter()
        .filter(|repository| {
            contract_text.contains(&repository.id.to_ascii_lowercase())
                || contract_text.contains(&repository.remote_url.to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    let selected = if matching.len() == 1 {
        Some(matching[0])
    } else if matching.is_empty() && rule_facts.repositories.len() == 1 {
        rule_facts.repositories.first()
    } else {
        None
    };
    let Some(repository) = selected else {
        let reason = if matching.len() > 1 {
            "The task matches multiple repository Rules; declare one repository root in the execution contract."
        } else {
            "Multiple repository Rules are available, but the task does not identify which repository to use."
        };
        return repository_preflight_failure("ambiguous", None, reason);
    };
    let contract_path = contract
        .pointer("/repository/root_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let (Some(contract_path), Some(rule_path)) =
        (contract_path, repository.local_path.as_deref())
    {
        let contract_candidate = absolute_repository_candidate(workspace_root, contract_path);
        let rule_candidate = absolute_repository_candidate(workspace_root, rule_path);
        let resolved_contract = contract_candidate
            .canonicalize()
            .unwrap_or(contract_candidate);
        let resolved_rule = rule_candidate.canonicalize().unwrap_or(rule_candidate);
        if resolved_contract != resolved_rule {
            return repository_preflight_failure(
                "conflict",
                Some(repository),
                "The execution contract repository root conflicts with the authoritative Rules checkout path.",
            );
        }
    }
    let candidate = contract_path
        .or(repository.local_path.as_deref())
        .map(|path| absolute_repository_candidate(workspace_root, path));
    let candidate = match candidate {
        Some(candidate) => candidate,
        None => match discover_named_repositories(workspace_root, &repository.id) {
            Ok((matches, false)) if matches.len() == 1 => matches[0].clone(),
            Ok((matches, _)) if matches.len() > 1 => {
                return repository_preflight_failure(
                    "ambiguous",
                    Some(repository),
                    "More than one contained Git checkout matches the repository Rule; declare its exact local checkout path.",
                );
            }
            Ok((_, true)) => {
                return repository_preflight_failure(
                    "missing_checkout",
                    Some(repository),
                    "Bounded repository discovery was exhausted; declare the exact local checkout path in Rules.",
                );
            }
            Ok(_) => {
                return repository_preflight_failure(
                    "missing_checkout",
                    Some(repository),
                    "The repository Rule has no local checkout path and no matching contained Git checkout was discovered. Add `Local checkout: /absolute/path` to Rules.",
                );
            }
            Err(error) => {
                return repository_preflight_failure(
                    "invalid_checkout",
                    Some(repository),
                    &format!("Repository discovery failed: {error}"),
                );
            }
        },
    };
    let canonical_workspace = match workspace_root.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The trusted workspace root does not exist or cannot be resolved.",
            );
        }
    };
    let canonical = match candidate.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return repository_preflight_failure(
                "missing_checkout",
                Some(repository),
                "The configured repository checkout does not exist or cannot be resolved.",
            );
        }
    };
    if !canonical.starts_with(&canonical_workspace) {
        return repository_preflight_failure(
            "outside_workspace",
            Some(repository),
            "The configured repository checkout escapes the trusted workspace root.",
        );
    }
    match repository_root_at(&canonical) {
        Ok(Some(root)) if root == canonical => {}
        Ok(Some(_)) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The configured checkout path is inside a repository but is not its root.",
            );
        }
        Ok(None) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The configured checkout path is not a Git repository root.",
            );
        }
        Err(error) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                &format!("Git could not validate the configured checkout: {error}"),
            );
        }
    }
    RepositoryPreflight {
        record: Some(repository_resolution_record(
            "resolved",
            Some(repository),
            Some(&canonical),
            "A unique contained Git checkout was resolved from the immutable contract and Rules.",
        )),
        repository_root: Some(canonical),
        blocker: None,
    }
}

fn repository_preflight_failure(
    status: &str,
    repository: Option<&RepositoryRuleFact>,
    reason: &str,
) -> RepositoryPreflight {
    RepositoryPreflight {
        record: Some(repository_resolution_record(
            status, repository, None, reason,
        )),
        repository_root: None,
        blocker: Some(reason.to_string()),
    }
}

fn repository_resolution_record(
    status: &str,
    repository: Option<&RepositoryRuleFact>,
    local_path: Option<&Path>,
    reason: &str,
) -> RepositoryResolutionRecord {
    RepositoryResolutionRecord {
        schema_version: 1,
        status: status.to_string(),
        repository_id: repository.map(|value| value.id.clone()),
        remote_url: repository.map(|value| sanitize_repository_url(&value.remote_url)),
        local_path: local_path.map(|path| path.to_string_lossy().to_string()),
        backend_path: repository.and_then(|value| value.backend_path.clone()),
        frontend_path: repository.and_then(|value| value.frontend_path.clone()),
        source: repository
            .map(|value| value.source.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "global.agent_rules".to_string()),
        source_line: repository.map(|value| value.source_line).unwrap_or(0),
        reason: reason.to_string(),
    }
}

fn sanitize_repository_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "invalid-repository-url".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn absolute_repository_candidate(workspace_root: &Path, value: &str) -> PathBuf {
    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        candidate
    } else {
        workspace_root.join(candidate)
    }
}

fn discover_named_repositories(
    workspace_root: &Path,
    repository_id: &str,
) -> Result<(Vec<PathBuf>, bool), String> {
    let mut pending = vec![(workspace_root.to_path_buf(), 0usize)];
    let mut matches = Vec::new();
    let mut inspected = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        inspected = inspected.saturating_add(1);
        if inspected > MAX_REPOSITORY_DISCOVERY_DIRECTORIES {
            return Ok((matches, true));
        }
        if directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(repository_id))
            && repository_root_at(&directory)?.as_deref() == Some(directory.as_path())
        {
            matches.push(directory);
            continue;
        }
        if depth >= MAX_REPOSITORY_DISCOVERY_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if depth == 0 => {
                return Err(format!("Could not read '{}': {error}", directory.display()));
            }
            Err(_) => continue,
        };
        let mut children = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                let name = entry.file_name();
                let name = name.to_str()?;
                (file_type.is_dir()
                    && !file_type.is_symlink()
                    && !matches!(name, ".git" | "node_modules" | "target" | ".cache"))
                .then(|| entry.path())
            })
            .collect::<Vec<_>>();
        children.sort();
        pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
    }
    matches.sort();
    matches.dedup();
    Ok((matches, false))
}

#[derive(Clone, Debug, Default)]
struct RuntimeAttemptObservation {
    successful_tool_calls: u32,
    successful_mutation_observed: bool,
    in_flight_tools: HashMap<String, AgentTaskObjectiveToolReceipt>,
    uncheckpointed_tool_receipts: Vec<AgentTaskObjectiveToolReceipt>,
    opencode_session_id: Option<String>,
    failed_tool: Option<String>,
    failed_tool_risk: Option<String>,
}

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

fn runtime_callback(
    reporter: TaskEventReporter,
    callback_gate: Arc<Mutex<bool>>,
    observation: Arc<Mutex<RuntimeAttemptObservation>>,
    objective_mutations: Arc<HashMap<String, bool>>,
    protected_mutations: Arc<Mutex<HashMap<(String, String), String>>>,
) -> AiWorkerEventCallback {
    Box::new(move |event| {
        let open = callback_gate.lock().map_err(|error| error.to_string())?;
        if !*open {
            return Err("Agent runtime callback is closed.".to_string());
        }
        if let AiWorkerStreamEvent::OpenCodeSession { session_id, .. } = &event {
            reporter
                .bind_opencode_session(session_id)
                .map_err(|error| error.to_string())?;
            observation
                .lock()
                .map_err(|error| error.to_string())?
                .opencode_session_id = Some(session_id.clone());
            return Ok(());
        }
        if let AiWorkerStreamEvent::ObjectiveCheckpoint {
            objective_id,
            evidence,
        } = &event
        {
            let mutation_expected = objective_mutations.get(objective_id).ok_or_else(|| {
                format!("Agent checkpoint referenced unknown objective '{objective_id}'.")
            })?;
            let tool_receipts = {
                let mut observation = observation.lock().map_err(|error| error.to_string())?;
                std::mem::take(&mut observation.uncheckpointed_tool_receipts)
            };
            if *mutation_expected
                && !tool_receipts
                    .iter()
                    .any(|receipt| !matches!(receipt.risk.as_str(), "read"))
            {
                return Err(format!(
                    "Mutation objective '{objective_id}' cannot checkpoint before a successful mutation tool event."
                ));
            }
            let checkpoint_event = reporter
                .record_objective_checkpoint(objective_id, evidence, &tool_receipts)
                .map_err(|error| error.to_string())?;
            if checkpoint_event.payload["new_checkpoint"] == true {
                let mut protected = protected_mutations
                    .lock()
                    .map_err(|error| error.to_string())?;
                for receipt in tool_receipts
                    .iter()
                    .filter(|receipt| receipt.risk != "read")
                {
                    protected.insert(
                        (receipt.tool_name.clone(), receipt.arguments_digest.clone()),
                        objective_id.clone(),
                    );
                }
            }
            return Ok(());
        }
        if let AiWorkerStreamEvent::ToolStarted {
            tool_call_id,
            tool_name,
            risk,
            arguments_digest,
            ..
        } = &event
        {
            let normalized_tool_name = tool_name.trim().to_ascii_lowercase();
            if let Some(objective_id) = protected_mutations
                .lock()
                .map_err(|error| error.to_string())?
                .get(&(normalized_tool_name.clone(), arguments_digest.clone()))
                .cloned()
            {
                return Err(format!(
                    "Tool call '{tool_name}' replays completed objective '{objective_id}' with identical arguments."
                ));
            }
            observation
                .lock()
                .map_err(|error| error.to_string())?
                .in_flight_tools
                .insert(
                    tool_call_id.clone(),
                    AgentTaskObjectiveToolReceipt {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: normalized_tool_name,
                        risk: match risk.trim().to_ascii_lowercase().as_str() {
                            "low" => "read".to_string(),
                            normalized => normalized.to_string(),
                        },
                        arguments_digest: arguments_digest.clone(),
                        resource_operation_key: None,
                    },
                );
        }
        if let AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            error,
            risk,
            arguments_digest,
            arguments_observed,
            resource_operation_key,
            ..
        } = &event
        {
            if *success {
                let mut observation = observation.lock().map_err(|error| error.to_string())?;
                let normalized_tool_name = tool_name.trim().to_ascii_lowercase();
                let mut receipt = match observation.in_flight_tools.remove(tool_call_id) {
                    Some(receipt)
                        if receipt.tool_name != normalized_tool_name
                            || (*arguments_observed
                                && receipt.arguments_digest != *arguments_digest) =>
                    {
                        return Err(format!(
                            "Tool completion '{tool_call_id}' did not match its started tool identity."
                        ));
                    }
                    Some(receipt) => receipt,
                    None => AgentTaskObjectiveToolReceipt {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: normalized_tool_name,
                        risk: match risk.trim().to_ascii_lowercase().as_str() {
                            "low" => "read".to_string(),
                            normalized => normalized.to_string(),
                        },
                        arguments_digest: arguments_digest.clone(),
                        resource_operation_key: None,
                    },
                };
                receipt.resource_operation_key = resource_operation_key.clone();
                observation.successful_tool_calls =
                    observation.successful_tool_calls.saturating_add(1);
                if !matches!(risk.trim().to_ascii_lowercase().as_str(), "read" | "low") {
                    observation.successful_mutation_observed = true;
                }
                observation.uncheckpointed_tool_receipts.push(receipt);
            } else {
                let mut attempt = observation.lock().map_err(|error| error.to_string())?;
                attempt.in_flight_tools.remove(tool_call_id);
                attempt.failed_tool = Some(tool_name.clone());
                attempt.failed_tool_risk = Some(risk.clone());
                drop(attempt);
                if error
                    .as_deref()
                    .is_some_and(|error| error.contains("[approval_required]"))
                {
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
        }
        emit_runtime_event(&reporter, event)
    })
}

/// Trusted configuration and contract reconstructed from one durable Task Session envelope.
pub struct ResolvedAgentTask {
    pub runtime_profile_id: String,
    pub config: AiWorkerConfig,
    pub task: AiWorkerTask,
    pub governance: GovernanceResolutionRecord,
    pub connector_capabilities: Vec<crate::domain::task_examination::ConnectorCapabilitySnapshot>,
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
        resolved.task.opencode_session_id = opencode_session_id.clone();
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
        let granted_capabilities = context
            .capability_grants()
            .iter()
            .map(|grant| grant.capability.as_str())
            .collect::<HashSet<_>>();
        let connectors = envelope
            .connector_ids
            .iter()
            .map(|connector_id| {
                let capability = format!("external_tools:{connector_id}");
                TaskMcpConnectorContext {
                    connector_id: connector_id.clone(),
                    requested: true,
                    granted: granted_capabilities.contains(capability.as_str()),
                    capability,
                }
            })
            .collect::<Vec<_>>();
        let mut examination = examine_task(
            resolved
                .task
                .execution_contract
                .as_ref()
                .expect("validated execution contract"),
            &envelope.context_digest,
            &envelope,
            &granted_capabilities,
            &resolved.governance.rules.facts,
            &resolved.connector_capabilities,
        );
        let objective_checkpoints = context.objective_checkpoints()?;
        let protected_mutations = Arc::new(Mutex::new(
            objective_checkpoints
                .iter()
                .flat_map(|checkpoint| {
                    checkpoint
                        .tool_receipts
                        .iter()
                        .filter(|receipt| receipt.risk != "read")
                        .map(|receipt| {
                            (
                                (receipt.tool_name.clone(), receipt.arguments_digest.clone()),
                                checkpoint.objective_id.clone(),
                            )
                        })
                })
                .collect::<HashMap<_, _>>(),
        ));
        let semantic_objective_mutations = resolved
            .task
            .execution_contract
            .as_ref()
            .and_then(|contract| contract.get("semantic_plan"))
            .and_then(|plan| plan.get("objectives"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|objective| {
                Some((
                    objective.get("id")?.as_str()?.to_string(),
                    objective
                        .get("mutation_expected")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                ))
            })
            .collect::<HashMap<_, _>>();
        if objective_checkpoints
            .iter()
            .any(|checkpoint| !semantic_objective_mutations.contains_key(&checkpoint.objective_id))
        {
            return Err(TaskExecutionError::new(
                "Retained objective checkpoint does not belong to the immutable semantic plan.",
            ));
        }
        examination.objective_checkpoints = objective_checkpoints;
        let (workspace_git_preflight, default_repository_root, repository_preflight_blocker) =
            if envelope
                .requested_capabilities
                .iter()
                .any(|capability| capability == "git")
            {
                resolved.config.opencode_workdir.as_deref().map_or(
                (None, None, None),
                |workdir| {
                let configured = std::path::PathBuf::from(workdir);
                let canonical = configured.canonicalize().ok();
                let repository_preflight = resolve_repository_preflight(
                    resolved
                        .task
                        .execution_contract
                        .as_ref()
                        .expect("validated execution contract"),
                    &resolved.governance.rules.facts,
                    &configured,
                );
                examination.repository_resolution = repository_preflight.record.clone();
                if let Some(record) = repository_preflight.record.as_ref() {
                    return (
                        Some(json!({
                            "type": "workspace_git_preflight",
                            "schema_version": 2,
                            "status": record.status,
                            "workspace_root": canonical,
                            "repository_root": repository_preflight.repository_root,
                            "repository_resolution": record,
                            "guidance": repository_preflight.blocker,
                        })),
                        repository_preflight.repository_root,
                        repository_preflight.blocker,
                    );
                }
                let (status, repository_root, guidance) = match canonical.as_deref() {
                    None => (
                        "invalid_workspace",
                        None,
                        "Configure an existing trusted workspace directory before using Git.",
                    ),
                    Some(root) => match repository_root_at(root) {
                        Ok(Some(repository_root)) if repository_root == root => {
                            ("workspace_repository", Some(repository_root), "")
                        }
                        Ok(Some(repository_root)) => (
                            "repository_outside_scope",
                            Some(repository_root),
                            "Configure the repository root or one of its parent directories as the trusted workspace.",
                        ),
                        Ok(None) => (
                            "nested_repository_required",
                            None,
                            "The workspace root is not a repository. Pass the repository directory using the Git tool 'workdir' argument.",
                        ),
                        Err(_) => (
                            "git_unavailable",
                            None,
                            "Install Git or expose it through PATH or a standard package-manager profile.",
                        ),
                    },
                };
                if !guidance.is_empty()
                    && examination.warnings.len() < 64
                    && !examination.warnings.iter().any(|warning| warning == guidance)
                {
                    examination.warnings.push(guidance.to_string());
                }
                (
                    Some(json!({
                        "type": "workspace_git_preflight",
                        "schema_version": 1,
                        "status": status,
                        "workspace_root": canonical,
                        "repository_root": repository_root,
                        "guidance": guidance,
                    })),
                    None,
                    None,
                )
            })
            } else {
                (None, None, None)
            };
        let semantic_objective_mutations = Arc::new(semantic_objective_mutations);
        examination
            .validate(&envelope.context_digest)
            .map_err(TaskExecutionError::new)?;
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "task_examined",
                "status": examination.status,
                "objective_count": examination.objectives.len(),
                "resource_count": examination.resources.len(),
                "required_capability_count": examination.required_capabilities.len(),
                "unresolved_requirement_count": examination.unresolved_requirements.len(),
                "approval_boundary_count": examination.approval_boundaries.len(),
                "objective_checkpoint_count": examination.objective_checkpoints.len(),
                "live_connector_count": examination.connector_capabilities.iter()
                    .filter(|connector| connector.status == crate::domain::task_examination::ConnectorDiscoveryStatus::Available)
                    .count(),
                "discovered_tool_count": examination.connector_capabilities.iter()
                    .map(|connector| connector.tools.len())
                    .sum::<usize>(),
            }),
        )?;
        if let Some(preflight) = workspace_git_preflight {
            context.emit_event(TaskSessionEventKind::Runtime, preflight)?;
        }
        let mut connector_preflight_blockers = examination
            .connector_capabilities
            .iter()
            .filter(|connector| {
                connector.status
                    == crate::domain::task_examination::ConnectorDiscoveryStatus::Unavailable
            })
            .map(|connector| {
                format!(
                    "{}: {}",
                    connector.connector_id,
                    connector
                        .error
                        .as_deref()
                        .unwrap_or("MCP tools are unavailable.")
                )
            })
            .collect::<Vec<_>>();
        connector_preflight_blockers.extend(
            examination
                .unresolved_requirements
                .iter()
                .filter(|requirement| !requirement.starts_with("Capability '"))
                .cloned(),
        );
        connector_preflight_blockers.sort();
        connector_preflight_blockers.dedup();
        resolved.task.task_examination = Some(examination.clone());
        context.bind_execution_manifest(&ExecutionManifestDraft {
            kind: envelope.kind,
            workspace_id: envelope.workspace_id.clone(),
            subject_id: envelope.subject_id.clone(),
            conversation_id: envelope.conversation_id.clone(),
            execution_run_id: envelope.execution_run_id.clone(),
            context_digest: envelope.context_digest.clone(),
            context_revision: envelope.context_revision.clone(),
            runtime: resolved.config.runtime.clone(),
            runtime_profile_id: resolved.runtime_profile_id.clone(),
            runtime_id: runtime_attempt_id.clone(),
            model: resolved.config.model.clone(),
            model_configuration: ExecutionModelConfiguration {
                provider_id: resolved.config.provider_id.clone(),
                api_style: resolved.config.api_style.clone(),
                temperature: resolved.config.temperature.to_string(),
            },
            prompt_template_version: envelope.prompt_template_version.clone(),
            rules_revision: envelope.rules_revision.clone(),
            skills_revision: envelope.skills_revision.clone(),
            rules: resolved.governance.rules.entries.clone(),
            rules_digest: resolved.governance.rules.final_digest.clone(),
            rule_facts: resolved.governance.rules.facts.clone(),
            task_examination: examination,
            skills_catalog_revision: resolved.governance.skills.catalog_revision.clone(),
            skills: resolved.governance.skills.entries.clone(),
            connectors,
            tool_permission_mode: "fenced_tools_only".to_string(),
            unknown_fields: vec![
                "git_revision".to_string(),
                "environment_fingerprint".to_string(),
                "mcp_implementation_versions".to_string(),
                "mcp_connection_ids".to_string(),
            ],
        })?;
        if let Some(blocker) = repository_preflight_blocker {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Repository preflight blocked execution. {blocker}"
            )));
        }
        if !connector_preflight_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "MCP capability preflight blocked execution. {}",
                connector_preflight_blockers.join(" ")
            )));
        }
        let workspace_root =
            resolved.config.opencode_workdir.as_deref().ok_or_else(|| {
                TaskExecutionError::new("Trusted Agent workspace root is required.")
            })?;
        let mut task_tool_authority = context.task_tool_authority(
            &envelope.workspace_id,
            std::path::PathBuf::from(workspace_root),
            &envelope.requested_capabilities,
        )?;
        if let Some(authority) = task_tool_authority.as_mut() {
            authority.default_repository_root = default_repository_root;
        }
        resolved.config.task_tool_authority = task_tool_authority;
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
        let runtime_preparation_duration = runtime_preparation.finish();
        emit_execution_trace_stage(
            context,
            "runtime_preparation",
            runtime_preparation_duration,
            "succeeded",
            &runtime_attempt_id,
            opencode_session_id.as_deref(),
        );

        let provider_request =
            crate::infrastructure::performance::span("provider_or_runtime_request_ms", "provider")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("runtime_id", runtime_attempt_id.clone());
        let mut retries_attempted = 0_u8;
        let mut capability_repairs_attempted = 0_u8;
        let mut retry_task = resolved.task;
        let mut retry_config = resolved.config;
        let mut terminal_recovery_action = None;
        let result = loop {
            context.ensure_current()?;
            let observation = Arc::new(Mutex::new(RuntimeAttemptObservation::default()));
            let callback_open = Arc::new(Mutex::new(true));
            let callback_guard = RuntimeCallbackGate {
                open: callback_open.clone(),
            };
            let callback = runtime_callback(
                context.event_reporter(),
                callback_open,
                observation.clone(),
                semantic_objective_mutations.clone(),
                protected_mutations.clone(),
            );
            let attempt_result = self.runner.execute(
                retry_config.clone(),
                retry_task.clone(),
                context.cancellation().shared_flag(),
                callback,
            );
            drop(callback_guard);

            let recovery_input = match &attempt_result {
                Err(error) => Some(error.as_str()),
                Ok(result) if result.completion_status == AiWorkerCompletionStatus::Blocked => {
                    Some(result.blocked_reason.as_deref().unwrap_or(&result.summary))
                }
                Ok(_) => None,
            };
            let Some(recovery_input) = recovery_input else {
                break attempt_result;
            };
            let observation = observation
                .lock()
                .map_err(|error| TaskExecutionError::new(error.to_string()))?
                .clone();
            let decision = decide_runtime_recovery(
                recovery_input,
                RuntimeRecoveryContext {
                    retries_attempted,
                    max_automatic_retries: MAX_AUTOMATIC_RUNTIME_RETRIES,
                    successful_mutation_observed: observation.successful_mutation_observed,
                    cancellation_requested: context.cancellation().is_cancelled(),
                },
            );
            if decision.failure_class == RuntimeFailureClass::MissingCapability {
                let repair = retry_task.task_examination.as_ref().map(|examination| {
                    decide_capability_repair(
                        examination,
                        recovery_input,
                        observation.failed_tool.as_deref().unwrap_or_default(),
                        observation.failed_tool_risk.as_deref().unwrap_or("unknown"),
                        capability_repairs_attempted,
                        observation.successful_mutation_observed,
                    )
                });
                if let Some(repair) = repair {
                    context.emit_event(
                        TaskSessionEventKind::Runtime,
                        json!({
                            "type": "capability_repair_decision",
                            "schema_version": repair.schema_version,
                            "repairable": repair.repairable,
                            "reason": repair.reason,
                            "failed_tool": observation.failed_tool.as_deref(),
                            "connector_id": repair.guidance.as_ref().map(|guidance| guidance.connector_id.as_str()),
                            "allowed_alternatives": repair.guidance.as_ref().map(|guidance| guidance.allowed_alternatives.as_slice()).unwrap_or_default(),
                            "repairs_attempted": capability_repairs_attempted,
                        }),
                    )?;
                    if repair.repairable {
                        capability_repairs_attempted =
                            capability_repairs_attempted.saturating_add(1);
                        if let Some(examination) = retry_task.task_examination.as_mut() {
                            examination.runtime_repair = repair.guidance;
                            examination
                                .validate(&envelope.context_digest)
                                .map_err(TaskExecutionError::new)?;
                            if let Some(guidance) = examination.runtime_repair.as_ref() {
                                for server in &mut retry_config.mcp_servers {
                                    if server.secret_id == guidance.connector_id {
                                        if let Some(authority) = server.proxy_authority.as_mut() {
                                            authority.allowed_tools =
                                                guidance.allowed_alternatives.clone();
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(session_id) = observation.opencode_session_id {
                            retry_task.opencode_session_id = Some(session_id);
                        }
                        context.report_progress(
                            TaskProgress {
                                phase: "repairing_capability_plan".to_string(),
                                completed: u64::from(capability_repairs_attempted),
                                total: Some(1),
                            },
                            json!({
                                "runtime_attempt_id": runtime_attempt_id,
                                "repair_number": capability_repairs_attempted,
                            }),
                        )?;
                        continue;
                    }
                }
            }
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "runtime_recovery_decision",
                    "schema_version": decision.schema_version,
                    "failure_class": decision.failure_class,
                    "action": decision.action,
                    "retryable": decision.retryable,
                    "reason": decision.reason,
                    "retries_attempted": retries_attempted,
                    "max_automatic_retries": MAX_AUTOMATIC_RUNTIME_RETRIES,
                    "successful_tool_calls": observation.successful_tool_calls,
                    "successful_mutation_observed": observation.successful_mutation_observed,
                }),
            )?;
            if !decision.should_retry() {
                terminal_recovery_action = Some(decision.action);
                break attempt_result;
            }

            retries_attempted = retries_attempted.saturating_add(1);
            if let Some(session_id) = observation.opencode_session_id {
                retry_task.opencode_session_id = Some(session_id);
            }
            context.report_progress(
                TaskProgress {
                    phase: "recovering_runtime".to_string(),
                    completed: u64::from(retries_attempted),
                    total: Some(u64::from(MAX_AUTOMATIC_RUNTIME_RETRIES)),
                },
                json!({
                    "runtime_attempt_id": runtime_attempt_id,
                    "retry_number": retries_attempted,
                    "reason": decision.reason,
                }),
            )?;
        };
        let provider_duration = provider_request.finish();
        let provider_outcome = match &result {
            Ok(result) if result.completion_status == AiWorkerCompletionStatus::Blocked => {
                "blocked"
            }
            Ok(_) => "succeeded",
            Err(error) if error.contains("[approval_required]") => "blocked",
            Err(_) if context.cancellation().is_cancelled() => "cancelled",
            Err(_) => "failed",
        };
        let bound_opencode_session_id = context.opencode_session_id().ok().flatten();
        emit_execution_trace_stage(
            context,
            "agent_runtime_request",
            provider_duration,
            provider_outcome,
            &runtime_attempt_id,
            bound_opencode_session_id.as_deref(),
        );
        let result = result.map_err(|error| {
            if error.contains("[approval_required]")
                || matches!(
                    terminal_recovery_action,
                    Some(
                        RuntimeRecoveryAction::AwaitOperator
                            | RuntimeRecoveryAction::ReviewUncertainOutcome
                    )
                )
            {
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
            objective_results: result
                .objective_results
                .into_iter()
                .map(|objective| AgentTaskObjectiveResult {
                    objective_id: objective.objective_id,
                    completion_status: match objective.completion_status {
                        AiWorkerCompletionStatus::Completed => AgentTaskCompletionStatus::Completed,
                        AiWorkerCompletionStatus::Blocked => AgentTaskCompletionStatus::Blocked,
                    },
                    evidence: objective.evidence,
                    blocked_reason: objective.blocked_reason,
                })
                .collect(),
        }))
    }
}

fn emit_execution_trace_stage(
    context: &TaskExecutionContext,
    stage: &'static str,
    duration: std::time::Duration,
    outcome: &'static str,
    runtime_id: &str,
    opencode_session_id: Option<&str>,
) {
    let result = context.emit_event(
        TaskSessionEventKind::Runtime,
        json!({
            "type": "execution_trace_stage",
            "schema_version": 1,
            "trace_id": format!("task-session:{}", context.session_id().0),
            "span_id": format!("attempt:{}:{stage}", context.attempt_id()),
            "stage": stage,
            "duration_us": duration.as_micros().min(u64::MAX as u128) as u64,
            "outcome": outcome,
            "worker_id": context.worker_id(),
            "runtime_id": runtime_id,
            "opencode_session_id": opencode_session_id,
        }),
    );
    if result.is_err() {
        crate::infrastructure::performance::increment(
            "execution_trace_events_dropped_total",
            "observability",
            1,
        );
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
        AiWorkerStreamEvent::ObjectiveCheckpoint {
            objective_id,
            evidence,
        } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "objective_checkpoint_candidate",
                "objective_id": objective_id,
                "evidence": evidence,
            }),
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
            arguments_observed: _,
            display_context,
            resource_operation_key,
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
                    "resource_operation_key": resource_operation_key,
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
    use crate::infrastructure::ai_worker::AiWorkerObjectiveResult;
    use crate::infrastructure::tool_broker::{argument_digest, ToolDisplayContext};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    struct FakeResolver {
        attempts: Arc<Mutex<Vec<String>>>,
        runtime_profile_id: String,
    }

    struct UnavailableConnectorResolver;

    struct CapabilityRepairResolver;

    struct CheckpointResolver {
        contract: serde_json::Value,
    }

    impl AgentRuntimeResolver for UnavailableConnectorResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            let mut resolved = FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: envelope.runtime_profile_id.clone(),
            }
            .resolve(
                task_session_id,
                envelope,
                runtime_attempt_id,
                retained_governance,
            )?;
            resolved.connector_capabilities = vec![
                crate::domain::task_examination::ConnectorCapabilitySnapshot {
                    connector_id: "jira".to_string(),
                    status: crate::domain::task_examination::ConnectorDiscoveryStatus::Unavailable,
                    tools: Vec::new(),
                    error: Some("MCP tools/list discovery failed.".to_string()),
                    warnings: Vec::new(),
                },
            ];
            Ok(resolved)
        }
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
                    task_examination: None,
                    session_key: None,
                    opencode_session_id: None,
                },
                governance: retained_governance
                    .cloned()
                    .unwrap_or_else(|| test_governance(task_session_id)),
                connector_capabilities: Vec::new(),
            })
        }
    }

    impl AgentRuntimeResolver for CapabilityRepairResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            let mut resolved = FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: envelope.runtime_profile_id.clone(),
            }
            .resolve(
                task_session_id,
                envelope,
                runtime_attempt_id,
                retained_governance,
            )?;
            resolved.connector_capabilities = vec![
                crate::domain::task_examination::ConnectorCapabilitySnapshot {
                    connector_id: "jira".to_string(),
                    status: crate::domain::task_examination::ConnectorDiscoveryStatus::Available,
                    tools: vec![crate::domain::task_examination::DiscoveredToolCapability {
                        name: "jira_read_issue".to_string(),
                        risk: "read".to_string(),
                        argument_names: vec!["issue_key".to_string()],
                    }],
                    error: None,
                    warnings: Vec::new(),
                },
            ];
            Ok(resolved)
        }
    }

    impl AgentRuntimeResolver for CheckpointResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            _runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            Ok(ResolvedAgentTask {
                runtime_profile_id: envelope.runtime_profile_id.clone(),
                config: test_config(),
                task: AiWorkerTask {
                    execution_contract: Some(self.contract.clone()),
                    task_examination: None,
                    session_key: None,
                    opencode_session_id: None,
                },
                governance: retained_governance
                    .cloned()
                    .unwrap_or_else(|| test_governance(task_session_id)),
                connector_capabilities: Vec::new(),
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

    struct MismatchedToolCompletionRunner;

    struct TransientThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        resumed_session_ids: Arc<Mutex<Vec<Option<String>>>>,
    }

    struct MutationThenTransientFailureRunner {
        executions: Arc<AtomicUsize>,
    }

    struct CapabilityDriftThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        observed_alternatives: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct CheckpointThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        observed_checkpoints: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct MutationCheckpointReplayRunner {
        executions: Arc<AtomicUsize>,
    }

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
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Reading from external tool".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            unreachable!("failed tool callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for MismatchedToolCompletionRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: "call-mismatch".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                risk: "mutation".to_string(),
                arguments_digest: "a".repeat(64),
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-mismatch".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: "b".repeat(64),
                arguments_observed: true,
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
                resource_operation_key: None,
            })?;
            unreachable!("mismatched completion callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for TransientThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.resumed_session_ids
                .lock()
                .expect("resumed sessions lock")
                .push(task.opencode_session_id.clone());
            if execution == 0 {
                on_event(AiWorkerStreamEvent::OpenCodeSession {
                    session_id: "opencode-recovery-session".to_string(),
                    action: "created".to_string(),
                })?;
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-transient".to_string(),
                    tool_name: "confluence_get_page".to_string(),
                    success: false,
                    error: Some("HTTP 503 service unavailable".to_string()),
                    risk: "read".to_string(),
                    arguments_digest: "digest-read".to_string(),
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Read Confluence page".to_string(),
                        category: "external".to_string(),
                        target: None,
                    },
                    resource_operation_key: None,
                })?;
                unreachable!("failed tool callback must terminate the first execution")
            }
            Ok(AiWorkerTaskResult {
                summary: "Recovered and completed".to_string(),
                evidence: vec!["Confluence page was read on retry".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for MutationThenTransientFailureRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-mutation".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: "digest-mutation".to_string(),
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-follow-up".to_string(),
                tool_name: "bamboo_get_build".to_string(),
                success: false,
                error: Some("gateway timeout".to_string()),
                risk: "read".to_string(),
                arguments_digest: "digest-follow-up".to_string(),
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Inspect Bamboo build".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            unreachable!("failed follow-up callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for CapabilityDriftThenCompleteRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.observed_alternatives
                .lock()
                .expect("alternatives lock")
                .push(
                    task.task_examination
                        .as_ref()
                        .and_then(|examination| examination.runtime_repair.as_ref())
                        .map(|repair| repair.allowed_alternatives.clone())
                        .unwrap_or_default(),
                );
            if execution == 0 {
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-stale-tool".to_string(),
                    tool_name: "jira_get_issue".to_string(),
                    success: false,
                    error: Some("unknown tool jira_get_issue".to_string()),
                    risk: "read".to_string(),
                    arguments_digest: "digest-stale".to_string(),
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Read Jira issue".to_string(),
                        category: "jira".to_string(),
                        target: None,
                    },
                    resource_operation_key: None,
                })?;
                unreachable!("missing tool callback must terminate the first execution")
            }
            assert_eq!(
                config.mcp_servers[0]
                    .proxy_authority
                    .as_ref()
                    .expect("repair authority")
                    .allowed_tools,
                vec!["jira_read_issue"]
            );
            Ok(AiWorkerTaskResult {
                summary: "Capability plan repaired".to_string(),
                evidence: vec!["jira_read_issue returned the issue".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for CheckpointThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.observed_checkpoints
                .lock()
                .expect("checkpoint observations lock")
                .push(
                    task.task_examination
                        .as_ref()
                        .map(|examination| {
                            examination
                                .objective_checkpoints
                                .iter()
                                .map(|checkpoint| checkpoint.objective_id.clone())
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            if execution == 0 {
                on_event(AiWorkerStreamEvent::OpenCodeSession {
                    session_id: "opencode-checkpoint-session".to_string(),
                    action: "created".to_string(),
                })?;
                on_event(AiWorkerStreamEvent::ObjectiveCheckpoint {
                    objective_id: "objective-1".to_string(),
                    evidence: vec!["Bamboo build 42 succeeded".to_string()],
                })?;
                return Ok(AiWorkerTaskResult {
                    summary: "Rollout still needs operator input".to_string(),
                    evidence: vec!["Bamboo build 42 succeeded".to_string()],
                    details: Vec::new(),
                    next: vec!["Continue rollout".to_string()],
                    completion_status: AiWorkerCompletionStatus::Blocked,
                    blocked_reason: Some("operator input required".to_string()),
                    objective_results: vec![
                        AiWorkerObjectiveResult {
                            objective_id: "objective-1".to_string(),
                            completion_status: AiWorkerCompletionStatus::Completed,
                            evidence: vec!["Bamboo build 42 succeeded".to_string()],
                            blocked_reason: None,
                        },
                        AiWorkerObjectiveResult {
                            objective_id: "objective-2".to_string(),
                            completion_status: AiWorkerCompletionStatus::Blocked,
                            evidence: Vec::new(),
                            blocked_reason: Some("operator input required".to_string()),
                        },
                    ],
                });
            }
            Ok(AiWorkerTaskResult {
                summary: "Rollout completed without rebuilding".to_string(),
                evidence: vec!["Existing build 42 deployed successfully".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: vec![
                    AiWorkerObjectiveResult {
                        objective_id: "objective-1".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Bamboo build 42 succeeded".to_string()],
                        blocked_reason: None,
                    },
                    AiWorkerObjectiveResult {
                        objective_id: "objective-2".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Rollout healthy".to_string()],
                        blocked_reason: None,
                    },
                ],
            })
        }
    }

    impl AgentRuntimeRunner for MutationCheckpointReplayRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            let digest = argument_digest(&json!({ "plan_key": "QCASH-BUILD" }))?;
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-mutation-checkpoint-session".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: format!("bamboo-call-{execution}"),
                tool_name: "bamboo_trigger_build".to_string(),
                risk: "mutation".to_string(),
                arguments_digest: digest.clone(),
                display_context: ToolDisplayContext {
                    label: "Triggering QCASH-BUILD".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: format!("bamboo-call-{execution}"),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: argument_digest(&json!({}))?,
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Triggering QCASH-BUILD".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
                resource_operation_key: None,
            })?;
            on_event(AiWorkerStreamEvent::ObjectiveCheckpoint {
                objective_id: "objective-1".to_string(),
                evidence: vec!["Bamboo build 842 succeeded".to_string()],
            })?;
            Ok(AiWorkerTaskResult {
                summary: "Deployment still needs operator input".to_string(),
                evidence: vec!["Bamboo build 842 succeeded".to_string()],
                details: Vec::new(),
                next: vec!["Continue deployment".to_string()],
                completion_status: AiWorkerCompletionStatus::Blocked,
                blocked_reason: Some("operator input required".to_string()),
                objective_results: vec![
                    AiWorkerObjectiveResult {
                        objective_id: "objective-1".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Bamboo build 842 succeeded".to_string()],
                        blocked_reason: None,
                    },
                    AiWorkerObjectiveResult {
                        objective_id: "objective-2".to_string(),
                        completion_status: AiWorkerCompletionStatus::Blocked,
                        evidence: Vec::new(),
                        blocked_reason: Some("operator input required".to_string()),
                    },
                ],
            })
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
                objective_results: Vec::new(),
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
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Restart deployment".to_string(),
                        category: "external".to_string(),
                        target: Some("deployment/clbo".to_string()),
                    },
                    resource_operation_key: None,
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
                objective_results: Vec::new(),
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
                objective_results: Vec::new(),
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
                objective_results: Vec::new(),
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
                objective_results: Vec::new(),
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
                arguments_observed: true,
                display_context: ToolDisplayContext {
                    label: "jira_search".to_string(),
                    category: "external".to_string(),
                    target: Some(authority.session_id.0.to_string()),
                },
                resource_operation_key: None,
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
                objective_results: Vec::new(),
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
        let trace = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .into_iter()
            .filter(|event| event.payload["type"] == "execution_trace_stage")
            .collect::<Vec<_>>();
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].payload["stage"], "runtime_preparation");
        assert_eq!(trace[1].payload["stage"], "agent_runtime_request");
        for event in &trace {
            assert_eq!(event.payload["schema_version"], 1);
            assert_eq!(
                event.payload["trace_id"],
                format!("task-session:{}", session.id.0)
            );
            assert_eq!(event.payload["worker_id"], 1);
            assert!(event.payload["duration_us"].as_u64().is_some());
            assert!(event.payload["runtime_id"]
                .as_str()
                .is_some_and(|runtime_id| runtime_id.contains("attempt-")));
        }
        assert!(trace[0].payload["opencode_session_id"].is_null());
        assert_eq!(
            trace[1].payload["opencode_session_id"],
            "opencode-session-fake"
        );
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
    fn scheduler_agent_executor_blocks_before_runner_when_live_tools_are_unavailable() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(UnavailableConnectorResolver),
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
                "unavailable-live-tools",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");

        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("MCP capability preflight blocked execution") }));
        assert!(engine
            .events_after(session.id, 0)
            .expect("events")
            .iter()
            .any(|event| {
                event.payload["type"] == "task_examined"
                    && event.payload["live_connector_count"] == 0
            }));
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
        let recovery = events
            .iter()
            .filter(|event| event.payload["type"] == "runtime_recovery_decision")
            .collect::<Vec<_>>();
        assert_eq!(recovery.len(), 2);
        assert_eq!(recovery[0].payload["action"], "retry_current_assignment");
        assert_eq!(recovery[1].payload["action"], "stop_failed");
    }

    #[test]
    fn scheduler_agent_executor_rejects_mismatched_tool_completion_identity() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(MismatchedToolCompletionRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mismatched-tool-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");

        assert_eq!(completed.state, TaskSessionState::Failed);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("did not match its started tool identity") }));
    }

    #[test]
    fn transient_read_failure_retries_once_in_the_same_opencode_session() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let resumed_session_ids = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(TransientThenCompleteRunner {
                executions: executions.clone(),
                resumed_session_ids: resumed_session_ids.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "transient-recovery-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task recovers");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            resumed_session_ids
                .lock()
                .expect("sessions lock")
                .as_slice(),
            &[None, Some("opencode-recovery-session".to_string())]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let recovery = events
            .iter()
            .find(|event| event.payload["type"] == "runtime_recovery_decision")
            .expect("recovery decision journaled");
        assert_eq!(recovery.payload["failure_class"], "transient_transport");
        assert_eq!(recovery.payload["action"], "retry_current_assignment");
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Progress
                && event.progress.as_ref().is_some_and(|progress| {
                    progress.phase == "recovering_runtime" && progress.completed == 1
                })
        }));
    }

    #[test]
    fn transient_failure_after_mutation_blocks_without_replay() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(MutationThenTransientFailureRunner {
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
                "uncertain-mutation-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");

        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let recovery = events
            .iter()
            .find(|event| event.payload["type"] == "runtime_recovery_decision")
            .expect("recovery decision journaled");
        assert_eq!(recovery.payload["action"], "review_uncertain_outcome");
        assert_eq!(recovery.payload["successful_mutation_observed"], true);
    }

    #[test]
    fn missing_read_tool_repairs_from_live_inventory_without_expanding_authority() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let observed_alternatives = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(CapabilityRepairResolver),
            Arc::new(CapabilityDriftThenCompleteRunner {
                executions: executions.clone(),
                observed_alternatives: observed_alternatives.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "capability-repair-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task repairs capability plan");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            observed_alternatives
                .lock()
                .expect("alternatives lock")
                .as_slice(),
            &[Vec::<String>::new(), vec!["jira_read_issue".to_string()]]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let repair = events
            .iter()
            .find(|event| event.payload["type"] == "capability_repair_decision")
            .expect("capability repair journaled");
        assert_eq!(repair.payload["repairable"], true);
        assert_eq!(repair.payload["connector_id"], "jira");
        assert_eq!(repair.payload["allowed_alternatives"][0], "jira_read_issue");
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Progress
                && event.progress.as_ref().is_some_and(|progress| {
                    progress.phase == "repairing_capability_plan" && progress.completed == 1
                })
        }));
    }

    #[test]
    fn blocked_continuation_retains_completed_objective_checkpoint() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "checkpoint-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    { "id": "objective-1", "summary": "Trigger Bamboo build", "success_evidence": "successful build" },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout" }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executions = Arc::new(AtomicUsize::new(0));
        let observed_checkpoints = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(CheckpointThenCompleteRunner {
                executions: executions.clone(),
                observed_checkpoints: observed_checkpoints.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "checkpoint-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks with partial completion");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        engine
            .continue_interrupted_session(
                session.id,
                "checkpoint-agent-continued",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            observed_checkpoints
                .lock()
                .expect("checkpoint observations lock")
                .as_slice(),
            &[Vec::<String>::new(), vec!["objective-1".to_string()]]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        assert!(events.iter().any(|event| {
            event.payload["type"] == "objective_checkpointed"
                && event.payload["objective_id"] == "objective-1"
                && event.payload["new_checkpoint"] == true
        }));
        assert!(events.iter().any(|event| {
            event.payload["type"] == "task_examined"
                && event.payload["objective_checkpoint_count"] == 1
        }));
    }

    #[test]
    fn mutation_objective_rejects_checkpoint_without_successful_mutation_event() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "mutation-checkpoint-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    {
                        "id": "objective-1",
                        "summary": "Trigger Bamboo build",
                        "success_evidence": "successful build",
                        "mutation_expected": true
                    },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout" }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(CheckpointThenCompleteRunner {
                executions: Arc::new(AtomicUsize::new(0)),
                observed_checkpoints: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mutation-checkpoint-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let failed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("invalid mutation checkpoint fails");

        assert_eq!(failed.state, TaskSessionState::Failed);
        assert!(failed.error.as_deref().is_some_and(
            |error| error.contains("cannot checkpoint before a successful mutation tool event")
        ));
        let events = engine.events_after(session.id, 0).expect("events replayed");
        assert!(!events
            .iter()
            .any(|event| event.payload["type"] == "objective_checkpointed"));
    }

    #[test]
    fn continuation_rejects_identical_mutation_from_checkpoint_receipt() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "mutation-replay-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    {
                        "id": "objective-1",
                        "summary": "Trigger Bamboo build",
                        "success_evidence": "successful build",
                        "mutation_expected": true
                    },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout", "mutation_expected": true }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(MutationCheckpointReplayRunner {
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
                "mutation-replay-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("first attempt blocks after checkpoint");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let first_events = engine.events_after(session.id, 0).expect("events replayed");
        let checkpoint = first_events
            .iter()
            .find(|event| event.payload["type"] == "objective_checkpointed")
            .expect("checkpoint event retained");
        assert_eq!(checkpoint.payload["tool_receipt_count"], 1);
        assert_eq!(
            checkpoint.payload["tool_receipts"][0]["arguments_digest"],
            argument_digest(&json!({ "plan_key": "QCASH-BUILD" })).expect("digest")
        );

        engine
            .continue_interrupted_session(
                session.id,
                "mutation-replay-agent-continued",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        let failed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("replayed mutation fails");

        assert_eq!(failed.state, TaskSessionState::Failed);
        assert!(failed.error.as_deref().is_some_and(|error| {
            error.contains("replays completed objective 'objective-1' with identical arguments")
        }));
        assert_eq!(executions.load(Ordering::SeqCst), 2);
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
                task_examination: None,
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
            connector_capabilities: Vec::new(),
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
                task_examination: None,
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
            connector_capabilities: Vec::new(),
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

    fn repository_fact(local_path: Option<String>) -> RepositoryRuleFact {
        RepositoryRuleFact {
            id: "qcash-deployment".to_string(),
            remote_url: "https://bitbucket.example/projects/OPS/repos/qcash-deployment".to_string(),
            local_path,
            backend_path: Some("service".to_string()),
            frontend_path: Some("frontend".to_string()),
            source: "global.agent_rules".to_string(),
            source_line: 2,
        }
    }

    fn initialize_repository(path: &Path) {
        std::fs::create_dir_all(path).expect("repository directory");
        let status = std::process::Command::new(
            crate::infrastructure::git::git_executable().expect("git executable"),
        )
        .args(["init", "--quiet"])
        .current_dir(path)
        .status()
        .expect("git init runs");
        assert!(status.success());
    }

    #[test]
    fn repository_preflight_discovers_unique_named_checkout_and_records_provenance() {
        let directory = tempdir().expect("workspace");
        let repository = directory.path().join("BRI").join("qcash-deployment");
        initialize_repository(&repository);
        let mut fact = repository_fact(None);
        let secret = "repository-secret";
        fact.remote_url = format!(
            "https://user:{secret}@bitbucket.example/projects/OPS/repos/qcash-deployment?token={secret}"
        );
        let facts = RuleFactsRecord {
            repositories: vec![fact],
            ..Default::default()
        };

        let preflight = resolve_repository_preflight(
            &json!({ "objective": { "summary": "Create qcash-deployment Helm template" } }),
            &facts,
            directory.path(),
        );

        assert!(preflight.blocker.is_none());
        assert_eq!(
            preflight.repository_root.as_deref(),
            Some(
                repository
                    .canonicalize()
                    .expect("repository root")
                    .as_path()
            )
        );
        let record = preflight.record.expect("resolution record");
        assert_eq!(record.status, "resolved");
        assert_eq!(record.repository_id.as_deref(), Some("qcash-deployment"));
        assert_eq!(record.source_line, 2);
        assert_eq!(record.backend_path.as_deref(), Some("service"));
        assert!(!serde_json::to_string(&record)
            .expect("resolution serializes")
            .contains(secret));
    }

    #[test]
    fn repository_preflight_blocks_ambiguous_and_escaping_checkouts() {
        let directory = tempdir().expect("workspace");
        for parent in ["one", "two"] {
            initialize_repository(&directory.path().join(parent).join("qcash-deployment"));
        }
        let facts = RuleFactsRecord {
            repositories: vec![repository_fact(None)],
            ..Default::default()
        };
        let ambiguous = resolve_repository_preflight(&json!({}), &facts, directory.path());
        assert_eq!(
            ambiguous.record.expect("ambiguous record").status,
            "ambiguous"
        );
        assert!(ambiguous.blocker.is_some());

        let outside = tempdir().expect("outside");
        initialize_repository(outside.path());
        let escaping_facts = RuleFactsRecord {
            repositories: vec![repository_fact(Some(
                outside.path().to_string_lossy().to_string(),
            ))],
            ..Default::default()
        };
        let escaping = resolve_repository_preflight(&json!({}), &escaping_facts, directory.path());
        assert_eq!(
            escaping.record.expect("escape record").status,
            "outside_workspace"
        );
        assert!(escaping.blocker.is_some());
    }

    #[test]
    fn repository_preflight_blocks_conflicting_contract_and_rule_paths() {
        let directory = tempdir().expect("workspace");
        let ruled = directory.path().join("ruled").join("qcash-deployment");
        let contracted = directory.path().join("contracted").join("qcash-deployment");
        initialize_repository(&ruled);
        initialize_repository(&contracted);
        let facts = RuleFactsRecord {
            repositories: vec![repository_fact(Some(ruled.to_string_lossy().to_string()))],
            ..Default::default()
        };

        let preflight = resolve_repository_preflight(
            &json!({
                "repository": {
                    "root_path": contracted.to_string_lossy(),
                }
            }),
            &facts,
            directory.path(),
        );

        assert_eq!(
            preflight.record.expect("conflict record").status,
            "conflict"
        );
        assert!(preflight
            .blocker
            .as_deref()
            .is_some_and(|reason| reason.contains("conflicts")));
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
                facts: Default::default(),
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
