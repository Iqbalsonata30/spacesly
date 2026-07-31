import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import { Channel } from "@tauri-apps/api/core";

export interface AiWorkerMcpServer {
  name: string;
  secret_id: string;
  command: string[];
  environment?: Record<string, string>;
}

export interface AiWorkerConfig {
  workspace_id: string;
  runtime: "api" | "opencode";
  provider_name: string;
  provider_id: string;
  base_url: string;
  api_style: "openai_chat" | "openai_responses" | "anthropic_messages";
  model: string;
  opencode_command: string;
  opencode_model: string;
  opencode_workdir: string | null;
  opencode_auto_approve: boolean;
  agent_rules: string;
  agent_skills: string;
  temperature: number;
  mcp_servers: AiWorkerMcpServer[];
}

export interface AiWorkerTask {
  execution_contract: ExecutionContract;
  session_key: string;
}

export interface ExecutionContract {
  contract_id: string;
  version: number;
  task_id: string;
  workspace_id: string;
  created_at: string;
  objective: {
    summary: string;
    success_criteria: string[];
  };
  task_context: {
    description: string;
    execution_detail: string;
  };
  ticket: {
    provider: "jira" | "local";
    key: string | null;
    url: string | null;
    title: string;
    labels: string[];
    status: string | null;
    updated_at: string | null;
    fetched_at: string | null;
  };
  workflow: ExecutionContractStep[];
  completed_steps: string[];
  current_step: string;
  remaining_steps: string[];
  repository: {
    root_path: string | null;
    branch: string | null;
    head_commit: string | null;
  };
  constraints: {
    execution_only: true;
    planning_completed: true;
    must_not_read_jira_for_planning: true;
    must_not_classify_ticket: true;
    must_not_regenerate_workflow: true;
    must_not_rediscover_repository: true;
    may_modify_files: boolean;
    may_update_jira: boolean;
  };
  runtime_inputs: {
    operator_notes: string | null;
    previous_output: string | null;
  };
}

export interface ExecutionContractStep {
  step_id: string;
  title: string;
  type: "jira.transition" | "worker.execute" | "worker.verify" | "jira.comment";
  status: "completed" | "current" | "remaining";
}

export interface AiWorkerChatRequest {
  run_id?: string;
  conversation_id?: string;
  message_id?: string;
  message_sequence?: number;
  message: string;
  terminal_context: string | null;
  context_revision: string | null;
  session_context: string | null;
  session_key: string;
}

export interface AiWorkerStatus {
  connected: boolean;
  provider_name: string;
  model: string;
  message: string;
}

export interface AiWorkerTaskResult {
  summary: string;
  evidence: string[];
  details: string[];
  next: string[];
  completion_status: "completed" | "blocked";
  blocked_reason: string | null;
}

export interface AiWorkerChatResult {
  run_id: string;
  message: string;
}

export type ToolOperationRisk =
  "read" | "mutation" | "destructive" | "credential_sensitive" | "unknown";

export interface ToolDisplayContext {
  label: string;
  category:
    "files" | "commands" | "git" | "jira" | "kubernetes" | "bamboo" | "runtime" | "external";
  target: string | null;
}

export type AiRuntimeEvent =
  | { type: "run_started"; run_id: string; sequence: number }
  | { type: "text_delta"; run_id: string; sequence: number; delta: string }
  | {
      type: "tool_started";
      run_id: string;
      sequence: number;
      tool_call_id: string;
      tool_name: string;
      risk: ToolOperationRisk;
      operation_id: string;
      arguments_digest: string;
      display_context: ToolDisplayContext;
    }
  | {
      type: "tool_completed";
      run_id: string;
      sequence: number;
      tool_call_id: string;
      tool_name: string;
      success: boolean;
      error: string | null;
      risk: ToolOperationRisk;
      operation_id: string;
      arguments_digest: string;
      display_context: ToolDisplayContext;
    }
  | {
      type: "approval_required";
      run_id: string;
      sequence: number;
      capability: string;
      operation: string;
      risk: ToolOperationRisk;
      operation_id: string;
      arguments_digest: string;
    }
  | {
      type: "usage_updated";
      run_id: string;
      sequence: number;
      input_tokens: number;
      output_tokens: number;
    }
  | { type: "run_completed"; run_id: string; sequence: number }
  | { type: "run_blocked"; run_id: string; sequence: number }
  | { type: "run_failed"; run_id: string; sequence: number; error_code: string }
  | { type: "run_cancelled"; run_id: string; sequence: number };

