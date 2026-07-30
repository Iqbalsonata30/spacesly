mod application;
mod domain;
mod infrastructure;

pub fn run_mcp_proxy() -> Result<(), String> {
    infrastructure::mcp::run_mcp_proxy_from_env()
}

pub fn run_task_tools() -> Result<(), String> {
    infrastructure::task_tools::run_task_tools_from_env()
}

use application::agent_task_executor::{
    execution_contract_digest, AgentTaskExecutor, AiWorkerRuntimeRunner,
};
use application::app::AppState;
use application::execution_engine::{ExecutionEngine, SchedulerHealth};
use application::files_service::FilesService;
use application::git_service::GitService;
use application::jira_service::JiraService;
use application::prompt_task_executor::{
    prompt_input_digest, AiWorkerPromptRuntimeRunner, PromptTaskExecutor, TaskSessionExecutor,
};
use application::stored_agent_runtime_resolver::StoredAgentRuntimeResolver;
use domain::entity::Workspace;
use domain::execution::ExecutionRun;
use domain::task_session::{
    TaskMcpContext, TaskSessionEnvelope, TaskSessionEventPage, TaskSessionId, TaskSessionInputV2,
    TaskSessionResult, TaskSessionSnapshot, TaskSessionUpdate, TaskToolState,
};
use infrastructure::ai_event::AiRuntimeEvent;
use infrastructure::ai_run::{AiRun, AiRunKind, AiRunRegistry, AiRunStatus};
use infrastructure::ai_worker::{
    chat_ai_worker as chat_ai_worker_impl,
    chat_ai_worker_streaming as chat_ai_worker_streaming_impl, close_all_opencode_servers,
    execute_ai_worker_task as execute_ai_worker_task_impl, propose_ai_edit as propose_ai_edit_impl,
    test_ai_worker as test_ai_worker_impl, AgentRunRegistry, AiEditRequest, AiEditResult,
    AiWorkerChatRequest, AiWorkerChatResult, AiWorkerConfig, AiWorkerStatus, AiWorkerStreamEvent,
    AiWorkerTask, AiWorkerTaskResult,
};
use infrastructure::execution_store::{
    ConversationImportInput, ConversationMessageInput, ExecutionStore,
};
use infrastructure::file_watcher::FileWatchRegistry;
use infrastructure::files::{
    FileEntry, FileSnapshot, FileWriteResult, LineEnding, TextEncoding, WorkspaceRoot,
};
use infrastructure::formatting::format_code as format_code_impl;
use infrastructure::git::git_info_for_path;
use infrastructure::git::{
    invalidate_workspace_git_status, CommitResult, GitStatus, GitWorkspaceInfo,
};
use infrastructure::global_environment::GlobalEnvironmentStore;
use infrastructure::lsp::{
    LspCodeAction, LspCodeActionRequest, LspCompletionRequest, LspCompletionResult,
    LspDiagnosticReport, LspDocumentSymbol, LspHoverResult, LspLocation, LspPosition, LspRegistry,
    LspServerConfig, LspServerStatus,
};
use infrastructure::mcp::{
    close_all_mcp_sessions, close_mcp_session, JiraBoard, JiraConnectionStatus, JiraIssue,
    JiraMcpConfig, McpConnectionStatus, McpServerConfig,
};
use infrastructure::provider_registry::profile as provider_profile;
use infrastructure::pty::{
    close_all_terminals, close_pty_terminal as close_pty_terminal_impl,
    open_pty_terminal as open_pty_terminal_impl,
    pty_current_directory as pty_current_directory_impl,
    resize_pty_terminal as resize_pty_terminal_impl, write_pty_terminal as write_pty_terminal_impl,
    PtyRegistry, PtyState,
};
use infrastructure::recovery_store::{RecoverySnapshot, RecoverySnapshotInput, RecoveryStore};
use infrastructure::runtime_profile_store::{AgentRuntimeProfile, RuntimeProfileStore};
use infrastructure::scheduler_store::SchedulerStore;
use infrastructure::secrets::{AppSecrets, AppSecretsStore, JiraConnectionProfile};
use infrastructure::shell::{
    complete_shell_input as complete_shell_input_impl, run_shell_command as run_shell_command_impl,
    ShellCommandRequest, ShellCommandResult, ShellCompletionRequest, ShellCompletionResult,
};
use infrastructure::tool_broker::{operation_id, ToolAuthorization, ToolBroker};
use infrastructure::workspace_cache::{
    load_cached_workspace as load_cached_workspace_impl,
    save_cached_workspace as save_cached_workspace_impl, CachedWorkspace,
};
use infrastructure::workspace_search::{
    apply_workspace_replace as apply_workspace_replace_impl,
    preview_workspace_replace as preview_workspace_replace_impl,
    search_workspace as search_workspace_impl, WorkspaceReplaceApplyRequest,
    WorkspaceReplaceApplyResponse, WorkspaceReplacePreviewRequest, WorkspaceReplacePreviewResponse,
    WorkspaceSearchRequest, WorkspaceSearchResponse,
};
use infrastructure::workspace_trust::{WorkspaceTrustRegistry, WorkspaceTrustStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

const TASK_SESSION_UPDATE_EVENT: &str = "task-session-update";

/// Each MCP request blocks for up to 45 s (recv_timeout inside StdioMcpClient::request).
/// Session initialisation makes two sequential requests: initialize + tools/list = up to 90 s.
/// The test also calls tool_metadata (cached after tools/list, 0 s extra) plus one optional
/// tool call.  120 s covers the full cold-start path with margin and still leaves the frontend
/// mcpTest policy (180 s) a comfortable 60 s gap to receive the structured error response.
const MCP_TEST_CONNECTION_TIMEOUT: Duration = Duration::from_secs(120);
/// Server-side guard for Jira sync operations.  The frontend jiraRead policy is 120 s, but
/// MCP requests can individually block for up to 45 s.  This server-side timeout ensures the
/// backend cancels cleanly and returns a structured error to the frontend before the IPC
/// channel goes silent, avoiding the timeout mismatch that caused the frontend to see
/// "request timed out" with no context.
const JIRA_SYNC_TIMEOUT: Duration = Duration::from_secs(90);

fn workspace_id_or_default(workspace_id: Option<String>) -> String {
    workspace_id.unwrap_or_else(|| "workspace-personal".to_string())
}

fn mcp_ipc_error(operation: &str, error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (category, retryable) = if lower.contains("timed out") || lower.contains("timeout") {
        ("timeout", true)
    } else if lower.contains("not found")
        || lower.contains("command is required")
        || lower.contains("could not find")
        || lower.contains("did not return")
        || lower.contains("did not contain")
    {
        ("validation", false)
    } else if lower.contains("401")
        || lower.contains("403")
        || lower.contains("authentication")
        || lower.contains("credential")
        || lower.contains("token")
        || lower.contains("permission")
    {
        ("auth", false)
    } else if lower.contains("failed to start")
        || lower.contains("connection")
        || lower.contains("temporarily")
        || lower.contains("unavailable")
    {
        ("transient", true)
    } else {
        ("unknown", false)
    };

    serde_json::json!({
        "category": category,
        "message": format!("{operation}: {message}"),
        "retryable": retryable,
    })
    .to_string()
}

fn file_ipc_error(operation: &str, error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let (category, retryable) = if lower.contains("timed out") || lower.contains("timeout") {
        ("timeout", true)
    } else if lower.contains("changed on disk")
        || lower.contains("workspace root changed")
        || lower.contains("already exists")
    {
        ("conflict", false)
    } else if lower.contains("permission denied") || lower.contains("access denied") {
        ("permission", false)
    } else if lower.contains("too large") {
        ("too_large", false)
    } else if lower.contains("utf-8") || lower.contains("utf-16") || lower.contains("binary file") {
        ("encoding", false)
    } else if lower.contains("does not exist")
        || lower.contains("not found")
        || lower.contains("no such file")
    {
        ("not_found", false)
    } else if lower.contains("temporarily")
        || lower.contains("failed to start")
        || lower.contains("resource busy")
        || lower.contains("would block")
        || lower.contains("interrupted")
    {
        ("transient", true)
    } else if lower.contains("path")
        || lower.contains("directory")
        || lower.contains("file name")
        || lower.contains("query")
        || lower.contains("replacement")
        || lower.contains("truncated")
    {
        ("validation", false)
    } else {
        ("unknown", false)
    };

    serde_json::json!({
        "category": category,
        "message": format!("{operation}: {message}"),
        "retryable": retryable,
    })
    .to_string()
}

#[tauri::command]
fn get_workspace(app_state: State<'_, AppState>) -> Workspace {
    app_state.workspace()
}

#[tauri::command]
async fn get_jira_issues(
    mut config: JiraMcpConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<Vec<JiraIssue>, String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || JiraService::new().issues(config))
        .await
        .map_err(|error| mcp_ipc_error("Jira issue task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira issue request failed", error))
}

#[tauri::command]
async fn get_jira_boards(
    mut config: JiraMcpConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<Vec<JiraBoard>, String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || JiraService::new().boards(config))
        .await
        .map_err(|error| mcp_ipc_error("Jira board task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira board request failed", error))
}

#[tauri::command]
async fn test_jira_mcp_connection(
    mut config: JiraMcpConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<JiraConnectionStatus, String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    // Drop any stale session or in-progress init lock from a previous timed-out attempt.
    // Without this, a hung spawn_blocking thread from a prior test holds the init-lock Arc;
    // the next call acquires the same Arc and deadlocks waiting for the previous thread.
    let cleanup_server = config.server.clone();
    let _ = close_mcp_session(config.server.clone());
    let task = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().test_jira_connection(config)
    });
    let result = tokio::time::timeout(MCP_TEST_CONNECTION_TIMEOUT, task)
        .await
        .map_err(|_| {
            let _ = close_mcp_session(cleanup_server);
            mcp_ipc_error(
                "Jira MCP test failed",
                "request timed out after 120 seconds while testing the Jira connector",
            )
        })?
        .map_err(|error| mcp_ipc_error("Jira MCP test task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira MCP test failed", error))
}

