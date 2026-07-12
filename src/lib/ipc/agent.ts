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
  config: AiWorkerConfig,
  task: AiWorkerTask,
): Promise<AiWorkerTaskResult> {
  return invokeWithPolicy<AiWorkerTaskResult>("execute_ai_worker_task", { config, task }, IPC_POLICIES.aiExecution);
}

export async function chatAiWorker(
  config: AiWorkerConfig,
  request: AiWorkerChatRequest,
): Promise<AiWorkerChatResult> {
  return invokeWithPolicy<AiWorkerChatResult>("chat_ai_worker", { config, request }, IPC_POLICIES.aiChat);
}
