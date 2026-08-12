import type { WorkspaceProjection } from "$lib/ipc";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface JiraIssue {
  key: string;
  summary: string;
  description: string | null;
  status: string;
  issue_type: string;
  url: string | null;
  labels: string[];
  updated_at: string | null;
}

export interface JiraBoard {
  id: string;
  name: string;
  board_type: string;
}

export interface JiraConnectionStatus {
  tool_count: number;
  issue_count: number;
  board_count: number;
  tools: string[];
  tool_metadata: McpToolMetadata[];
}

export interface McpToolMetadata {
  name: string;
  description: string | null;
  input_schema: unknown | null;
}

export interface JiraMcpServerConfig {
  command: string;
  args: string[];
  env: Record<string, string>;
  scope_id?: string;
  secret_id?: string;
}

export interface McpConnectionStatus {
  tool_count: number;
  tools: string[];
  tool_metadata: McpToolMetadata[];
}

export interface JiraMcpConfig {
  server: JiraMcpServerConfig;
  secret_id: string;
  auth: {
    base_url: string;
    auth_mode: string;
    username: string;
    api_token?: string;
    personal_access_token?: string;
    password?: string;
  };
  tool_name: string;
  board_tool_name: string;
  board_issues_tool_name: string;
  jql: string;
  board_id: string | null;
  project_key: string | null;
  board_name: string | null;
  page_size: number;
  max_pages: number;
}

export async function getJiraIssues(config: JiraMcpConfig): Promise<JiraIssue[]> {
  return invokeWithPolicy<JiraIssue[]>("get_jira_issues", { config }, IPC_POLICIES.jiraRead);
}

export async function getJiraBoards(config: JiraMcpConfig): Promise<JiraBoard[]> {
  return invokeWithPolicy<JiraBoard[]>("get_jira_boards", { config }, IPC_POLICIES.jiraRead);
}

export async function testJiraMcpConnection(config: JiraMcpConfig): Promise<JiraConnectionStatus> {
  return invokeWithPolicy<JiraConnectionStatus>(
    "test_jira_mcp_connection",
    { config },
    IPC_POLICIES.mcpTest,
  );
}

export async function testMcpServerConnection(
  config: JiraMcpServerConfig,
): Promise<McpConnectionStatus> {
  return invokeWithPolicy<McpConnectionStatus>(
    "test_mcp_server_connection",
    { config },
    IPC_POLICIES.mcpTest,
  );
}

export async function disconnectMcpServer(config: JiraMcpServerConfig): Promise<boolean> {
  return invokeWithPolicy<boolean>("disconnect_mcp_server", { config }, IPC_POLICIES.jiraMutation);
}

export async function syncJiraWorkspace(config: JiraMcpConfig): Promise<WorkspaceProjection> {
  return invokeWithPolicy<WorkspaceProjection>(
    "sync_jira_workspace",
    { config },
    IPC_POLICIES.jiraRead,
  );
}

export async function transitionJiraIssue(
  config: JiraMcpConfig,
  issueKey: string,
  targetStatus: string,
): Promise<void> {
  return invokeWithPolicy<void>(
    "transition_jira_issue",
    { config, issueKey, targetStatus },
    IPC_POLICIES.jiraMutation,
  );
}

export async function assignJiraIssue(config: JiraMcpConfig, issueKey: string): Promise<void> {
  return invokeWithPolicy<void>(
    "assign_jira_issue",
    { config, issueKey },
    IPC_POLICIES.jiraMutation,
  );
}

export async function addJiraComment(
  config: JiraMcpConfig,
  issueKey: string,
  comment: string,
): Promise<void> {
  return invokeWithPolicy<void>(
    "add_jira_comment",
    { config, issueKey, comment },
    IPC_POLICIES.jiraMutation,
  );
}

export type JiraFinalResultCommentResult = {
  status: "created" | "already_complete";
  comment_id: string;
};

export async function addJiraFinalResultComment(
  config: JiraMcpConfig,
  executionRunId: string,
  issueKey: string,
  comment: string,
): Promise<JiraFinalResultCommentResult> {
  return invokeWithPolicy<JiraFinalResultCommentResult>(
    "add_jira_final_result_comment",
    { config, executionRunId, issueKey, comment },
    IPC_POLICIES.jiraMutation,
  );
}