#[tauri::command]
async fn sync_recovery_snapshots(
    workspace_id: String,
    snapshots: Vec<RecoverySnapshotInput>,
    workspace_root: State<'_, WorkspaceRoot>,
    recovery_store: State<'_, RecoveryStore>,
) -> Result<(), String> {
    let root = workspace_root.inner().clone();
    let store = recovery_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.sync_workspace(&root, workspace_id, snapshots)
    })
    .await
    .map_err(|error| file_ipc_error("Recovery snapshot sync failed", error))?
    .map_err(|error| file_ipc_error("Recovery snapshot sync failed", error))
}

#[tauri::command]
async fn list_recovery_snapshots(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
    recovery_store: State<'_, RecoveryStore>,
) -> Result<Vec<RecoverySnapshot>, String> {
    let root = workspace_root.inner().clone();
    let store = recovery_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_workspace(&root, workspace_id))
        .await
        .map_err(|error| file_ipc_error("Recovery snapshot load failed", error))?
        .map_err(|error| file_ipc_error("Recovery snapshot load failed", error))
}

#[tauri::command]
async fn delete_recovery_snapshot(
    workspace_id: String,
    path: String,
    workspace_root: State<'_, WorkspaceRoot>,
    recovery_store: State<'_, RecoveryStore>,
) -> Result<(), String> {
    let root = workspace_root.inner().clone();
    let store = recovery_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.delete_snapshot(&root, workspace_id, path))
        .await
        .map_err(|error| file_ipc_error("Recovery snapshot delete failed", error))?
        .map_err(|error| file_ipc_error("Recovery snapshot delete failed", error))
}

#[tauri::command]
async fn test_mcp_server_connection(
    mut config: McpServerConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<McpConnectionStatus, String> {
    resolve_mcp_secret_environment(&mut config, secrets.inner())?;
    let cleanup_config = config.clone();
    let _ = close_mcp_session(config.clone());
    let task = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().test_mcp_connection(config)
    });
    let result = tokio::time::timeout(MCP_TEST_CONNECTION_TIMEOUT, task)
        .await
        .map_err(|_| {
            let _ = close_mcp_session(cleanup_config);
            mcp_ipc_error(
                "MCP test failed",
                "request timed out after 60 seconds while testing the connector",
            )
        })?
        .map_err(|error| mcp_ipc_error("MCP test task failed", error))?;
    result.map_err(|error| mcp_ipc_error("MCP test failed", error))
}

#[tauri::command]
async fn disconnect_mcp_server(
    mut config: McpServerConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<bool, String> {
    resolve_mcp_secret_environment(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().disconnect_mcp_server(config)
    })
    .await
    .map_err(|error| mcp_ipc_error("MCP disconnect task failed", error))?;
    result.map_err(|error| mcp_ipc_error("MCP disconnect failed", error))
}

#[tauri::command]
async fn sync_jira_workspace(
    mut config: JiraMcpConfig,
    app_state: State<'_, AppState>,
    secrets: State<'_, AppSecretsStore>,
) -> Result<Workspace, String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    // Clone the seeded workspace once from the managed singleton — avoids reconstructing it per call.
    let base_workspace = app_state.workspace();
    let task = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().sync_workspace_from(base_workspace, config)
    });
    let result = tokio::time::timeout(JIRA_SYNC_TIMEOUT, task)
        .await
        .map_err(|_| {
            mcp_ipc_error(
                "Jira sync failed",
                "request timed out after 90 seconds while syncing Jira board",
            )
        })?
        .map_err(|error| mcp_ipc_error("Jira sync task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira sync failed", error))
}

#[tauri::command]
async fn transition_jira_issue(
    mut config: JiraMcpConfig,
    issue_key: String,
    target_status: String,
    secrets: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().transition_issue(config, issue_key, target_status)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira transition task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira transition failed", error))
}

#[tauri::command]
async fn assign_jira_issue(
    mut config: JiraMcpConfig,
    issue_key: String,
    secrets: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().assign_issue(config, issue_key)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira assign task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira assign failed", error))
}

#[tauri::command]
async fn add_jira_comment(
    mut config: JiraMcpConfig,
    issue_key: String,
    comment: String,
    secrets: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    resolve_jira_secrets(&mut config, secrets.inner())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().add_comment(config, issue_key, comment)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira comment task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira comment failed", error))
}

#[tauri::command]
async fn test_ai_worker(
    mut config: AiWorkerConfig,
    secrets: State<'_, AppSecretsStore>,
) -> Result<AiWorkerStatus, String> {
    resolve_ai_secrets(&mut config, secrets.inner())?;
    tauri::async_runtime::spawn_blocking(move || test_ai_worker_impl(config))
        .await
        .map_err(|error| format!("Agent diagnostic task failed: {error}"))?
}

#[tauri::command]
fn reserve_ai_worker_run(
    run_id: String,
    mut config: AiWorkerConfig,
    agent_runs: State<'_, AgentRunRegistry>,
    ai_runs: State<'_, AiRunRegistry>,
    workspace_root: State<'_, WorkspaceRoot>,
    workspace_trust: State<'_, WorkspaceTrustRegistry>,
) -> Result<(), String> {
    bind_tool_capable_ai_workspace(&mut config, &workspace_root, &workspace_trust)?;
    ai_runs.begin(run_id.clone(), AiRunKind::Agent)?;
    if let Err(error) = agent_runs.reserve(&run_id, &config) {
        let _ = ai_runs.finish(&run_id, AiRunStatus::Failed);
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
fn get_ai_run(run_id: String, ai_runs: State<'_, AiRunRegistry>) -> Result<Option<AiRun>, String> {
    ai_runs.get(&run_id)
}

#[tauri::command]
fn grant_ai_run_capabilities(
    run_id: String,
    capabilities: Vec<String>,
    ai_runs: State<'_, AiRunRegistry>,
    execution_store: State<'_, ExecutionStore>,
) -> Result<(), String> {
    ai_runs.grant_capabilities(&run_id, capabilities.clone())?;
    let payload = serde_json::json!({
        "capabilities": capabilities,
        "source": "operator_confirmation",
    });
    execution_store.record_ai_audit(Some(&run_id), "capabilities_granted", &payload)
}

#[tauri::command]
fn begin_ai_run(kind: AiRunKind, ai_runs: State<'_, AiRunRegistry>) -> Result<AiRun, String> {
    ai_runs.begin_generated(kind)
}

#[tauri::command]
async fn execute_ai_worker_task(
    run_id: String,
    mut config: AiWorkerConfig,
    task: AiWorkerTask,
    agent_runs: State<'_, AgentRunRegistry>,
    ai_runs: State<'_, AiRunRegistry>,
    execution_store: State<'_, ExecutionStore>,
    workspace_root: State<'_, WorkspaceRoot>,
    workspace_trust: State<'_, WorkspaceTrustRegistry>,
    secrets: State<'_, AppSecretsStore>,
    on_event: Channel<AiRuntimeEvent>,
) -> Result<AiWorkerTaskResult, String> {
    bind_tool_capable_ai_workspace(&mut config, &workspace_root, &workspace_trust)?;
    resolve_ai_secrets(&mut config, secrets.inner())?;
    let persisted_run = execution_store
        .get(&run_id)?
        .ok_or_else(|| "Durable execution run was not found.".to_string())?;
    let submitted_contract = task
        .execution_contract
        .as_ref()
        .ok_or_else(|| "Execution contract is required.".to_string())?;
    if submitted_contract != &persisted_run.contract {
        return Err("Execution contract does not match the persisted run.".to_string());
    }
    ai_runs.require_capabilities(
        &run_id,
        &["workspace_read", "workspace_write", "shell", "git"],
    )?;
    for server in &config.mcp_servers {
        let capability = format!("external_tools:{}", server.secret_id);
        ai_runs.require_capabilities(&run_id, &[capability.as_str()])?;
    }
    let tool_broker = ToolBroker::new(
        ai_runs.granted_capabilities(&run_id)?,
        config
            .mcp_servers
            .iter()
            .map(|server| (server.name.clone(), server.secret_id.clone())),
    );
    let registry = agent_runs.inner().clone();
    registry.start(&run_id)?;
    let runtime = ai_runs.inner().clone();
    if let Err(error) = runtime.start(&run_id) {
        let _ = registry.finish(&run_id);
        return Err(error);
    }
    emit_ai_event(
        Some(&on_event),
        AiRuntimeEvent::RunStarted {
            run_id: run_id.clone(),
            sequence: 1,
        },
    );
    let cancellation = runtime.cancellation(&run_id)?;
    let store = execution_store.inner().clone();
    let worker_run_id = run_id.clone();
    let worker_stream_channel = on_event.clone();
    let worker_stream_sequence = Arc::new(AtomicU64::new(2));
    let worker_stream_sequence_for_task = worker_stream_sequence.clone();
    if let Err(error) = store.claim_step(&run_id, "worker.execute", &run_id, 15 * 60 * 1000) {
        let _ = registry.finish(&run_id);
        let _ = runtime.finish(&run_id, AiRunStatus::Failed);
        return Err(error);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let worker_stream_run_id = worker_run_id.clone();
        let worker_stream_store = store.clone();
        let worker_callback: Box<dyn FnMut(AiWorkerStreamEvent) -> Result<(), String> + Send> =
            Box::new(move |event| {
                if let AiWorkerStreamEvent::ToolStarted { tool_name, .. } = &event {
                    if let ToolAuthorization::ApprovalRequired { capability, risk } =
                        tool_broker.authorize(tool_name)
                    {
                        let operation_id = operation_id(
                            &worker_stream_run_id,
                            match &event {
                                AiWorkerStreamEvent::ToolStarted { tool_call_id, .. } => {
                                    tool_call_id
                                }
                                _ => unreachable!(),
                            },
                            tool_name,
                            risk,
                            match &event {
                                AiWorkerStreamEvent::ToolStarted {
                                    arguments_digest, ..
                                } => arguments_digest,
                                _ => unreachable!(),
                            },
                        );
                        let arguments_digest = match &event {
                            AiWorkerStreamEvent::ToolStarted {
                                arguments_digest, ..
                            } => arguments_digest.clone(),
                            _ => unreachable!(),
                        };
                        let payload = serde_json::json!({
                            "capability": capability,
                            "operation": tool_name,
                            "risk": risk,
                            "operation_id": operation_id,
                            "arguments_digest": arguments_digest,
                        });
                        let _ = worker_stream_store.record_ai_audit(
                            Some(&worker_stream_run_id),
                            "approval_required",
                            &payload,
                        );
                        emit_ai_event(
                            Some(&worker_stream_channel),
                            AiRuntimeEvent::ApprovalRequired {
                                run_id: worker_stream_run_id.clone(),
                                sequence: worker_stream_sequence_for_task
                                    .fetch_add(1, Ordering::Relaxed),
                                capability: capability.clone(),
                                operation: tool_name.clone(),
                                risk: risk.as_str().to_string(),
                                operation_id,
                                arguments_digest,
                            },
                        );
                        return Err(format!(
                            "Tool operation '{tool_name}' requires the '{capability}' capability."
                        ));
                    }
                }
                emit_worker_stream_event(
                    &worker_stream_channel,
                    &worker_stream_run_id,
                    &worker_stream_sequence_for_task,
                    event,
                    Some(&worker_stream_store),
                );
                Ok(())
            });
        let result = execute_ai_worker_task_impl(config, task, cancellation, Some(worker_callback));
        let (status, summary) = match &result {
            Ok(value)
                if value.completion_status
                    == infrastructure::ai_worker::AiWorkerCompletionStatus::Completed =>
            {
                ("completed", Some(value.summary.as_str()))
            }
            Ok(value) => ("blocked", Some(value.summary.as_str())),
            Err(error) if error.to_lowercase().contains("cancelled") => {
                ("cancelled", Some(error.as_str()))
            }
            Err(error) => ("failed", Some(error.as_str())),
        };
        let _ = store.finish_step(
            &worker_run_id,
            "worker.execute",
            &worker_run_id,
            status,
            summary,
        );
        let _ = registry.finish(&worker_run_id);
        let runtime_status = match status {
            "completed" => AiRunStatus::Completed,
            "blocked" => AiRunStatus::Blocked,
            "cancelled" => AiRunStatus::Cancelled,
            _ => AiRunStatus::Failed,
        };
        let _ = runtime.finish(&worker_run_id, runtime_status);
        let event = match status {
            "completed" => AiRuntimeEvent::RunCompleted {
                run_id: worker_run_id.clone(),
                sequence: worker_stream_sequence.fetch_add(1, Ordering::Relaxed),
            },
            "blocked" => AiRuntimeEvent::RunBlocked {
                run_id: worker_run_id.clone(),
                sequence: worker_stream_sequence.fetch_add(1, Ordering::Relaxed),
            },
            "cancelled" => AiRuntimeEvent::RunCancelled {
                run_id: worker_run_id.clone(),
                sequence: worker_stream_sequence.fetch_add(1, Ordering::Relaxed),
            },
            _ => AiRuntimeEvent::RunFailed {
                run_id: worker_run_id.clone(),
                sequence: worker_stream_sequence.fetch_add(1, Ordering::Relaxed),
                error_code: "agent_execution_failed".to_string(),
            },
        };
        emit_ai_event(Some(&on_event), event);
        result
    })
    .await
    .map_err(|error| format!("Agent execution task failed: {error}"))?;
    if result.is_err() {
        let _ = ai_runs.finish(&run_id, AiRunStatus::Failed);
    }
    result
}

#[tauri::command]
fn release_ai_worker_run(
    run_id: String,
    agent_runs: State<'_, AgentRunRegistry>,
    ai_runs: State<'_, AiRunRegistry>,
) -> Result<bool, String> {
    let released = agent_runs.release_reservation(&run_id)?;
    if released {
        let _ = ai_runs.finish(&run_id, AiRunStatus::Cancelled);
    }
    Ok(released)
}

#[tauri::command]
fn cancel_ai_worker_task(
    run_id: String,
    agent_runs: State<'_, AgentRunRegistry>,
    ai_runs: State<'_, AiRunRegistry>,
) -> Result<bool, String> {
    let cancelled = agent_runs.cancel(&run_id)?;
    if cancelled {
        let _ = ai_runs.cancel(&run_id);
    }
    Ok(cancelled)
}

#[tauri::command]
fn cancel_ai_run(run_id: String, ai_runs: State<'_, AiRunRegistry>) -> Result<bool, String> {
    ai_runs.cancel(&run_id)
}

#[tauri::command]
async fn list_conversations(
    workspace_id: String,
    execution_store: State<'_, ExecutionStore>,
) -> Result<Vec<infrastructure::execution_store::ConversationRecord>, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_conversations(&workspace_id))
        .await
        .map_err(|error| format!("Conversation list task failed: {error}"))?
}

