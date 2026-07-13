import type { AiWorkerTaskResult } from "$lib/ipc";

export type AgentRunStatus = "idle" | "running" | "timeout" | "completed" | "blocked";

export type AgentRunLog = {
  id: string;
  at: string;
  tone: "info" | "success" | "error";
  label: string;
  message: string;
};

export type AgentTerminalLine = {
  id: string;
  prompt: string;
  text: string;
};

export type AgentRunSession = {
  cardId: string;
  title: string;
  status: AgentRunStatus;
  progress: number;
  output: string;
  result: AiWorkerTaskResult | null;
  logs: AgentRunLog[];
  terminalLines: AgentTerminalLine[];
};

export function createAgentRunSession(
  cardId: string,
  title: string,
  status: AgentRunStatus,
  progress: number,
  output: string,
  result: AiWorkerTaskResult | null,
  logs: AgentRunLog[],
  terminalLines: AgentTerminalLine[],
): AgentRunSession {
  return {
    cardId,
    title,
    status,
    progress,
    output,
    result,
    logs,
    terminalLines,
  };
}
