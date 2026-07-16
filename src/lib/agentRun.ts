import type { AiWorkerTaskResult, ExecutionContract } from "$lib/ipc";
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

export type AgentRunGitSnapshot = Pick<
  GitWorkspaceInfo,
  "repo_root" | "current_branch" | "head_commit"
>;

export type AgentSessionEventType =
  "system" | "operator_note" | "approval" | "agent_output" | "blocker" | "error";

export type AgentSessionEvent = {
  id: string;
  type: AgentSessionEventType;
  at: number;
  text: string;
};

export type ExecutionStepStatus =
  | "pending"
  | "ready"
  | "running"
  | "completed"
  | "blocked"
  | "failed"
  | "interrupted"
  | "skipped";

export type StepRun = {
  step_id: string;
  status: ExecutionStepStatus;
  attempt: number;
  started_at: string | null;
  completed_at: string | null;
  summary: string | null;
  lease_owner?: string | null;
  lease_expires_at?: number | null;
};

export type ExecutionRun = {
  run_id: string;
  contract: ExecutionContract;
  status: "pending" | "running" | "blocked" | "failed" | "completed" | "cancelled";
  current_step_ids: string[];
  step_runs: Record<string, StepRun>;
  started_at: string;
  completed_at: string | null;
  revision?: number;
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
  executionRun: ExecutionRun | null;
};

const ACTIVE_AGENT_RUNS_KEY = "spacesly.agent.active-runs.v1";

type ActiveAgentRunMarker = {
  cardId: string;
  runId: string;
  startedAt: number;
};

export function loadActiveAgentRunCardIds(storage: Storage | undefined): string[] {
  if (!storage) return [];

  try {
    const parsed = JSON.parse(storage.getItem(ACTIVE_AGENT_RUNS_KEY) ?? "[]") as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isActiveAgentRunMarker).map((marker) => marker.cardId);
  } catch {
    return [];
  }
}

export function markActiveAgentRun(
  storage: Storage | undefined,
  cardId: string,
  runId: string,
): void {
  if (!storage || !cardId || !runId) return;
  try {
    const markers = readActiveAgentRunMarkers(storage).filter((marker) => marker.cardId !== cardId);
    markers.push({ cardId, runId, startedAt: Date.now() });
    storage.setItem(ACTIVE_AGENT_RUNS_KEY, JSON.stringify(markers));
  } catch {
    // Run execution must not fail because browser storage is unavailable.
  }
}

export function clearActiveAgentRun(
  storage: Storage | undefined,
  cardId: string,
  runId: string,
): void {
  if (!storage) return;
  try {
    const markers = readActiveAgentRunMarkers(storage).filter(
      (marker) => marker.cardId !== cardId || marker.runId !== runId,
    );
    if (markers.length === 0) storage.removeItem(ACTIVE_AGENT_RUNS_KEY);
    else storage.setItem(ACTIVE_AGENT_RUNS_KEY, JSON.stringify(markers));
  } catch {
    // Cleanup is best effort when browser storage is unavailable.
  }
}

export function clearActiveAgentRuns(storage: Storage | undefined): void {
  if (!storage) return;
  try {
    storage.removeItem(ACTIVE_AGENT_RUNS_KEY);
  } catch {
    // Cleanup is best effort when browser storage is unavailable.
  }
}

function readActiveAgentRunMarkers(storage: Storage): ActiveAgentRunMarker[] {
  try {
    const parsed = JSON.parse(storage.getItem(ACTIVE_AGENT_RUNS_KEY) ?? "[]") as unknown;
    return Array.isArray(parsed) ? parsed.filter(isActiveAgentRunMarker) : [];
  } catch {
    return [];
  }
}

function isActiveAgentRunMarker(value: unknown): value is ActiveAgentRunMarker {
  if (!value || typeof value !== "object") return false;
  const marker = value as Partial<ActiveAgentRunMarker>;
  return (
    typeof marker.cardId === "string" &&
    marker.cardId.length > 0 &&
    typeof marker.runId === "string" &&
    marker.runId.length > 0 &&
    typeof marker.startedAt === "number"
  );
}

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
  executionRun: ExecutionRun | null,
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
    executionRun,
  };
}

export function createAgentSessionEvent(
  type: AgentSessionEventType,
  text: string,
  at = Date.now(),
): AgentSessionEvent {
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

export function agentSessionReplay(
  transcript: AgentSessionEvent[],
  maxChars: number,
): string | null {
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
