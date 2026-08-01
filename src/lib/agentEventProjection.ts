import { capList } from "$lib/boundedBuffers";
import type { AgentRunLog, AgentRunSession } from "$lib/agentRun";
import type { TaskSessionEvent, TaskSessionState } from "$lib/ipc";

export type AgentEventProjection = {
  progress: number | null;
  taskSessionState: TaskSessionState | null;
  logs: AgentRunLog[];
};

export function projectAgentTaskSessionEvent(
  event: TaskSessionEvent,
  logId: string,
  at: string,
): AgentEventProjection {
  const payload =
    typeof event.payload === "object" && event.payload !== null && !Array.isArray(event.payload)
      ? (event.payload as Record<string, unknown>)
      : {};
  const eventType = typeof payload.type === "string" ? payload.type : event.kind;
  const progress = event.progress
    ? event.progress.total && event.progress.total > 0
      ? 35 + Math.round((event.progress.completed / event.progress.total) * 35)
      : 55
    : null;

  if (event.kind === "runtime" && eventType === "text_delta") {
    return { progress, taskSessionState: null, logs: [] };
  }

  let tone: AgentRunLog["tone"] = "info";
  let summary = `Task Session ${event.kind}: ${eventType}.`;
  let taskSessionState: TaskSessionState | null = null;
  if (event.kind === "lifecycle" && typeof payload.state === "string") {
    taskSessionState = payload.state as TaskSessionState;
    tone = payload.state === "failed" || payload.state === "blocked" ? "error" : "info";
    summary = `Task Session entered ${payload.state}.`;
  } else if (event.kind === "tool") {
    const context =
      typeof payload.display_context === "object" && payload.display_context !== null
        ? (payload.display_context as Record<string, unknown>)
        : {};
    const label = typeof context.label === "string" ? context.label : payload.tool_name;
    const failed = payload.type === "tool_completed" && payload.success === false;
    tone = failed ? "error" : "info";
    summary = `${payload.type === "tool_completed" ? (failed ? "Tool failed" : "Tool completed; task still running") : "Tool started"}: ${String(label ?? "Agent tool")}.`;
  } else if (event.kind === "runtime" && eventType === "agent_result_candidate") {
    summary = "Agent result staged for authoritative Task Session commit.";
  }

  return {
    progress,
    taskSessionState,
    logs: [
      {
        id: logId,
        at,
        tone,
        label: event.kind,
        message: [
          `STATUS: ${tone === "error" ? "Blocked" : "Running"}`,
          `SUMMARY: ${summary}`,
          "EVIDENCE:",
          `- Task Session event sequence: ${event.sequence}`,
          `- Assignment attempt: ${event.attempt_id ?? "unassigned"}`,
          "DETAILS:",
          ...(event.progress ? [`- Progress phase: ${event.progress.phase}`] : []),
        ].join("\n"),
      },
    ],
  };
}

export function mergeAgentEventProjection(
  current: AgentEventProjection,
  incoming: AgentEventProjection,
): AgentEventProjection {
  const logs = [...current.logs];
  for (const log of incoming.logs) {
    if (log.label === "progress" && logs.at(-1)?.label === "progress") logs[logs.length - 1] = log;
    else logs.push(log);
  }
  return {
    progress:
      incoming.progress === null
        ? current.progress
        : Math.max(current.progress ?? 0, incoming.progress),
    taskSessionState: incoming.taskSessionState ?? current.taskSessionState,
    logs,
  };
}

export function applyAgentEventProjection(
  session: AgentRunSession,
  projection: AgentEventProjection,
  maxLogs: number,
): AgentRunSession {
  const progress =
    projection.progress === null
      ? session.progress
      : Math.max(session.progress, Math.min(100, projection.progress));
  const taskSessionState = projection.taskSessionState ?? session.taskSessionState;
  if (
    progress === session.progress &&
    taskSessionState === session.taskSessionState &&
    projection.logs.length === 0
  ) {
    return session;
  }
  return {
    ...session,
    progress,
    taskSessionState,
    logs:
      projection.logs.length === 0
        ? session.logs
        : capList([...session.logs, ...projection.logs], maxLogs),
  };
}

export function emptyAgentEventProjection(): AgentEventProjection {
  return { progress: null, taskSessionState: null, logs: [] };
}
