import { invokeWithPolicy } from "$lib/ipc/policy";

const AI_TEST_POLICY = { timeoutMs: 30_000, retries: 1, retryDelayMs: 750 };
const AI_EXECUTION_POLICY = { timeoutMs: 180_000, retries: 0 };
const AI_CHAT_POLICY = { timeoutMs: 120_000, retries: 0 };

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
}

export interface AiWorkerStatus {
  connected: boolean;
  provider_name: string;
  model: string;
  message: string;
}

export interface AiWorkerResult {
  summary: string;
  raw_response: string;
  completion_status: "completed" | "blocked";
  blocked_reason: string | null;
}

export async function testAiWorker(config: AiWorkerConfig): Promise<AiWorkerStatus> {
  return invokeWithPolicy<AiWorkerStatus>("test_ai_worker", { config }, AI_TEST_POLICY);
}

export async function executeAiWorkerTask(
  config: AiWorkerConfig,
  task: AiWorkerTask,
): Promise<AiWorkerResult> {
  return invokeWithPolicy<AiWorkerResult>("execute_ai_worker_task", { config, task }, AI_EXECUTION_POLICY);
}

export async function chatAiWorker(
  config: AiWorkerConfig,
  request: AiWorkerChatRequest,
): Promise<AiWorkerResult> {
  return invokeWithPolicy<AiWorkerResult>("chat_ai_worker", { config, request }, AI_CHAT_POLICY);
}
