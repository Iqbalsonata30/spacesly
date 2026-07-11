import type { WorkspaceProjection } from "$lib/ipc";
import { invokeWithPolicy } from "$lib/ipc/policy";

const JIRA_READ_POLICY = { timeoutMs: 30_000, retries: 2, retryDelayMs: 500 };
const JIRA_TEST_POLICY = { timeoutMs: 20_000, retries: 1, retryDelayMs: 500 };
const JIRA_MUTATION_POLICY = { timeoutMs: 30_000, retries: 0 };

export interface JiraIssue {
  key: string;
  summary: string;
  description: string | null;
  status: string;
  issue_type: string;
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
}

export interface JiraMcpServerConfig {
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface McpConnectionStatus {
  tool_count: number;
  tools: string[];
}

export interface JiraMcpConfig {
  server: JiraMcpServerConfig;
  auth: {
    base_url: string;
    auth_mode: string;
    username: string;
    api_token: string;
    personal_access_token: string;
    password: string;
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
  return invokeWithPolicy<JiraIssue[]>("get_jira_issues", { config }, JIRA_READ_POLICY);
}

export async function getJiraBoards(config: JiraMcpConfig): Promise<JiraBoard[]> {
  return invokeWithPolicy<JiraBoard[]>("get_jira_boards", { config }, JIRA_READ_POLICY);
}

export async function testJiraMcpConnection(config: JiraMcpConfig): Promise<JiraConnectionStatus> {
  return invokeWithPolicy<JiraConnectionStatus>("test_jira_mcp_connection", { config }, JIRA_TEST_POLICY);
}

export async function testMcpServerConnection(config: JiraMcpServerConfig): Promise<McpConnectionStatus> {
  return invokeWithPolicy<McpConnectionStatus>("test_mcp_server_connection", { config }, JIRA_TEST_POLICY);
}

export async function syncJiraWorkspace(config: JiraMcpConfig): Promise<WorkspaceProjection> {
  return invokeWithPolicy<WorkspaceProjection>("sync_jira_workspace", { config }, JIRA_READ_POLICY);
}

export async function transitionJiraIssue(
  config: JiraMcpConfig,
  issueKey: string,
  targetStatus: string,
): Promise<void> {
  return invokeWithPolicy<void>("transition_jira_issue", { config, issueKey, targetStatus }, JIRA_MUTATION_POLICY);
}

export async function assignJiraIssue(config: JiraMcpConfig, issueKey: string): Promise<void> {
  return invokeWithPolicy<void>("assign_jira_issue", { config, issueKey }, JIRA_MUTATION_POLICY);
}

export async function addJiraComment(
  config: JiraMcpConfig,
  issueKey: string,
  comment: string,
): Promise<void> {
  return invokeWithPolicy<void>("add_jira_comment", { config, issueKey, comment }, JIRA_MUTATION_POLICY);
}
