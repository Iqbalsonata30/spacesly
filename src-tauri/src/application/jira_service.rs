use crate::application::app::{AppState, ImportedIssue};
use crate::domain::entity::Workspace;
use crate::infrastructure::jira_rest::{add_comment, assign_issue, transition_issue};
use crate::infrastructure::mcp::{
    close_mcp_session, fetch_jira_boards, fetch_jira_issues, test_jira_connection,
    test_mcp_connection, JiraBoard, JiraConnectionStatus, JiraIssue, JiraMcpConfig,
    McpConnectionStatus, McpServerConfig,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Application boundary for Jira/MCP workflows exposed through IPC.
pub struct JiraService;

impl JiraService {
    pub fn new() -> Self {
        Self
    }

    pub fn issues(&self, config: JiraMcpConfig) -> Result<Vec<JiraIssue>, String> {
        fetch_jira_issues(config)
    }

    pub fn boards(&self, config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
        fetch_jira_boards(config)
    }

    pub fn test_jira_connection(
        &self,
        config: JiraMcpConfig,
    ) -> Result<JiraConnectionStatus, String> {
        test_jira_connection(config)
    }

    pub fn test_mcp_connection(
        &self,
        config: McpServerConfig,
    ) -> Result<McpConnectionStatus, String> {
        test_mcp_connection(config)
    }

    pub fn disconnect_mcp_server(&self, config: McpServerConfig) -> Result<bool, String> {
        close_mcp_session(config)
    }

    pub fn sync_workspace(&self, config: JiraMcpConfig) -> Result<Workspace, String> {
        let issues = fetch_jira_issues(config)?;
        let fetched_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("System clock is before Unix epoch: {error}"))?
            .as_millis()
            .to_string();
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
                jira_updated_at: issue.updated_at,
                jira_fetched_at: fetched_at.clone(),
            })
            .collect();

        Ok(AppState::new().workspace_with_imported_issues(&imported_issues))
    }

    pub fn transition_issue(
        &self,
        config: JiraMcpConfig,
        issue_key: String,
        target_status: String,
    ) -> Result<(), String> {
        transition_issue(&config.auth, &issue_key, &target_status)
    }

    pub fn assign_issue(&self, config: JiraMcpConfig, issue_key: String) -> Result<(), String> {
        assign_issue(&config.auth, &issue_key)
    }

    pub fn add_comment(
        &self,
        config: JiraMcpConfig,
        issue_key: String,
        comment: String,
    ) -> Result<(), String> {
        add_comment(&config.auth, &issue_key, &comment)
    }
}