#[tauri::command]
async fn load_conversation_messages(
    workspace_id: String,
    conversation_id: String,
    execution_store: State<'_, ExecutionStore>,
) -> Result<Vec<infrastructure::execution_store::ConversationMessageRecord>, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.load_conversation_messages(&workspace_id, &conversation_id)
    })
    .await
    .map_err(|error| format!("Conversation load task failed: {error}"))?
}

#[tauri::command]
async fn append_conversation_message(
    workspace_id: String,
    conversation_id: String,
    title: String,
    message: ConversationMessageInput,
    execution_store: State<'_, ExecutionStore>,
) -> Result<infrastructure::execution_store::ConversationMessageRecord, String> {
    validate_renderer_conversation_role(&message)?;
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.append_conversation_message(&workspace_id, &conversation_id, &title, &message)
    })
    .await
    .map_err(|error| format!("Conversation append task failed: {error}"))?
}

fn validate_renderer_conversation_role(message: &ConversationMessageInput) -> Result<(), String> {
    match message.role.as_str() {
        "user" | "system" => Ok(()),
        "agent" => Err(
            "Renderer cannot append agent conversation messages; assistant results are backend-owned."
                .to_string(),
        ),
        _ => Err("Renderer conversation message role must be user or system.".to_string()),
    }
}

#[tauri::command]
async fn import_conversations(
    workspace_id: String,
    conversations: Vec<ConversationImportInput>,
    execution_store: State<'_, ExecutionStore>,
) -> Result<usize, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.import_conversations(&workspace_id, &conversations)
    })
    .await
    .map_err(|error| format!("Conversation import task failed: {error}"))?
}

#[tauri::command]
async fn prune_conversations(
    workspace_id: String,
    retained_ids: Vec<String>,
    execution_store: State<'_, ExecutionStore>,
) -> Result<usize, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.prune_conversations(&workspace_id, &retained_ids)
    })
    .await
    .map_err(|error| format!("Conversation retention task failed: {error}"))?
}

#[tauri::command]
async fn chat_ai_worker(
    mut config: AiWorkerConfig,
    mut request: AiWorkerChatRequest,
    ai_runs: State<'_, AiRunRegistry>,
    execution_store: State<'_, ExecutionStore>,
    workspace_root: State<'_, WorkspaceRoot>,
    workspace_trust: State<'_, WorkspaceTrustRegistry>,
    secrets: State<'_, AppSecretsStore>,
    on_event: Channel<AiRuntimeEvent>,
) -> Result<AiWorkerChatResult, String> {
    bind_tool_capable_ai_workspace(&mut config, &workspace_root, &workspace_trust)?;
    resolve_ai_secrets(&mut config, secrets.inner())?;
    let conversation_id = request
        .conversation_id
        .as_deref()
        .ok_or_else(|| "AI chat durable conversation ID is required.".to_string())?;
    let message_id = request
        .message_id
        .as_deref()
        .ok_or_else(|| "AI chat durable message ID is required.".to_string())?;
    let message_sequence = request
        .message_sequence
        .ok_or_else(|| "AI chat durable message sequence is required.".to_string())?;
    let chat_snapshot = execution_store.resolve_chat_snapshot(
        &config.workspace_id,
        conversation_id,
        message_id,
        message_sequence,
        &request.message,
    )?;
    let final_message = chat_snapshot.final_user_message()?;
    request.message = final_message.text.clone();
    request.terminal_context = None;
    request.session_context = Some(chat_snapshot.prior_model_context()?);
    request.context_revision = Some(format!(
        "{}:{}",
        chat_snapshot.revision, chat_snapshot.digest
    ));
    config.opencode_auto_approve = false;
    config.restrict_tools = true;
    config.fenced_tools_only = true;
    config.isolated_opencode_process = true;
    config.mcp_servers.clear();
    let run_id = request
        .run_id
        .clone()
        .ok_or_else(|| "AI chat run ID is required.".to_string())?;
    let run = ai_runs
        .get(&run_id)?
        .ok_or_else(|| "AI chat run was not registered.".to_string())?;
    ai_runs.start(&run.run_id)?;
    emit_ai_event(
        Some(&on_event),
        AiRuntimeEvent::RunStarted {
            run_id: run.run_id.clone(),
            sequence: 1,
        },
    );
    let event_sequence = Arc::new(AtomicU64::new(2));
    let cancellation = ai_runs.cancellation(&run.run_id)?;
    let run_id = run.run_id.clone();
    let runtime = ai_runs.inner().clone();
    let stream_channel = on_event.clone();
    let stream_run_id = run_id.clone();
    let stream_sequence = event_sequence.clone();
    let buffered_stream_events = Arc::new(Mutex::new(Vec::new()));
    let callback_stream_events = buffered_stream_events.clone();
    let stream_callback: Box<dyn FnMut(AiWorkerStreamEvent) -> Result<(), String> + Send> =
        Box::new(move |event| {
            callback_stream_events
                .lock()
                .map_err(|error| error.to_string())?
                .push(event);
            Ok(())
        });
    let result = if config.runtime == "api" {
        chat_ai_worker_streaming_impl(config, request, cancellation, stream_callback).await
    } else {
        match tauri::async_runtime::spawn_blocking(move || {
            chat_ai_worker_impl(config, request, cancellation, Some(stream_callback))
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = runtime.finish(&run_id, AiRunStatus::Failed);
                emit_ai_event(
                    Some(&on_event),
                    AiRuntimeEvent::RunFailed {
                        run_id: run_id.clone(),
                        sequence: event_sequence.fetch_add(1, Ordering::Relaxed),
                        error_code: "worker_join_failed".to_string(),
                    },
                );
                return Err(format!("Agent chat task failed: {error}"));
            }
        }
    };
    let result = result.and_then(|value| {
        persist_legacy_chat_assistant(
            execution_store.inner(),
            &chat_snapshot,
            &run_id,
            &value.message,
        )?;
        Ok(value)
    });
    if result.is_ok() {
        for event in buffered_stream_events
            .lock()
            .map_err(|error| error.to_string())?
            .drain(..)
        {
            emit_worker_stream_event(
                &stream_channel,
                &stream_run_id,
                &stream_sequence,
                event,
                None,
            );
        }
    }
    match result {
        Ok(mut value) => {
            if ai_runs
                .get(&run_id)?
                .is_some_and(|run| run.status == AiRunStatus::Cancelling)
            {
                let _ = runtime.finish(&run_id, AiRunStatus::Cancelled);
                emit_ai_event(
                    Some(&on_event),
                    AiRuntimeEvent::RunCancelled {
                        run_id: run_id.clone(),
                        sequence: event_sequence.fetch_add(1, Ordering::Relaxed),
                    },
                );
                return Err("AI chat run was cancelled.".to_string());
            }
            value.run_id = run_id.clone();
            let _ = runtime.finish(&run_id, AiRunStatus::Completed);
            emit_ai_event(
                Some(&on_event),
                AiRuntimeEvent::RunCompleted {
                    run_id: run_id.clone(),
                    sequence: event_sequence.fetch_add(1, Ordering::Relaxed),
                },
            );
            Ok(value)
        }
        Err(error) => {
            let status = if error.to_lowercase().contains("cancelled") {
                AiRunStatus::Cancelled
            } else {
                AiRunStatus::Failed
            };
            let _ = runtime.finish(&run_id, status);
            if status == AiRunStatus::Cancelled {
                emit_ai_event(
                    Some(&on_event),
                    AiRuntimeEvent::RunCancelled {
                        run_id: run_id.clone(),
                        sequence: event_sequence.fetch_add(1, Ordering::Relaxed),
                    },
                );
            } else {
                emit_ai_event(
                    Some(&on_event),
                    AiRuntimeEvent::RunFailed {
                        run_id: run_id.clone(),
                        sequence: event_sequence.fetch_add(1, Ordering::Relaxed),
                        error_code: "provider_failed".to_string(),
                    },
                );
            }
            Err(error)
        }
    }
}

