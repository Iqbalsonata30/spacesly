import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface AiWorkerConfig {
  runtime: "api" | "opencode";
  provider_name: string;
  base_url: string;
  api_style: "openai_chat" | "openai_responses" | "anthropic_messages";
  api_key: string;
  model: string;
  opencode_command: string;
  opencode_model: string;
  opencode_workdir: string | null;
  opencode_auto_approve: boolean;
  agent_rules: string;
  agent_skills: string;
  temperature: number;
}

export interface AiWorkerTask {
  key: string | null;
  title: string;
  description: string;
  labels: string[];
  url: string | null;
  operator_notes?: string | null;
  previous_output?: string | null;
}

export interface AiWorkerChatRequest {
  message: string;
  terminal_context: string | null;
  session_context: string | null;
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
  message: string;
}

export async function testAiWorker(config: AiWorkerConfig): Promise<AiWorkerStatus> {
  return invokeWithPolicy<AiWorkerStatus>("test_ai_worker", { config }, IPC_POLICIES.aiTest);
}

export async function executeAiWorkerTask(
  runId: string,
  config: AiWorkerConfig,
  task: AiWorkerTask,
): Promise<AiWorkerTaskResult> {
  const result = await invokeWithPolicy<unknown>("execute_ai_worker_task", { runId, config, task }, IPC_POLICIES.aiExecution);
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
): Promise<AiWorkerChatResult> {
  return invokeWithPolicy<AiWorkerChatResult>("chat_ai_worker", { config, request }, IPC_POLICIES.aiChat);
}

function validateAiWorkerTaskResult(result: unknown): AiWorkerTaskResult {
  if (typeof result !== "object" || result === null) {
    return invalidAiWorkerTaskResult("Agent returned an invalid structured result.");
  }

  const value = result as Partial<AiWorkerTaskResult>;
  const validStatus = value.completion_status === "completed" || value.completion_status === "blocked";
  const validShape = typeof value.summary === "string"
    && Array.isArray(value.evidence)
    && Array.isArray(value.details)
    && Array.isArray(value.next)
    && value.evidence.every((line) => typeof line === "string")
    && value.details.every((line) => typeof line === "string")
    && value.next.every((line) => typeof line === "string")
    && (value.blocked_reason === null || typeof value.blocked_reason === "string");

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
