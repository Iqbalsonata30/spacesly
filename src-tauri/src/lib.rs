mod application;
mod domain;
mod infrastructure;

use application::app::AppState;
use application::files_service::FilesService;
use application::git_service::GitService;
use application::jira_service::JiraService;
use domain::entity::Workspace;
use domain::execution::ExecutionRun;
use infrastructure::ai_worker::{
    chat_ai_worker as chat_ai_worker_impl, execute_ai_worker_task as execute_ai_worker_task_impl,
    test_ai_worker as test_ai_worker_impl, AgentRunRegistry, AiWorkerChatRequest,
    AiWorkerChatResult, AiWorkerConfig, AiWorkerStatus, AiWorkerTask, AiWorkerTaskResult,
};
use infrastructure::execution_store::ExecutionStore;
use infrastructure::files::{FileEntry, WorkspaceRoot};
use infrastructure::formatting::format_code as format_code_impl;
use infrastructure::git::git_info_for_path;
use infrastructure::git::{
    invalidate_workspace_git_status, CommitResult, GitStatus, GitWorkspaceInfo,
};
use infrastructure::mcp::{
    close_all_mcp_sessions, JiraBoard, JiraConnectionStatus, JiraIssue, JiraMcpConfig,
    McpConnectionStatus, McpServerConfig,
};
use infrastructure::pty::{
    close_all_terminals, close_pty_terminal as close_pty_terminal_impl,
    open_pty_terminal as open_pty_terminal_impl,
    pty_current_directory as pty_current_directory_impl,
    resize_pty_terminal as resize_pty_terminal_impl, write_pty_terminal as write_pty_terminal_impl,
    PtyRegistry, PtyState,
};
use infrastructure::secrets::{
    load_app_secrets as load_app_secrets_impl, save_app_secrets as save_app_secrets_impl,
    AppSecrets,
};
use infrastructure::shell::{
    complete_shell_input as complete_shell_input_impl, run_shell_command as run_shell_command_impl,
    ShellCommandRequest, ShellCommandResult, ShellCompletionRequest, ShellCompletionResult,
};
use infrastructure::workspace_cache::{
    load_cached_workspace as load_cached_workspace_impl,
    save_cached_workspace as save_cached_workspace_impl, CachedWorkspace,
};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::State;

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

#[tauri::command]
fn get_workspace() -> Workspace {
    AppState::new().workspace()
}

#[tauri::command]
async fn get_jira_issues(config: JiraMcpConfig) -> Result<Vec<JiraIssue>, String> {
    let result = tauri::async_runtime::spawn_blocking(move || JiraService::new().issues(config))
        .await
        .map_err(|error| mcp_ipc_error("Jira issue task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira issue request failed", error))
}

#[tauri::command]
async fn get_jira_boards(config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
    let result = tauri::async_runtime::spawn_blocking(move || JiraService::new().boards(config))
        .await
        .map_err(|error| mcp_ipc_error("Jira board task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira board request failed", error))
}

#[tauri::command]
async fn test_jira_mcp_connection(config: JiraMcpConfig) -> Result<JiraConnectionStatus, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().test_jira_connection(config)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira MCP test task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira MCP test failed", error))
}

#[tauri::command]
async fn test_mcp_server_connection(
    config: McpServerConfig,
) -> Result<McpConnectionStatus, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().test_mcp_connection(config)
    })
    .await
    .map_err(|error| mcp_ipc_error("MCP test task failed", error))?;
    result.map_err(|error| mcp_ipc_error("MCP test failed", error))
}

#[tauri::command]
async fn disconnect_mcp_server(config: McpServerConfig) -> Result<bool, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().disconnect_mcp_server(config)
    })
    .await
    .map_err(|error| mcp_ipc_error("MCP disconnect task failed", error))?;
    result.map_err(|error| mcp_ipc_error("MCP disconnect failed", error))
}

#[tauri::command]
async fn sync_jira_workspace(config: JiraMcpConfig) -> Result<Workspace, String> {
    let result =
        tauri::async_runtime::spawn_blocking(move || JiraService::new().sync_workspace(config))
            .await
            .map_err(|error| mcp_ipc_error("Jira sync task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira sync failed", error))
}

#[tauri::command]
async fn transition_jira_issue(
    config: JiraMcpConfig,
    issue_key: String,
    target_status: String,
) -> Result<(), String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().transition_issue(config, issue_key, target_status)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira transition task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira transition failed", error))
}

#[tauri::command]
async fn assign_jira_issue(config: JiraMcpConfig, issue_key: String) -> Result<(), String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().assign_issue(config, issue_key)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira assign task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira assign failed", error))
}

#[tauri::command]
async fn add_jira_comment(
    config: JiraMcpConfig,
    issue_key: String,
    comment: String,
) -> Result<(), String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().add_comment(config, issue_key, comment)
    })
    .await
    .map_err(|error| mcp_ipc_error("Jira comment task failed", error))?;
    result.map_err(|error| mcp_ipc_error("Jira comment failed", error))
}

#[tauri::command]
async fn test_ai_worker(config: AiWorkerConfig) -> Result<AiWorkerStatus, String> {
    tauri::async_runtime::spawn_blocking(move || test_ai_worker_impl(config))
        .await
        .map_err(|error| format!("Agent diagnostic task failed: {error}"))?
}