fn persist_legacy_chat_assistant(
    execution_store: &ExecutionStore,
    snapshot: &infrastructure::execution_store::ChatConversationSnapshot,
    run_id: &str,
    response: &str,
) -> Result<(), String> {
    let action_suffix = regex::Regex::new(r"(?is)\n?SPACESLY_ACTIONS:\s*\[.*\]\s*$")
        .map_err(|error| format!("Failed to prepare Chat action filter: {error}"))?;
    let stripped = action_suffix.replace(response, "").trim().to_string();
    let assistant_text = if stripped.is_empty() {
        response.trim()
    } else {
        &stripped
    };
    execution_store.append_chat_assistant_if_current(
        snapshot,
        &format!("chat-run:{run_id}:assistant"),
        assistant_text,
    )?;
    Ok(())
}

fn emit_ai_event(channel: Option<&Channel<AiRuntimeEvent>>, event: AiRuntimeEvent) {
    if let Some(channel) = channel {
        let _ = channel.send(event);
    }
}

fn emit_worker_stream_event(
    channel: &Channel<AiRuntimeEvent>,
    run_id: &str,
    sequence: &AtomicU64,
    event: AiWorkerStreamEvent,
    audit_store: Option<&ExecutionStore>,
) {
    let sequence = sequence.fetch_add(1, Ordering::Relaxed);
    let runtime_event = match event {
        AiWorkerStreamEvent::TextDelta(delta) => AiRuntimeEvent::TextDelta {
            run_id: run_id.to_string(),
            sequence,
            delta,
        },
        AiWorkerStreamEvent::ToolStarted {
            tool_call_id,
            tool_name,
            risk,
            arguments_digest,
            display_context,
        } => {
            let tool_risk = ToolBroker::risk_for_tool(&tool_name);
            let operation_id = operation_id(
                run_id,
                &tool_call_id,
                &tool_name,
                tool_risk,
                &arguments_digest,
            );
            if let Some(store) = audit_store {
                let payload = serde_json::json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "risk": risk,
                    "operation_id": operation_id,
                    "arguments_digest": arguments_digest,
                    "display_context": display_context,
                });
                let _ = store.record_ai_audit(Some(run_id), "tool_started", &payload);
            }
            AiRuntimeEvent::ToolStarted {
                run_id: run_id.to_string(),
                sequence,
                tool_call_id,
                tool_name,
                risk,
                operation_id,
                arguments_digest,
                display_context,
            }
        }
        AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            risk,
            arguments_digest,
            display_context,
        } => {
            let operation_id = operation_id(
                run_id,
                &tool_call_id,
                &tool_name,
                ToolBroker::risk_for_tool(&tool_name),
                &arguments_digest,
            );
            if let Some(store) = audit_store {
                let payload = serde_json::json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "success": success,
                    "risk": risk,
                    "operation_id": operation_id,
                    "arguments_digest": arguments_digest,
                    "display_context": display_context,
                });
                let _ = store.record_ai_audit(Some(run_id), "tool_completed", &payload);
            }
            AiRuntimeEvent::ToolCompleted {
                run_id: run_id.to_string(),
                sequence,
                tool_call_id,
                tool_name,
                success,
                risk,
                operation_id,
                arguments_digest,
                display_context,
            }
        }
        AiWorkerStreamEvent::UsageUpdated {
            input_tokens,
            output_tokens,
        } => AiRuntimeEvent::UsageUpdated {
            run_id: run_id.to_string(),
            sequence,
            input_tokens,
            output_tokens,
        },
    };
    emit_ai_event(Some(channel), runtime_event);
}

fn bind_tool_capable_ai_workspace(
    config: &mut AiWorkerConfig,
    roots: &WorkspaceRoot,
    trust: &WorkspaceTrustRegistry,
) -> Result<(), String> {
    if config.runtime != "opencode" {
        return Ok(());
    }
    let workspace_id = config.workspace_id.trim();
    if workspace_id.is_empty() {
        return Err("AI workspace ID is required for OpenCode execution.".to_string());
    }
    let path = trust.require_trusted(roots, workspace_id)?;
    config.opencode_workdir = Some(path.to_string_lossy().to_string());
    Ok(())
}

#[tauri::command]
fn ai_workspace_trust_status(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
    workspace_trust: State<'_, WorkspaceTrustRegistry>,
) -> Result<WorkspaceTrustStatus, String> {
    workspace_trust.status(&workspace_root, &workspace_id)
}

#[tauri::command]
fn trust_ai_workspace(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
    workspace_trust: State<'_, WorkspaceTrustRegistry>,
) -> Result<WorkspaceTrustStatus, String> {
    workspace_trust.trust(&workspace_root, &workspace_id)
}

#[tauri::command]
async fn propose_ai_edit(
    config: AiWorkerConfig,
    request: AiEditRequest,
    ai_runs: State<'_, AiRunRegistry>,
    secrets: State<'_, AppSecretsStore>,
    on_event: Channel<AiRuntimeEvent>,
) -> Result<AiEditResult, String> {
    let mut config = config;
    resolve_ai_secrets(&mut config, secrets.inner())?;
    let run_id = request
        .run_id
        .clone()
        .ok_or_else(|| "AI edit run ID is required.".to_string())?;
    let run = ai_runs
        .get(&run_id)?
        .ok_or_else(|| "AI edit run was not registered.".to_string())?;
    ai_runs.start(&run.run_id)?;
    emit_ai_event(
        Some(&on_event),
        AiRuntimeEvent::RunStarted {
            run_id: run.run_id.clone(),
            sequence: 1,
        },
    );
    let cancellation = ai_runs.cancellation(&run.run_id)?;
    let run_id = run.run_id.clone();
    let runtime = ai_runs.inner().clone();
    let result = match tauri::async_runtime::spawn_blocking(move || {
        propose_ai_edit_impl(config, request, cancellation)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            let _ = runtime.finish(&run_id, AiRunStatus::Failed);
            emit_ai_event(
                Some(&on_event),
                AiRuntimeEvent::RunFailed {
                    run_id: run_id.clone(),
                    sequence: 2,
                    error_code: "edit_worker_join_failed".to_string(),
                },
            );
            return Err(format!("AI edit proposal task failed: {error}"));
        }
    };
    match result {
        Ok(mut value) => {
            if ai_runs
                .get(&run_id)?
                .is_some_and(|run| run.status == AiRunStatus::Cancelling)
            {
                let _ = runtime.finish(&run_id, AiRunStatus::Cancelled);
                emit_ai_event(
                    Some(&on_event),
                    AiRuntimeEvent::RunCancelled {
                        run_id: run_id.clone(),
                        sequence: 2,
                    },
                );
                return Err("AI edit run was cancelled.".to_string());
            }
            value.run_id = run_id.clone();
            let _ = runtime.finish(&run_id, AiRunStatus::Completed);
            emit_ai_event(
                Some(&on_event),
                AiRuntimeEvent::RunCompleted {
                    run_id: run_id.clone(),
                    sequence: 2,
                },
            );
            Ok(value)
        }
        Err(error) => {
            let status = if error.to_lowercase().contains("cancelled") {
                AiRunStatus::Cancelled
            } else {
                AiRunStatus::Failed
            };
            let _ = runtime.finish(&run_id, status);
            let event = if status == AiRunStatus::Cancelled {
                AiRuntimeEvent::RunCancelled {
                    run_id: run_id.clone(),
                    sequence: 2,
                }
            } else {
                AiRuntimeEvent::RunFailed {
                    run_id: run_id.clone(),
                    sequence: 2,
                    error_code: "edit_generation_failed".to_string(),
                }
            };
            emit_ai_event(Some(&on_event), event);
            Err(error)
        }
    }
}

