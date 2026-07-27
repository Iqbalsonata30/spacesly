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

    /// Syncs Jira issues onto a pre-built base workspace.
    /// Prefer this path when a managed `AppState` is already available (Tauri command context),
    /// so the seeded workspace is not reconstructed on every IPC call.
    pub fn sync_workspace_from(
        &self,
        base_workspace: Workspace,
        config: JiraMcpConfig,
    ) -> Result<Workspace, String> {
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

        Ok(AppState::from_workspace(base_workspace)
            .workspace_with_imported_issues(&imported_issues))
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::domain::entity::CardSource;
    use crate::infrastructure::mcp::{JiraAuthConfig, McpServerConfig};
    use std::collections::HashMap;

    #[test]
    fn syncs_board_issues_through_a_new_mcp_session() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"jira_get_board_issues"}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"issues":[{"key":"SPC-99","fields":{"summary":"Synced issue","status":{"name":"To Do"},"issuetype":{"name":"Task"}}}]}}\n' "$id"
      ;;
  esac
done
"#;
        let server = McpServerConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: HashMap::new(),
            scope_id: Some("jira-sync-regression".to_string()),
            secret_id: None,
        };
        let config = JiraMcpConfig {
            server: server.clone(),
            auth: JiraAuthConfig {
                base_url: "https://jira.example.com".to_string(),
                auth_mode: "api_token".to_string(),
                username: "user@example.com".to_string(),
                api_token: "token".to_string(),
                personal_access_token: String::new(),
                password: String::new(),
            },
            secret_id: "jira-default".to_string(),
            tool_name: "jira_search".to_string(),
            board_tool_name: "jira_get_agile_boards".to_string(),
            board_issues_tool_name: "jira_get_board_issues".to_string(),
            jql: String::new(),
            board_id: Some("7".to_string()),
            project_key: None,
            board_name: None,
            page_size: 25,
            max_pages: 1,
        };

        let workspace = JiraService::new()
            .sync_workspace_from(AppState::new().workspace(), config)
            .unwrap();
        let synced = workspace.projects[0].boards[0]
            .columns
            .iter()
            .flat_map(|column| &column.cards)
            .any(|card| matches!(&card.source, CardSource::Jira { key } if key == "SPC-99"));

        assert!(synced);
        close_mcp_session(server).unwrap();
    }
}
