mod application;
mod domain;
mod infrastructure;

use application::app::{AppState, ImportedIssue};
use domain::entity::Workspace;
use infrastructure::ai_worker::{
    chat_ai_worker as chat_ai_worker_impl, execute_ai_worker_task as execute_ai_worker_task_impl,
    test_ai_worker as test_ai_worker_impl, AiWorkerChatRequest, AiWorkerConfig, AiWorkerResult,
    AiWorkerStatus, AiWorkerTask,
};
use infrastructure::jira_rest::{add_comment, assign_issue, transition_issue};
use infrastructure::mcp::{
    fetch_jira_boards, fetch_jira_issues, test_jira_connection, test_mcp_connection, JiraBoard,
    JiraConnectionStatus, JiraIssue, JiraMcpConfig, McpConnectionStatus, McpServerConfig,
};
use infrastructure::pty::{
    close_all_terminals, open_pty_terminal as open_pty_terminal_impl,
    resize_pty_terminal as resize_pty_terminal_impl, write_pty_terminal as write_pty_terminal_impl,
    PtyRegistry, PtyState,
};
use infrastructure::shell::{
    complete_shell_input as complete_shell_input_impl, run_shell_command as run_shell_command_impl,
    ShellCommandRequest, ShellCommandResult, ShellCompletionRequest, ShellCompletionResult,
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
    tauri::async_runtime::spawn_blocking(move || fetch_jira_issues(config))
        .await
        .map_err(|error| format!("Jira issue task failed: {error}"))?
}

#[tauri::command]
async fn get_jira_boards(config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
    tauri::async_runtime::spawn_blocking(move || fetch_jira_boards(config))
        .await
        .map_err(|error| format!("Jira board task failed: {error}"))?
}

#[tauri::command]
async fn test_jira_mcp_connection(config: JiraMcpConfig) -> Result<JiraConnectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || test_jira_connection(config))
        .await
        .map_err(|error| format!("Jira MCP test task failed: {error}"))?
}

#[tauri::command]
async fn test_mcp_server_connection(
    config: McpServerConfig,
) -> Result<McpConnectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || test_mcp_connection(config))
        .await
        .map_err(|error| format!("MCP test task failed: {error}"))?
}

#[tauri::command]
async fn sync_jira_workspace(config: JiraMcpConfig) -> Result<Workspace, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let issues = fetch_jira_issues(config)?;
        let imported_issues: Vec<ImportedIssue> = issues
            .into_iter()
            .map(|issue| ImportedIssue {
                key: issue.key,
                summary: issue.summary,
                description: issue.description,
                status: issue.status,
                issue_type: issue.issue_type,
                url: issue.url,
                labels: issue.labels,
            })
            .collect();

        Ok(AppState::new().workspace_with_imported_issues(&imported_issues))
    })
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
        transition_issue(&config.auth, &issue_key, &target_status)
    })
    .await
    .map_err(|error| format!("Jira transition task failed: {error}"))?
}

#[tauri::command]
async fn assign_jira_issue(config: JiraMcpConfig, issue_key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || assign_issue(&config.auth, &issue_key))
        .await
        .map_err(|error| format!("Jira assign task failed: {error}"))?
}

#[tauri::command]
async fn add_jira_comment(
    config: JiraMcpConfig,
    issue_key: String,
    comment: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || add_comment(&config.auth, &issue_key, &comment))
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
) -> Result<AiWorkerResult, String> {
    tauri::async_runtime::spawn_blocking(move || execute_ai_worker_task_impl(config, task))
        .await
        .map_err(|error| format!("Agent execution task failed: {error}"))?
}

#[tauri::command]
async fn chat_ai_worker(
    config: AiWorkerConfig,
    request: AiWorkerChatRequest,
) -> Result<AiWorkerResult, String> {
    tauri::async_runtime::spawn_blocking(move || chat_ai_worker_impl(config, request))
        .await
        .map_err(|error| format!("Agent chat task failed: {error}"))?
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
) -> Result<(), String> {
    open_pty_terminal_impl(&state, terminal_id, workdir, on_data)
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
    tauri::Builder::default()
        .manage(pty_state)
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
            run_shell_command,
            complete_shell_input,
            open_pty_terminal,
            write_pty_terminal,
            resize_pty_terminal
        ])
        .on_window_event(move |_window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                close_all_terminals(&shutdown_state);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