fn resolve_ai_secrets(
    config: &mut AiWorkerConfig,
    secrets: &AppSecretsStore,
) -> Result<(), String> {
    if config.runtime == "api" {
        bind_backend_provider_profile(config)?;
    }
    let api_key = secrets.ai_api_key(&config.provider_id)?;
    if config.runtime == "api" && api_key.trim().is_empty() {
        return Err(format!(
            "No API key is configured for provider '{}'.",
            config.provider_id
        ));
    }
    config.api_key = api_key;
    for server in &mut config.mcp_servers {
        if server.secret_id.trim().is_empty() {
            return Err(format!(
                "MCP server '{}' has no secret profile ID.",
                server.name
            ));
        }
        let profile = secrets.mcp_connector(&server.secret_id)?;
        server.command = std::iter::once(profile.command)
            .chain(profile.args)
            .collect();
        server.environment = secrets.mcp_environment(&server.secret_id)?;
    }
    Ok(())
}

fn bind_backend_provider_profile(config: &mut AiWorkerConfig) -> Result<(), String> {
    let profile = provider_profile(&config.provider_id)
        .ok_or_else(|| "Unknown AI provider profile.".to_string())?;
    if !profile.models.contains(&config.model.as_str()) {
        return Err(format!(
            "Model '{}' is not allowed for provider '{}'.",
            config.model, config.provider_id
        ));
    }
    // Only allocate when the value differs — avoids 3 String allocs on every AI call
    // when the frontend already sent the correct provider name/url/style.
    if config.provider_name != profile.name {
        config.provider_name = profile.name.to_string();
    }
    if config.base_url != profile.base_url {
        config.base_url = profile.base_url.to_string();
    }
    let profile_api_style = profile.api_style.as_str();
    if config.api_style != profile_api_style {
        config.api_style = profile_api_style.to_string();
    }
    Ok(())
}

fn resolve_mcp_secret_environment(
    config: &mut McpServerConfig,
    secrets: &AppSecretsStore,
) -> Result<(), String> {
    let Some(secret_id) = config.secret_id.as_deref() else {
        return Ok(());
    };
    // The saved connector profile stores the canonical command + args for servers whose
    // credentials were persisted via saveMcpEnvironmentSecret.  However, the frontend also
    // sends the command and args directly in the config payload.  When no profile has been
    // saved yet (e.g. the server has no environment variables so the frontend never called
    // saveMcpEnvironmentSecret), we fall back to the command/args the frontend already
    // provided rather than returning a hard error.
    match secrets.mcp_connector(secret_id) {
        Ok(profile) => {
            config.command = profile.command;
            config.args = profile.args;
        }
        Err(_) if !config.command.trim().is_empty() => {
            // No saved profile, but the frontend supplied a command directly — proceed with it.
        }
        Err(error) => return Err(error),
    }
    config.env = secrets.mcp_environment(secret_id)?;
    Ok(())
}

fn resolve_jira_secrets(
    config: &mut JiraMcpConfig,
    secrets: &AppSecretsStore,
) -> Result<(), String> {
    if config.secret_id.trim().is_empty() {
        return Err("Jira secret ID is required.".to_string());
    }
    let (api_token, personal_access_token, password) = secrets.jira_credentials()?;
    let profile = secrets.jira_profile()?;
    config.auth.base_url = profile.base_url.clone();
    config.auth.auth_mode = profile.auth_mode;
    config.auth.username = profile.username;
    config.server.command = profile.command;
    config.server.args = profile.args;
    config.auth.api_token = api_token;
    config.auth.personal_access_token = personal_access_token;
    config.auth.password = password;
    let env = &mut config.server.env;
    env.insert("JIRA_URL".to_string(), config.auth.base_url.clone());
    env.insert("JIRA_BASE_URL".to_string(), config.auth.base_url.clone());
    env.insert(
        "ATLASSIAN_SITE_URL".to_string(),
        config.auth.base_url.clone(),
    );
    env.insert("JIRA_USERNAME".to_string(), config.auth.username.clone());
    env.insert("JIRA_EMAIL".to_string(), config.auth.username.clone());
    env.insert("ATLASSIAN_EMAIL".to_string(), config.auth.username.clone());
    match config.auth.auth_mode.as_str() {
        "pat" => {
            for key in [
                "JIRA_PAT",
                "JIRA_PERSONAL_ACCESS_TOKEN",
                "ATLASSIAN_PAT",
                "ATLASSIAN_PERSONAL_ACCESS_TOKEN",
            ] {
                env.insert(key.to_string(), config.auth.personal_access_token.clone());
            }
        }
        "password" => {
            for key in ["JIRA_PASSWORD", "ATLASSIAN_PASSWORD"] {
                env.insert(key.to_string(), config.auth.password.clone());
            }
        }
        _ => {
            for key in ["JIRA_API_TOKEN", "ATLASSIAN_API_TOKEN"] {
                env.insert(key.to_string(), config.auth.api_token.clone());
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn list_directory(
    workspace_id: String,
    relative_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<Vec<FileEntry>, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).list_directory(workspace_id, relative_path)
    })
    .await
    .map_err(|error| file_ipc_error("File listing task failed", error))?;
    result.map_err(|error| file_ipc_error("File listing failed", error))
}

#[tauri::command]
async fn read_file(
    workspace_id: String,
    relative_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<FileSnapshot, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).read_file(workspace_id, relative_path)
    })
    .await
    .map_err(|error| file_ipc_error("File read task failed", error))?;
    result.map_err(|error| file_ipc_error("File read failed", error))
}

#[tauri::command]
async fn search_workspace(
    request: WorkspaceSearchRequest,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<WorkspaceSearchResponse, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = root.path(&request.workspace_id)?;
        search_workspace_impl(
            &workspace_path,
            &request.query,
            request.case_sensitive,
            request.max_results,
        )
    })
    .await
    .map_err(|error| file_ipc_error("Workspace search task failed", error))?;
    result.map_err(|error| file_ipc_error("Workspace search failed", error))
}

#[tauri::command]
async fn preview_workspace_replace(
    request: WorkspaceReplacePreviewRequest,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<WorkspaceReplacePreviewResponse, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let workspace_path = root.path(&request.workspace_id)?;
        preview_workspace_replace_impl(
            &workspace_path,
            &request.query,
            &request.replacement,
            request.case_sensitive,
        )
    })
    .await
    .map_err(|error| file_ipc_error("Workspace replace preview task failed", error))?;
    result.map_err(|error| file_ipc_error("Workspace replace preview failed", error))
}

#[tauri::command]
async fn apply_workspace_replace(
    request: WorkspaceReplaceApplyRequest,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<WorkspaceReplaceApplyResponse, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<WorkspaceReplaceApplyResponse, String> {
            let workspace_id = request.workspace_id.clone();
            let workspace_path = root.path(&workspace_id)?;
            let response = apply_workspace_replace_impl(&workspace_path, request)?;
            let _ = invalidate_workspace_git_status(&root, workspace_id);
            Ok(response)
        },
    )
    .await
    .map_err(|error| file_ipc_error("Workspace replace apply task failed", error))?;
    result.map_err(|error| file_ipc_error("Workspace replace apply failed", error))
}

#[tauri::command]
async fn write_file(
    workspace_id: String,
    relative_path: String,
    content: String,
    expected_version: Option<String>,
    expected_root_revision: Option<u64>,
    encoding: TextEncoding,
    line_ending: LineEnding,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<FileWriteResult, String> {
    let root = workspace_root.inner().clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<FileWriteResult, String> {
            let result = FilesService::new(root.clone()).write_file(
                workspace_id.clone(),
                relative_path,
                content,
                expected_version,
                expected_root_revision,
                encoding,
                line_ending,
            )?;
            let _ = invalidate_workspace_git_status(&root, workspace_id);
            Ok(result)
        })
        .await
        .map_err(|error| file_ipc_error("File write task failed", error))?;
    result.map_err(|error| file_ipc_error("File write failed", error))
}

#[tauri::command]
async fn workspace_root_path(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).workspace_root_path(workspace_id)
    })
    .await
    .map_err(|error| file_ipc_error("Workspace root task failed", error))?;
    result.map_err(|error| file_ipc_error("Workspace root lookup failed", error))
}

#[tauri::command]
async fn workspace_root_revision(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<u64, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || root.revision(&workspace_id))
        .await
        .map_err(|error| file_ipc_error("Workspace root revision task failed", error))?
}

#[tauri::command]
async fn set_workspace_root(
    workspace_id: String,
    absolute_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).set_workspace_root(workspace_id, absolute_path)
    })
    .await
    .map_err(|error| file_ipc_error("Set workspace root task failed", error))?;
    result.map_err(|error| file_ipc_error("Set workspace root failed", error))
}

#[tauri::command]
fn watch_workspace_files(
    app: AppHandle,
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
    file_watchers: State<'_, FileWatchRegistry>,
) -> Result<(), String> {
    file_watchers
        .watch(app, workspace_root.inner(), workspace_id)
        .map_err(|error| file_ipc_error("Workspace file watch failed", error))
}

#[tauri::command]
fn unwatch_workspace_files(
    workspace_id: String,
    file_watchers: State<'_, FileWatchRegistry>,
) -> Result<bool, String> {
    file_watchers
        .unwatch(&workspace_id)
        .map_err(|error| file_ipc_error("Workspace file unwatch failed", error))
}