#[tauri::command]
fn reserve_ai_worker_run(
    run_id: String,
    config: AiWorkerConfig,
    agent_runs: State<'_, AgentRunRegistry>,
) -> Result<(), String> {
    agent_runs.reserve(&run_id, &config)
}

#[tauri::command]
async fn execute_ai_worker_task(
    run_id: String,
    config: AiWorkerConfig,
    task: AiWorkerTask,
    agent_runs: State<'_, AgentRunRegistry>,
    execution_store: State<'_, ExecutionStore>,
) -> Result<AiWorkerTaskResult, String> {
    let registry = agent_runs.inner().clone();
    let cancellation = registry.start(&run_id)?;
    let store = execution_store.inner().clone();
    if let Err(error) = store.claim_step(&run_id, "worker.execute", &run_id, 15 * 60 * 1000) {
        let _ = registry.finish(&run_id);
        return Err(error);
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = execute_ai_worker_task_impl(config, task, cancellation);
        let (status, summary) = match &result {
            Ok(value)
                if value.completion_status
                    == infrastructure::ai_worker::AiWorkerCompletionStatus::Completed =>
            {
                ("completed", Some(value.summary.as_str()))
            }
            Ok(value) => ("blocked", Some(value.summary.as_str())),
            Err(error) => ("failed", Some(error.as_str())),
        };
        let _ = store.finish_step(&run_id, "worker.execute", &run_id, status, summary);
        let _ = registry.finish(&run_id);
        result
    })
    .await
    .map_err(|error| format!("Agent execution task failed: {error}"))?;
    result
}

#[tauri::command]
fn release_ai_worker_run(
    run_id: String,
    agent_runs: State<'_, AgentRunRegistry>,
) -> Result<bool, String> {
    agent_runs.release_reservation(&run_id)
}

#[tauri::command]
fn cancel_ai_worker_task(
    run_id: String,
    agent_runs: State<'_, AgentRunRegistry>,
) -> Result<bool, String> {
    agent_runs.cancel(&run_id)
}

#[tauri::command]
async fn chat_ai_worker(
    config: AiWorkerConfig,
    request: AiWorkerChatRequest,
) -> Result<AiWorkerChatResult, String> {
    tauri::async_runtime::spawn_blocking(move || chat_ai_worker_impl(config, request))
        .await
        .map_err(|error| format!("Agent chat task failed: {error}"))?
}

#[tauri::command]
async fn list_directory(
    workspace_id: String,
    relative_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<Vec<FileEntry>, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).list_directory(workspace_id, relative_path)
    })
    .await
    .map_err(|error| format!("File listing task failed: {error}"))?
}

#[tauri::command]
async fn read_file(
    workspace_id: String,
    relative_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).read_file(workspace_id, relative_path)
    })
    .await
    .map_err(|error| format!("File read task failed: {error}"))?
}

#[tauri::command]
async fn write_file(
    workspace_id: String,
    relative_path: String,
    content: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<(), String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root.clone()).write_file(workspace_id.clone(), relative_path, content)?;
        let _ = invalidate_workspace_git_status(&root, workspace_id);
        Ok(())
    })
    .await
    .map_err(|error| format!("File write task failed: {error}"))?
}

#[tauri::command]
async fn workspace_root_path(
    workspace_id: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).workspace_root_path(workspace_id)
    })
    .await
    .map_err(|error| format!("Workspace root task failed: {error}"))?
}

#[tauri::command]
async fn set_workspace_root(
    workspace_id: String,
    absolute_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).set_workspace_root(workspace_id, absolute_path)
    })
    .await
    .map_err(|error| format!("Set workspace root task failed: {error}"))?
}

#[tauri::command]
async fn load_app_secrets() -> Result<AppSecrets, String> {
    tauri::async_runtime::spawn_blocking(load_app_secrets_impl)
        .await
        .map_err(|error| format!("Load secrets task failed: {error}"))?
}

#[tauri::command]
async fn save_app_secrets(secrets: AppSecrets) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || save_app_secrets_impl(secrets))
        .await
        .map_err(|error| format!("Save secrets task failed: {error}"))?
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

#[cfg(test)]
mod tests {
    use super::mcp_ipc_error;
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
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pty_state: PtyState = Arc::new(Mutex::new(PtyRegistry::new()));
    let shutdown_state = pty_state.clone();
    let workspace_root = WorkspaceRoot::home().expect("failed to initialize workspace root");
    let execution_store = ExecutionStore::open().expect("failed to initialize execution store");
    tauri::Builder::default()
        .manage(pty_state)
        .manage(workspace_root)
        .manage(execution_store)
        .manage(AgentRunRegistry::default())
        .plugin(tauri_plugin_dialog::init())
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
            execute_ai_worker_task,
            release_ai_worker_run,
            cancel_ai_worker_task,
            chat_ai_worker,
            list_directory,
            read_file,
            write_file,
            workspace_root_path,
            set_workspace_root,
            load_app_secrets,
            save_app_secrets,
            load_cached_workspace,
            save_cached_workspace,
            save_execution_run,
            list_active_execution_runs,
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
            pty_current_directory
        ])
        .on_window_event(move |_window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                close_all_terminals(&shutdown_state);
                close_all_mcp_sessions();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
