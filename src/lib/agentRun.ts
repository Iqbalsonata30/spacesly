import type { AiWorkerTaskResult } from "$lib/ipc";
import type { GitWorkspaceInfo } from "$lib/ipc/git";

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

export type AgentRunGitSnapshot = Pick<GitWorkspaceInfo, "repo_root" | "current_branch" | "head_commit">;

export type AgentSessionEventType = "system" | "operator_note" | "approval" | "agent_output" | "blocker" | "error";

export type AgentSessionEvent = {
  id: string;
  type: AgentSessionEventType;
  at: number;
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
  gitSnapshot: AgentRunGitSnapshot | null;
  transcript: AgentSessionEvent[];
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
  gitSnapshot: AgentRunGitSnapshot | null,
  transcript: AgentSessionEvent[],
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
    gitSnapshot,
    transcript,
  };
}

export function createAgentSessionEvent(type: AgentSessionEventType, text: string, at = Date.now()): AgentSessionEvent {
  return {
    id: `evt-${at.toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
    type,
    at,
    text,
  };
}

export function appendAgentSessionEvent(
  transcript: AgentSessionEvent[],
  event: AgentSessionEvent,
  maxEvents: number,
): AgentSessionEvent[] {
  return [...transcript, event].slice(-maxEvents);
}

export function agentSessionReplay(transcript: AgentSessionEvent[], maxChars: number): string | null {
  const lines = transcript
    .map((event) => {
      const label = event.type.replace("_", " ").toUpperCase();
      const at = new Date(event.at).toLocaleString();
      return `[${at}] ${label}: ${event.text.trim()}`;
    })
    .filter((line) => line.trim().length > 0);

  if (lines.length === 0) return null;
  const text = lines.join("\n");
  return text.length > maxChars ? text.slice(-maxChars) : text;
}