#[tauri::command]
async fn load_app_secrets(store: State<'_, AppSecretsStore>) -> Result<AppSecrets, String> {
    store.redacted_snapshot()
}

#[tauri::command]
fn ai_provider_secret_statuses(
    store: State<'_, AppSecretsStore>,
) -> Result<std::collections::HashMap<String, bool>, String> {
    store.ai_provider_statuses()
}

#[tauri::command]
fn mcp_environment_secret_statuses(
    store: State<'_, AppSecretsStore>,
) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    store.mcp_environment_statuses()
}

#[tauri::command]
fn jira_secret_statuses(
    store: State<'_, AppSecretsStore>,
) -> Result<std::collections::HashMap<String, bool>, String> {
    store.jira_secret_statuses()
}

#[tauri::command]
async fn save_jira_secret(
    secret_type: String,
    value: Option<String>,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.save_jira_secret(&secret_type, value.as_deref())
    })
    .await
    .map_err(|error| format!("Save Jira secret task failed: {error}"))?
}

#[tauri::command]
async fn save_mcp_environment_secret(
    server_id: String,
    command: String,
    args: Vec<String>,
    environment: Option<std::collections::HashMap<String, String>>,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.save_mcp_environment(&server_id, command, args, environment)
    })
    .await
    .map_err(|error| format!("Save MCP environment task failed: {error}"))?
}

#[tauri::command]
async fn remove_mcp_connector(
    server_id: String,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.remove_mcp_connector(&server_id))
        .await
        .map_err(|error| format!("Remove MCP connector task failed: {error}"))?
}

#[tauri::command]
async fn save_jira_connection_profile(
    profile: JiraConnectionProfile,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save_jira_profile(profile))
        .await
        .map_err(|error| format!("Save Jira profile task failed: {error}"))?
}

#[tauri::command]
async fn save_ai_provider_secret(
    provider_id: String,
    api_key: Option<String>,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.save_ai_api_key(&provider_id, api_key.as_deref())
    })
    .await
    .map_err(|error| format!("Save AI provider secret task failed: {error}"))?
}

#[tauri::command]
async fn save_app_secrets(
    secrets: AppSecrets,
    store: State<'_, AppSecretsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save_from_renderer(secrets))
        .await
        .map_err(|error| format!("Save secrets task failed: {error}"))?
}

#[tauri::command]
async fn list_global_environment_variables(
    store: State<'_, GlobalEnvironmentStore>,
) -> Result<Vec<infrastructure::global_environment::GlobalEnvironmentVariableView>, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list())
        .await
        .map_err(|error| format!("List global env vars task failed: {error}"))?
}

#[tauri::command]
async fn save_global_environment_variable(
    input: infrastructure::global_environment::GlobalEnvironmentVariableInput,
    store: State<'_, GlobalEnvironmentStore>,
) -> Result<infrastructure::global_environment::GlobalEnvironmentVariableView, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save(input))
        .await
        .map_err(|error| format!("Save global env var task failed: {error}"))?
}

#[tauri::command]
async fn delete_global_environment_variable(
    id: String,
    store: State<'_, GlobalEnvironmentStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.delete(&id))
        .await
        .map_err(|error| format!("Delete global env var task failed: {error}"))?
}

#[tauri::command]
async fn reveal_global_environment_variable(
    id: String,
    store: State<'_, GlobalEnvironmentStore>,
) -> Result<String, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.reveal(&id))
        .await
        .map_err(|error| format!("Reveal global env var task failed: {error}"))?
}

#[tauri::command]
async fn load_cached_workspace() -> Result<Option<CachedWorkspace>, String> {
    tauri::async_runtime::spawn_blocking(load_cached_workspace_impl)
        .await
        .map_err(|error| format!("Load workspace cache task failed: {error}"))?
}

#[tauri::command]
async fn save_cached_workspace(
    workspace: Workspace,
    deleted_card_ids: Option<Vec<String>>,
) -> Result<CachedWorkspace, String> {
    tauri::async_runtime::spawn_blocking(move || {
        save_cached_workspace_impl(workspace, deleted_card_ids.unwrap_or_default())
    })
    .await
    .map_err(|error| format!("Save workspace cache task failed: {error}"))?
}

#[tauri::command]
async fn save_execution_run(
    run: ExecutionRun,
    execution_store: State<'_, ExecutionStore>,
) -> Result<ExecutionRun, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save(&run))
        .await
        .map_err(|error| format!("Save execution run task failed: {error}"))?
}

#[tauri::command]
async fn list_active_execution_runs(
    execution_store: State<'_, ExecutionStore>,
) -> Result<Vec<ExecutionRun>, String> {
    let store = execution_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_active())
        .await
        .map_err(|error| format!("List execution runs task failed: {error}"))?
}

#[tauri::command]
async fn list_task_sessions(
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<Vec<TaskSessionSnapshot>, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_sessions())
        .await
        .map_err(|error| format!("List Task Sessions task failed: {error}"))?
}

#[tauri::command]
async fn list_agent_runtime_profiles(
    profile_store: State<'_, RuntimeProfileStore>,
) -> Result<Vec<AgentRuntimeProfile>, String> {
    let store = profile_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list())
        .await
        .map_err(|error| format!("List Agent runtime profiles task failed: {error}"))?
}

#[tauri::command]
async fn save_agent_runtime_profile(
    profile: AgentRuntimeProfile,
    profile_store: State<'_, RuntimeProfileStore>,
) -> Result<AgentRuntimeProfile, String> {
    let store = profile_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save(&profile))
        .await
        .map_err(|error| format!("Save Agent runtime profile task failed: {error}"))?
}

#[tauri::command]
async fn save_immutable_agent_runtime_profile(
    profile: AgentRuntimeProfile,
    profile_store: State<'_, RuntimeProfileStore>,
) -> Result<AgentRuntimeProfile, String> {
    let store = profile_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save_immutable(&profile))
        .await
        .map_err(|error| format!("Save immutable Agent runtime profile task failed: {error}"))?
}

#[tauri::command]
async fn submit_task_session(
    label: String,
    envelope: TaskSessionEnvelope,
    granted_capabilities: Vec<String>,
    execution_engine: State<'_, Arc<ExecutionEngine>>,
) -> Result<TaskSessionSnapshot, String> {
    match &envelope {
        TaskSessionEnvelope::V1(session) => session.validate_agent_runtime_ownership()?,
        TaskSessionEnvelope::V2(session) => session.validate()?,
    }
    let engine = execution_engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .submit_envelope_with_grants(
                label,
                &envelope,
                granted_capabilities,
                "renderer_user_approval",
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Submit Task Session task failed: {error}"))?
}

#[tauri::command]
fn digest_task_session_prompt_input(input: TaskSessionInputV2) -> Result<String, String> {
    prompt_input_digest(&input)
}

#[tauri::command]
fn digest_agent_execution_contract(contract: serde_json::Value) -> Result<String, String> {
    execution_contract_digest(&contract)
}

#[tauri::command]
async fn cancel_task_session(
    session_id: u64,
    execution_engine: State<'_, Arc<ExecutionEngine>>,
) -> Result<bool, String> {
    let engine = execution_engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .cancel(TaskSessionId(session_id))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Cancel Task Session task failed: {error}"))?
}

/// Returns scheduler health from shared memory without waiting on the scheduler command channel.
#[tauri::command]
fn get_scheduler_health(execution_engine: State<'_, Arc<ExecutionEngine>>) -> SchedulerHealth {
    execution_engine.health()
}

#[tauri::command]
async fn remove_task_session(
    session_id: u64,
    execution_engine: State<'_, Arc<ExecutionEngine>>,
) -> Result<bool, String> {
    let engine = execution_engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .remove_session(TaskSessionId(session_id))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Remove Task Session task failed: {error}"))?
}

#[tauri::command]
async fn get_task_session(
    session_id: u64,
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<Option<TaskSessionSnapshot>, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.get_session(TaskSessionId(session_id)))
        .await
        .map_err(|error| format!("Get Task Session task failed: {error}"))?
}

/// Returns the durable authoritative Agent result once staging has completed, including while the
/// session is still committing its executions.db projection.
#[tauri::command]
async fn get_task_session_result(
    session_id: u64,
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<Option<TaskSessionResult>, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.task_session_result(TaskSessionId(session_id))
    })
    .await
    .map_err(|error| format!("Get Task Session result task failed: {error}"))?
}

#[tauri::command]
async fn list_task_session_events(
    session_id: u64,
    after_sequence: u64,
    limit: Option<usize>,
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<TaskSessionEventPage, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let session_id = TaskSessionId(session_id);
        store.event_page(session_id, after_sequence, limit.unwrap_or(100))
    })
    .await
    .map_err(|error| format!("List Task Session events task failed: {error}"))?
}

#[tauri::command]
async fn get_task_session_tool_state(
    session_id: u64,
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<TaskToolState, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.tool_state(TaskSessionId(session_id)))
        .await
        .map_err(|error| format!("Get Task Session tool state task failed: {error}"))?
}

#[tauri::command]
async fn get_task_session_mcp_context(
    session_id: u64,
    scheduler_store: State<'_, SchedulerStore>,
) -> Result<TaskMcpContext, String> {
    let store = scheduler_store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.mcp_context(TaskSessionId(session_id)))
        .await
        .map_err(|error| format!("Get Task Session MCP context task failed: {error}"))?
}

#[tauri::command]
async fn format_code(formatter: String, source: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || format_code_impl(formatter, source))
        .await
        .map_err(|error| format!("Format task failed: {error}"))?
}

#[tauri::command]
async fn get_workspace_git_info(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).workspace_git_info(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Workspace git info task failed: {error}"))?
}

#[tauri::command]
async fn get_path_git_info(path: String) -> Result<GitWorkspaceInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::PathBuf::from(path);
        git_info_for_path(&path)
    })
    .await
    .map_err(|error| format!("Path git info task failed: {error}"))?
}

#[tauri::command]
async fn get_workspace_git_status(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitStatus, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).status(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Workspace git status task failed: {error}"))?
}

