mod application;
mod domain;
mod infrastructure;

use application::app::AppState;
use application::files_service::FilesService;
use application::git_service::GitService;
use application::jira_service::JiraService;
use domain::entity::Workspace;
use infrastructure::ai_worker::{
    chat_ai_worker as chat_ai_worker_impl, execute_ai_worker_task as execute_ai_worker_task_impl,
    test_ai_worker as test_ai_worker_impl, AiWorkerChatRequest, AiWorkerChatResult, AiWorkerConfig,
    AiWorkerStatus, AiWorkerTask, AiWorkerTaskResult,
};
use infrastructure::files::{FileEntry, WorkspaceRoot};
use infrastructure::formatting::format_code as format_code_impl;
use infrastructure::git::GitWorkspaceInfo;
use infrastructure::git::git_info_for_path;
use infrastructure::mcp::{
    JiraBoard, JiraConnectionStatus, JiraIssue, JiraMcpConfig, McpConnectionStatus, McpServerConfig,
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

#[tauri::command]
fn get_workspace() -> Workspace {
    AppState::new().workspace()
}

#[tauri::command]
async fn get_jira_issues(config: JiraMcpConfig) -> Result<Vec<JiraIssue>, String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().issues(config))
        .await
        .map_err(|error| format!("Jira issue task failed: {error}"))?
}

#[tauri::command]
async fn get_jira_boards(config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().boards(config))
        .await
        .map_err(|error| format!("Jira board task failed: {error}"))?
}

#[tauri::command]
async fn test_jira_mcp_connection(config: JiraMcpConfig) -> Result<JiraConnectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().test_jira_connection(config))
        .await
        .map_err(|error| format!("Jira MCP test task failed: {error}"))?
}

#[tauri::command]
async fn test_mcp_server_connection(
    config: McpServerConfig,
) -> Result<McpConnectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().test_mcp_connection(config))
        .await
        .map_err(|error| format!("MCP test task failed: {error}"))?
}

#[tauri::command]
async fn sync_jira_workspace(config: JiraMcpConfig) -> Result<Workspace, String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().sync_workspace(config))
        .await
        .map_err(|error| format!("Jira sync task failed: {error}"))?
}

#[tauri::command]
async fn transition_jira_issue(
    config: JiraMcpConfig,
    issue_key: String,
    target_status: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().transition_issue(config, issue_key, target_status)
    })
    .await
    .map_err(|error| format!("Jira transition task failed: {error}"))?
}

#[tauri::command]
async fn assign_jira_issue(config: JiraMcpConfig, issue_key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || JiraService::new().assign_issue(config, issue_key))
        .await
        .map_err(|error| format!("Jira assign task failed: {error}"))?
}

#[tauri::command]
async fn add_jira_comment(
    config: JiraMcpConfig,
    issue_key: String,
    comment: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        JiraService::new().add_comment(config, issue_key, comment)
    })
    .await
    .map_err(|error| format!("Jira comment task failed: {error}"))?
}

#[tauri::command]
async fn test_ai_worker(config: AiWorkerConfig) -> Result<AiWorkerStatus, String> {
    tauri::async_runtime::spawn_blocking(move || test_ai_worker_impl(config))
        .await
        .map_err(|error| format!("Agent diagnostic task failed: {error}"))?
}

#[tauri::command]
async fn execute_ai_worker_task(
    config: AiWorkerConfig,
    task: AiWorkerTask,
) -> Result<AiWorkerTaskResult, String> {
    tauri::async_runtime::spawn_blocking(move || execute_ai_worker_task_impl(config, task))
        .await
        .map_err(|error| format!("Agent execution task failed: {error}"))?
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
        FilesService::new(root).write_file(workspace_id, relative_path, content)
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
    absolute_path: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<String, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        FilesService::new(root).set_workspace_root(absolute_path)
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
async fn save_cached_workspace(workspace: Workspace) -> Result<CachedWorkspace, String> {
    tauri::async_runtime::spawn_blocking(move || save_cached_workspace_impl(workspace))
        .await
        .map_err(|error| format!("Save workspace cache task failed: {error}"))?
}

#[tauri::command]
async fn format_code(formatter: String, source: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || format_code_impl(formatter, source))
        .await
        .map_err(|error| format!("Format task failed: {error}"))?
}

#[tauri::command]
async fn get_workspace_git_info(
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || GitService::new(root).workspace_git_info())
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
async fn checkout_workspace_git_branch(
    branch: String,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<GitWorkspaceInfo, String> {
    let root = workspace_root.inner().clone();
    tauri::async_runtime::spawn_blocking(move || GitService::new(root).checkout_branch(branch))
        .await
        .map_err(|error| format!("Checkout branch task failed: {error}"))?
}

#[tauri::command]
async fn run_shell_command(request: ShellCommandRequest) -> Result<ShellCommandResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_shell_command_impl(request))
        .await
        .map_err(|error| format!("Shell command task failed: {error}"))?
}

#[tauri::command]
fn open_pty_terminal(
    terminal_id: String,
    workdir: Option<String>,
    on_data: Channel<Vec<u8>>,
    state: State<'_, PtyState>,
    workspace_root: State<'_, WorkspaceRoot>,
) -> Result<(), String> {
    open_pty_terminal_impl(&state, &workspace_root, terminal_id, workdir, on_data)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pty_state: PtyState = Arc::new(Mutex::new(PtyRegistry::new()));
    let shutdown_state = pty_state.clone();
    let workspace_root = WorkspaceRoot::home().expect("failed to initialize workspace root");
    tauri::Builder::default()
        .manage(pty_state)
        .manage(workspace_root)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_workspace,
            get_jira_issues,
            get_jira_boards,
            test_jira_mcp_connection,
            test_mcp_server_connection,
            sync_jira_workspace,
            transition_jira_issue,
            assign_jira_issue,
            add_jira_comment,
            test_ai_worker,
            execute_ai_worker_task,
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
            format_code,
            get_workspace_git_info,
            get_path_git_info,
            checkout_workspace_git_branch,
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
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