export type AiRunKind = "chat" | "edit" | "agent";
export type AiRunStatus =
  "queued" | "running" | "cancelling" | "completed" | "blocked" | "failed" | "cancelled";

export interface AiRun {
  run_id: string;
  kind: AiRunKind;
  status: AiRunStatus;
  created_at: number;
  updated_at: number;
}

export interface AiWorkspaceTrustStatus {
  path: string;
  trusted: boolean;
}

export interface AiEditRequest {
  run_id?: string;
  file_path: string;
  instruction: string;
  content: string;
  selection?: AiEditSelection | null;
  context_files?: AiEditContextFile[];
  diagnostics?: string[];
}

export interface AiEditSelection {
  start_line: number;
  start_character: number;
  end_line: number;
  end_character: number;
  text: string;
}

export interface AiEditContextFile {
  file_path: string;
  content: string;
}

export interface AiEditResult {
  run_id: string;
  summary: string;
  content: string;
}

export interface ConversationRecord {
  id: string;
  workspace_id: string;
  title: string;
  created_at: number;
  updated_at: number;
}

export interface ConversationMessageRecord {
  id: string;
  conversation_id: string;
  sequence: number;
  role: "user" | "agent" | "system";
  text: string;
  created_at: number;
}

export async function listConversations(workspaceId: string): Promise<ConversationRecord[]> {
  return invokeWithPolicy<ConversationRecord[]>(
    "list_conversations",
    { workspaceId },
    IPC_POLICIES.fileRead,
  );
}

export async function loadConversationMessages(
  workspaceId: string,
  conversationId: string,
): Promise<ConversationMessageRecord[]> {
  return invokeWithPolicy<ConversationMessageRecord[]>(
    "load_conversation_messages",
    { workspaceId, conversationId },
    IPC_POLICIES.fileRead,
  );
}

export async function appendConversationMessage(
  workspaceId: string,
  conversationId: string,
  title: string,
  message: Pick<ConversationMessageRecord, "id" | "text"> & { role: "user" | "system" },
): Promise<ConversationMessageRecord> {
  return invokeWithPolicy<ConversationMessageRecord>(
    "append_conversation_message",
    { workspaceId, conversationId, title, message },
    IPC_POLICIES.fileWrite,
  );
}

export async function importConversations(
  workspaceId: string,
  conversations: Array<{
    id: string;
    title: string;
    messages: Array<Pick<ConversationMessageRecord, "id" | "role" | "text">>;
  }>,
): Promise<number> {
  return invokeWithPolicy<number>(
    "import_conversations",
    { workspaceId, conversations },
    IPC_POLICIES.fileWrite,
  );
}

export async function pruneConversations(
  workspaceId: string,
  retainedIds: string[],
): Promise<number> {
  return invokeWithPolicy<number>(
    "prune_conversations",
    { workspaceId, retainedIds },
    IPC_POLICIES.fileWrite,
  );
}

export async function testAiWorker(config: AiWorkerConfig): Promise<AiWorkerStatus> {
  return invokeWithPolicy<AiWorkerStatus>("test_ai_worker", { config }, IPC_POLICIES.aiTest);
}

export async function getAiRun(runId: string): Promise<AiRun | null> {
  return invokeWithPolicy<AiRun | null>("get_ai_run", { runId }, IPC_POLICIES.fileRead);
}

export async function grantAiRunCapabilities(
  runId: string,
  capabilities: (
    | "workspace_read"
    | "workspace_write"
    | "shell"
    | "git"
    | "external_tools"
    | `external_tools:${string}`
  )[],
): Promise<void> {
  return invokeWithPolicy<void>(
    "grant_ai_run_capabilities",
    { runId, capabilities },
    IPC_POLICIES.aiExecution,
  );
}

export async function cancelAiRun(runId: string): Promise<boolean> {
  return invokeWithPolicy<boolean>("cancel_ai_run", { runId }, IPC_POLICIES.pty);
}

export async function aiWorkspaceTrustStatus(workspaceId: string): Promise<AiWorkspaceTrustStatus> {
  return invokeWithPolicy<AiWorkspaceTrustStatus>(
    "ai_workspace_trust_status",
    { workspaceId },
    IPC_POLICIES.fileRead,
  );
}