#[tauri::command]
async fn stage_workspace_git_file(
    workspace_id: Option<String>,
    path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitStatus, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).stage_file(workspace_id_or_default(workspace_id), path)
    })
    .await
    .map_err(|error| format!("Stage git file task failed: {error}"))?
}

#[tauri::command]
async fn stage_all_workspace_git_files(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitStatus, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).stage_all(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Stage all git files task failed: {error}"))?
}

#[tauri::command]
async fn unstage_workspace_git_file(
    workspace_id: Option<String>,
    path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitStatus, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).unstage_file(workspace_id_or_default(workspace_id), path)
    })
    .await
    .map_err(|error| format!("Unstage git file task failed: {error}"))?
}

#[tauri::command]
async fn unstage_all_workspace_git_files(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitStatus, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).unstage_all(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Unstage all git files task failed: {error}"))?
}

#[tauri::command]
async fn checkout_workspace_git_branch(
    workspace_id: Option<String>,
    branch: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).checkout_branch(workspace_id_or_default(workspace_id), branch)
    })
    .await
    .map_err(|error| format!("Checkout branch task failed: {error}"))?
}

#[tauri::command]
async fn pull_workspace_git_changes(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).pull_changes(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Git pull task failed: {error}"))?
}

#[tauri::command]
async fn commit_workspace_git_changes(
    workspace_id: Option<String>,
    message: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<CommitResult, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).commit_changes(workspace_id_or_default(workspace_id), message)
    })
    .await
    .map_err(|error| format!("Git commit task failed: {error}"))?
}

#[tauri::command]
async fn push_workspace_git_changes(
    workspace_id: Option<String>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).push_changes(workspace_id_or_default(workspace_id))
    })
    .await
    .map_err(|error| format!("Git push task failed: {error}"))?
}

#[tauri::command]
async fn merge_workspace_git_branch(
    workspace_id: Option<String>,
    branch: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).merge_branch(workspace_id_or_default(workspace_id), branch)
    })
    .await
    .map_err(|error| format!("Git merge task failed: {error}"))?
}

#[tauri::command]
async fn rebase_workspace_git_branch(
    workspace_id: Option<String>,
    branch: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        GitService::new(root).rebase_branch(workspace_id_or_default(workspace_id), branch)
    })
    .await
    .map_err(|error| format!("Git rebase task failed: {error}"))?
}

#[tauri::command]
async fn run_shell_command(request: ShellCommandRequest) -> Result<ShellCommandResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_shell_command_impl(request))
        .await
        .map_err(|error| format!("Shell command task failed: {error}"))?
}

#[tauri::command]
fn open_pty_terminal(
    workspace_id: Option<String>,
    terminal_id: String,
    workdir: Option<String>,
    on_data: Channel<Vec<u8>>,
    state: State<'_, PtyState>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<(), String> {
    open_pty_terminal_impl(
        &state,
        &workspace_root,
        workspace_id,
        terminal_id,
        workdir,
        on_data,
    )
}

#[tauri::command]
fn close_pty_terminal(terminal_id: String, state: State<'_, PtyState>) -> Result<(), String> {
    close_pty_terminal_impl(&state, terminal_id)
}

#[tauri::command]
fn write_pty_terminal(
    terminal_id: String,
    data: Vec<u8>,
    state: State<'_, PtyState>,
) -> Result<(), String> {
    write_pty_terminal_impl(&state, terminal_id, data)
}

#[tauri::command]
fn resize_pty_terminal(
    terminal_id: String,
    rows: u16,
    cols: u16,
    state: State<'_, PtyState>,
) -> Result<(), String> {
    resize_pty_terminal_impl(&state, terminal_id, rows, cols)
}

#[tauri::command]
fn pty_current_directory(
    terminal_id: String,
    state: State<'_, PtyState>,
) -> Result<Option<String>, String> {
    pty_current_directory_impl(&state, terminal_id)
}

#[tauri::command]
async fn complete_shell_input(
    request: ShellCompletionRequest,
) -> Result<ShellCompletionResult, String> {
    tauri::async_runtime::spawn_blocking(move || complete_shell_input_impl(request))
        .await
        .map_err(|error| format!("Shell completion task failed: {error}"))?
}

#[tauri::command]
async fn lsp_start_server(
    workspace_id: String,
    config: LspServerConfig,
    workspace_root: State<'_, WorkspaceRoot>,
    lsp: State<'_, LspRegistry>,
) -> Result<LspServerStatus, String> {
    let roots = workspace_root.inner().clone();
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || registry.start(&roots, workspace_id, config))
        .await
        .map_err(|error| file_ipc_error("LSP start task failed", error))?
        .map_err(|error| file_ipc_error("LSP start failed", error))
}

#[tauri::command]
async fn lsp_stop_server(
    workspace_id: String,
    server_id: String,
    lsp: State<'_, LspRegistry>,
) -> Result<bool, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || registry.stop(&workspace_id, &server_id))
        .await
        .map_err(|error| file_ipc_error("LSP stop task failed", error))?
        .map_err(|error| file_ipc_error("LSP stop failed", error))
}

#[tauri::command]
fn lsp_get_status(lsp: State<'_, LspRegistry>) -> Result<Vec<LspServerStatus>, String> {
    lsp.statuses()
        .map_err(|error| file_ipc_error("LSP status failed", error))
}

#[tauri::command]
async fn lsp_sync_document(
    workspace_id: String,
    server_id: String,
    file_path: String,
    language_id: String,
    version: i64,
    text: String,
    lsp: State<'_, LspRegistry>,
) -> Result<(), String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.sync_document(
            &workspace_id,
            &server_id,
            &file_path,
            &language_id,
            version,
            text,
        )
    })
    .await
    .map_err(|error| file_ipc_error("LSP document sync task failed", error))?
    .map_err(|error| file_ipc_error("LSP document sync failed", error))
}

#[tauri::command]
fn lsp_close_document(
    workspace_id: String,
    server_id: String,
    file_path: String,
    lsp: State<'_, LspRegistry>,
) -> Result<(), String> {
    lsp.close_document(&workspace_id, &server_id, &file_path)
        .map_err(|error| file_ipc_error("LSP document close failed", error))
}

#[tauri::command]
fn lsp_diagnostics(
    workspace_id: String,
    server_id: String,
    file_path: String,
    lsp: State<'_, LspRegistry>,
) -> Result<LspDiagnosticReport, String> {
    lsp.diagnostics(&workspace_id, &server_id, &file_path)
        .map_err(|error| file_ipc_error("LSP diagnostics failed", error))
}

#[tauri::command]
async fn lsp_hover(
    workspace_id: String,
    server_id: String,
    file_path: String,
    position: LspPosition,
    lsp: State<'_, LspRegistry>,
) -> Result<Option<LspHoverResult>, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.hover(&workspace_id, &server_id, &file_path, position)
    })
    .await
    .map_err(|error| file_ipc_error("LSP hover task failed", error))?
    .map_err(|error| file_ipc_error("LSP hover failed", error))
}

#[tauri::command]
async fn lsp_goto_definition(
    workspace_id: String,
    server_id: String,
    file_path: String,
    position: LspPosition,
    lsp: State<'_, LspRegistry>,
) -> Result<Option<LspLocation>, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.definition(&workspace_id, &server_id, &file_path, position)
    })
    .await
    .map_err(|error| file_ipc_error("LSP definition task failed", error))?
    .map_err(|error| file_ipc_error("LSP definition failed", error))
}

#[tauri::command]
async fn lsp_references(
    workspace_id: String,
    server_id: String,
    file_path: String,
    position: LspPosition,
    lsp: State<'_, LspRegistry>,
) -> Result<Vec<LspLocation>, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.references(&workspace_id, &server_id, &file_path, position)
    })
    .await
    .map_err(|error| file_ipc_error("LSP references task failed", error))?
    .map_err(|error| file_ipc_error("LSP references failed", error))
}

#[tauri::command]
async fn lsp_document_symbols(
    workspace_id: String,
    server_id: String,
    file_path: String,
    lsp: State<'_, LspRegistry>,
) -> Result<Vec<LspDocumentSymbol>, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.document_symbols(&workspace_id, &server_id, &file_path)
    })
    .await
    .map_err(|error| file_ipc_error("LSP document symbols task failed", error))?
    .map_err(|error| file_ipc_error("LSP document symbols failed", error))
}

#[tauri::command]
async fn lsp_completion(
    workspace_id: String,
    server_id: String,
    request: LspCompletionRequest,
    lsp: State<'_, LspRegistry>,
) -> Result<LspCompletionResult, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.completion(&workspace_id, &server_id, request)
    })
    .await
    .map_err(|error| file_ipc_error("LSP completion task failed", error))?
    .map_err(|error| file_ipc_error("LSP completion failed", error))
}

#[tauri::command]
async fn lsp_code_actions(
    workspace_id: String,
    server_id: String,
    request: LspCodeActionRequest,
    lsp: State<'_, LspRegistry>,
) -> Result<Vec<LspCodeAction>, String> {
    let registry = lsp.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        registry.code_actions(&workspace_id, &server_id, request)
    })
    .await
    .map_err(|error| file_ipc_error("LSP code action task failed", error))?
    .map_err(|error| file_ipc_error("LSP code action failed", error))
}

#[cfg(test)]
mod tests {
    use super::{
        bind_backend_provider_profile, file_ipc_error, mcp_ipc_error,
        validate_renderer_conversation_role,
    };
    use crate::infrastructure::ai_worker::AiWorkerConfig;
    use crate::infrastructure::execution_store::ConversationMessageInput;
    use serde_json::Value;

    #[test]
    fn mcp_ipc_errors_include_category_and_retryability() {
        let value: Value =
            serde_json::from_str(&mcp_ipc_error("MCP test failed", "request timed out"))
                .expect("structured MCP error");

        assert_eq!(value["category"], "timeout");
        assert_eq!(value["retryable"], true);
        assert!(value["message"]
            .as_str()
            .unwrap()
            .contains("MCP test failed"));
    }

    #[test]
    fn file_ipc_errors_preserve_actionable_categories() {
        let conflict: Value = serde_json::from_str(&file_ipc_error(
            "File write failed",
            "File changed on disk after it was opened.",
        ))
        .expect("structured file conflict");
        let encoding: Value = serde_json::from_str(&file_ipc_error(
            "File read failed",
            "File is not valid UTF-8.",
        ))
        .expect("structured encoding error");

        assert_eq!(conflict["category"], "conflict");
        assert_eq!(conflict["retryable"], false);
        assert_eq!(encoding["category"], "encoding");
    }

    #[test]
    fn provider_profile_overrides_renderer_controlled_destination() {
        let mut config = AiWorkerConfig {
            workspace_id: "workspace-personal".to_string(),
            runtime: "api".to_string(),
            provider_name: "Attacker".to_string(),
            provider_id: "openai".to_string(),
            base_url: "https://attacker.invalid".to_string(),
            api_style: "anthropic_messages".to_string(),
            api_key: String::new(),
            model: "gpt-4.1-mini".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_model: "openai/gpt-4.1-mini".to_string(),
            opencode_workdir: None,
            opencode_auto_approve: false,
            agent_rules: String::new(),
            agent_skills: String::new(),
            temperature: 0.2,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: Vec::new(),
        };

        bind_backend_provider_profile(&mut config).unwrap();

        assert_eq!(config.provider_name, "OpenAI");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_style, "openai_responses");
    }

    #[test]
    fn provider_profile_rejects_unknown_models() {
        let mut config = AiWorkerConfig {
            workspace_id: String::new(),
            runtime: "api".to_string(),
            provider_name: String::new(),
            provider_id: "openai".to_string(),
            base_url: String::new(),
            api_style: String::new(),
            api_key: String::new(),
            model: "other-provider-model".to_string(),
            opencode_command: String::new(),
            opencode_model: String::new(),
            opencode_workdir: None,
            opencode_auto_approve: false,
            agent_rules: String::new(),
            agent_skills: String::new(),
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: Vec::new(),
        };

        assert!(bind_backend_provider_profile(&mut config).is_err());
    }

    #[test]
    fn renderer_cannot_append_agent_conversation_messages() {
        let error = validate_renderer_conversation_role(&ConversationMessageInput {
            id: "assistant-1".to_string(),
            role: "agent".to_string(),
            text: "Backend result".to_string(),
        })
        .unwrap_err();

        assert!(error.contains("backend-owned"));
        for role in ["user", "system"] {
            validate_renderer_conversation_role(&ConversationMessageInput {
                id: format!("{role}-1"),
                role: role.to_string(),
                text: "Allowed".to_string(),
            })
            .unwrap();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pty_state: PtyState = Arc::new(Mutex::new(PtyRegistry::new()));
    let shutdown_state = pty_state.clone();
    let workspace_root = WorkspaceRoot::home().expect("failed to initialize workspace root");

    // Initialize the most expensive startup resources in parallel:
    //   - ExecutionStore: opens SQLite + runs schema DDL + recover_interrupted_runs()
    //   - RecoveryStore:  opens SQLite + runs schema DDL + prune_expired()
    //   - AppSecretsStore: reads secrets.json + keyring round-trips
    //   - GlobalEnvironmentStore: reads managed process environment variables
    //
    // Each opens a different file so there is no contention. Running them
    // concurrently reduces the serial startup chain from ~3×T to ~max(T).
    let execution_handle = std::thread::spawn(|| {
        ExecutionStore::open().expect("failed to initialize execution store")
    });
    let recovery_handle =
        std::thread::spawn(|| RecoveryStore::open().expect("failed to initialize recovery store"));
    let secrets_handle =
        std::thread::spawn(|| AppSecretsStore::load().expect("failed to initialize app secrets"));
    let global_environment_handle = std::thread::spawn(|| {
        GlobalEnvironmentStore::global().expect("failed to initialize global environment")
    });
    let scheduler_store_handle = std::thread::spawn(|| {
        SchedulerStore::open_query().expect("failed to initialize Task Session query store")
    });
    let runtime_profile_handle = std::thread::spawn(|| {
        RuntimeProfileStore::open().expect("failed to initialize Agent runtime profiles")
    });

    let execution_store = execution_handle
        .join()
        .expect("execution store init panicked");
    let recovery_store = recovery_handle
        .join()
        .expect("recovery store init panicked");
    let app_secrets = secrets_handle.join().expect("app secrets init panicked");
    let global_environment = global_environment_handle
        .join()
        .expect("global environment init panicked");
    let scheduler_store = scheduler_store_handle
        .join()
        .expect("Task Session store init panicked");
    let runtime_profile_store = runtime_profile_handle
        .join()
        .expect("Agent runtime profile store init panicked");
    let ai_run_registry = AiRunRegistry::default();
    let workspace_trust = WorkspaceTrustRegistry::default();
    let runtime_resolver = Arc::new(StoredAgentRuntimeResolver::new(
        runtime_profile_store.clone(),
        execution_store.clone(),
        app_secrets.clone(),
        workspace_root.clone(),
        workspace_trust.clone(),
    ));
    let agent_executor = Arc::new(AgentTaskExecutor::new(
        runtime_resolver.clone(),
        Arc::new(AiWorkerRuntimeRunner),
    ));
    let prompt_executor = Arc::new(PromptTaskExecutor::new(
        runtime_resolver,
        Arc::new(AiWorkerPromptRuntimeRunner),
    ));
    let task_executor = TaskSessionExecutor::new(agent_executor, prompt_executor);
    let execution_engine = Arc::new(
        ExecutionEngine::open_persistent_with_executor_and_projector(
            Arc::new(task_executor),
            Arc::new(execution_store.clone()),
        )
        .expect("failed to initialize Task Session execution engine"),
    );
    let task_session_updates = execution_engine.subscribe_updates();
    let lsp_registry = LspRegistry::default();
    let shutdown_lsp = lsp_registry.clone();
    tauri::Builder::default()
        .manage(pty_state)
        .manage(workspace_root)
        .manage(execution_store)
        .manage(recovery_store)
        .manage(ai_run_registry)
        .manage(workspace_trust)
        .manage(FileWatchRegistry::default())
        .manage(lsp_registry)
        .manage(app_secrets)
        .manage(global_environment)
        .manage(scheduler_store)
        .manage(runtime_profile_store)
        .manage(execution_engine)
        .manage(AppState::new())
        .manage(AgentRunRegistry::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let app_handle = app.handle().clone();
            std::thread::Builder::new()
                .name("spacesly-task-session-events".to_string())
                .spawn(move || {
                    while let Ok(update) = task_session_updates.recv() {
                        let _ =
                            app_handle.emit::<TaskSessionUpdate>(TASK_SESSION_UPDATE_EVENT, update);
                    }
                })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_workspace,
            get_jira_issues,
            get_jira_boards,
            test_jira_mcp_connection,
            test_mcp_server_connection,
            disconnect_mcp_server,
            sync_jira_workspace,
            transition_jira_issue,
            assign_jira_issue,
            add_jira_comment,
            test_ai_worker,
            reserve_ai_worker_run,
            get_ai_run,
            grant_ai_run_capabilities,
            begin_ai_run,
            ai_workspace_trust_status,
            trust_ai_workspace,
            execute_ai_worker_task,
            release_ai_worker_run,
            cancel_ai_worker_task,
            cancel_ai_run,
            list_conversations,
            load_conversation_messages,
            append_conversation_message,
            import_conversations,
            prune_conversations,
            chat_ai_worker,
            propose_ai_edit,
            sync_recovery_snapshots,
            list_recovery_snapshots,
            delete_recovery_snapshot,
            list_directory,
            read_file,
            search_workspace,
            preview_workspace_replace,
            apply_workspace_replace,
            write_file,
            workspace_root_path,
            workspace_root_revision,
            set_workspace_root,
            watch_workspace_files,
            unwatch_workspace_files,
            load_app_secrets,
            save_app_secrets,
            ai_provider_secret_statuses,
            save_ai_provider_secret,
            mcp_environment_secret_statuses,
            save_mcp_environment_secret,
            remove_mcp_connector,
            save_jira_connection_profile,
            jira_secret_statuses,
            save_jira_secret,
            list_global_environment_variables,
            save_global_environment_variable,
            delete_global_environment_variable,
            reveal_global_environment_variable,
            load_cached_workspace,
            save_cached_workspace,
            save_execution_run,
            list_active_execution_runs,
            list_task_sessions,
            get_scheduler_health,
            get_task_session,
            get_task_session_result,
            list_task_session_events,
            get_task_session_tool_state,
            get_task_session_mcp_context,
            list_agent_runtime_profiles,
            save_agent_runtime_profile,
            save_immutable_agent_runtime_profile,
            submit_task_session,
            digest_task_session_prompt_input,
            digest_agent_execution_contract,
            cancel_task_session,
            remove_task_session,
            format_code,
            get_workspace_git_info,
            get_path_git_info,
            get_workspace_git_status,
            stage_workspace_git_file,
            stage_all_workspace_git_files,
            unstage_workspace_git_file,
            unstage_all_workspace_git_files,
            checkout_workspace_git_branch,
            pull_workspace_git_changes,
            commit_workspace_git_changes,
            push_workspace_git_changes,
            merge_workspace_git_branch,
            rebase_workspace_git_branch,
            run_shell_command,
            complete_shell_input,
            open_pty_terminal,
            close_pty_terminal,
            write_pty_terminal,
            resize_pty_terminal,
            pty_current_directory,
            lsp_start_server,
            lsp_stop_server,
            lsp_get_status,
            lsp_sync_document,
            lsp_close_document,
            lsp_diagnostics,
            lsp_hover,
            lsp_goto_definition,
            lsp_references,
            lsp_document_symbols,
            lsp_completion,
            lsp_code_actions
        ])
        .on_window_event(move |_window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                close_all_terminals(&shutdown_state);
                close_all_mcp_sessions();
                close_all_opencode_servers();
                shutdown_lsp.stop_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