export async function trustAiWorkspace(workspaceId: string): Promise<AiWorkspaceTrustStatus> {
  return invokeWithPolicy<AiWorkspaceTrustStatus>(
    "trust_ai_workspace",
    { workspaceId },
    IPC_POLICIES.fileWrite,
  );
}

export async function beginAiRun(kind: AiRunKind): Promise<AiRun> {
  return invokeWithPolicy<AiRun>("begin_ai_run", { kind }, IPC_POLICIES.fileWrite);
}

export async function executeAiWorkerTask(
  runId: string,
  config: AiWorkerConfig,
  task: AiWorkerTask,
  onEvent: (event: AiRuntimeEvent) => void,
): Promise<AiWorkerTaskResult> {
  const channel = new Channel<AiRuntimeEvent>();
  channel.onmessage = onEvent;
  const result = await invokeWithPolicy<unknown>(
    "execute_ai_worker_task",
    { runId, config, task, onEvent: channel },
    IPC_POLICIES.aiExecution,
  );
  return validateAiWorkerTaskResult(result);
}

export async function reserveAiWorkerRun(runId: string, config: AiWorkerConfig): Promise<void> {
  await invokeWithPolicy<void>("reserve_ai_worker_run", { runId, config }, IPC_POLICIES.pty);
}

export async function releaseAiWorkerRun(runId: string): Promise<boolean> {
  return invokeWithPolicy<boolean>("release_ai_worker_run", { runId }, IPC_POLICIES.pty);
}

export async function cancelAiWorkerTask(runId: string): Promise<boolean> {
  return invokeWithPolicy<boolean>("cancel_ai_worker_task", { runId }, IPC_POLICIES.pty);
}

export async function chatAiWorker(
  config: AiWorkerConfig,
  request: AiWorkerChatRequest,
  onEvent: (event: AiRuntimeEvent) => void,
): Promise<AiWorkerChatResult> {
  const runId = request.run_id ?? (await beginAiRun("chat")).run_id;
  const channel = new Channel<AiRuntimeEvent>();
  channel.onmessage = onEvent;
  return invokeWithPolicy<AiWorkerChatResult>(
    "chat_ai_worker",
    { config, request: { ...request, run_id: runId }, onEvent: channel },
    IPC_POLICIES.aiChat,
  );
}

export async function proposeAiEdit(
  config: AiWorkerConfig,
  request: AiEditRequest,
  onEvent: (event: AiRuntimeEvent) => void,
): Promise<AiEditResult> {
  const runId = request.run_id ?? (await beginAiRun("edit")).run_id;
  const channel = new Channel<AiRuntimeEvent>();
  channel.onmessage = onEvent;
  return invokeWithPolicy<AiEditResult>(
    "propose_ai_edit",
    { config, request: { ...request, run_id: runId }, onEvent: channel },
    IPC_POLICIES.aiEdit,
  );
}

function validateAiWorkerTaskResult(result: unknown): AiWorkerTaskResult {
  if (typeof result !== "object" || result === null) {
    return invalidAiWorkerTaskResult("Agent returned an invalid structured result.");
  }

  const value = result as Partial<AiWorkerTaskResult>;
  const validStatus =
    value.completion_status === "completed" || value.completion_status === "blocked";
  const validShape =
    typeof value.summary === "string" &&
    Array.isArray(value.evidence) &&
    Array.isArray(value.details) &&
    Array.isArray(value.next) &&
    value.evidence.every((line) => typeof line === "string") &&
    value.details.every((line) => typeof line === "string") &&
    value.next.every((line) => typeof line === "string") &&
    (value.blocked_reason === null || typeof value.blocked_reason === "string");

  if (!validStatus || !validShape) {
    return invalidAiWorkerTaskResult("Agent returned an invalid structured result.");
  }

  return value as AiWorkerTaskResult;
}

function invalidAiWorkerTaskResult(reason: string): AiWorkerTaskResult {
  return {
    summary: reason,
    evidence: [],
    details: ["Spacesly could not validate the Agent result payload at the IPC boundary."],
    next: ["Retry the Agent or switch to a runtime that returns the required structured result."],
    completion_status: "blocked",
    blocked_reason: reason,
  };
}
