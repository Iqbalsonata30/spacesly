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

  if (
    event.kind === "runtime" &&
    (eventType === "text_delta" || eventType === "execution_trace_stage")
  ) {
    return { progress, taskSessionState: null, logs: [] };
  }

  let tone: AgentRunLog["tone"] = "info";
  let summary = `Task Session ${event.kind}: ${eventType}.`;
  let taskSessionState: TaskSessionState | null = null;
  const details: string[] = [];
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
  } else if (event.kind === "runtime" && eventType === "runtime_recovery_decision") {
    const action = typeof payload.action === "string" ? payload.action : "stop_failed";
    const failureClass =
      typeof payload.failure_class === "string" ? payload.failure_class : "unknown";
    const reason = typeof payload.reason === "string" ? payload.reason : "No reason recorded.";
    tone = action === "retry_current_assignment" ? "info" : "error";
    summary =
      action === "retry_current_assignment"
        ? `Transient ${failureClass.replaceAll("_", " ")} failure; Spacesly is retrying safely.`
        : `Runtime recovery requires attention: ${failureClass.replaceAll("_", " ")}.`;
    details.push(`- Recovery action: ${action}`, `- Recovery reason: ${reason}`);
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
          ...details,
        ].join("\n"),
      },
    ],
  };
}

export function mergeAgentEventProjection(
  current: AgentEventProjection,
  incoming: AgentEventProjection,
): AgentEventProjection {
  if (
    isTerminalTaskSessionState(current.taskSessionState) &&
    incoming.taskSessionState !== current.taskSessionState
  ) {
    return current;
  }
  const logs = [...current.logs];
  const logIds = new Set(logs.map((log) => log.id));
  for (const log of incoming.logs) {
    if (logIds.has(log.id)) continue;
    if (log.label === "progress" && logs.at(-1)?.label === "progress") {
      logIds.delete(logs[logs.length - 1].id);
      logs[logs.length - 1] = log;
    } else {
      logs.push(log);
    }
    logIds.add(log.id);
  }
  return {
    progress:
      incoming.progress === null
        ? current.progress
        : Math.max(current.progress ?? 0, incoming.progress),
    taskSessionState: reconcileTaskSessionState(
      current.taskSessionState,
      incoming.taskSessionState,
    ),
    logs,
  };
}

export function applyAgentEventProjection(
  session: AgentRunSession,
  projection: AgentEventProjection,
  maxLogs: number,
): AgentRunSession {
  const currentTaskSessionState = session.taskSessionState ?? null;
  if (
    isTerminalTaskSessionState(currentTaskSessionState) &&
    projection.taskSessionState !== currentTaskSessionState
  ) {
    return session;
  }
  const progress =
    projection.progress === null
      ? session.progress
      : Math.max(session.progress, Math.min(100, projection.progress));
  const taskSessionState = reconcileTaskSessionState(
    currentTaskSessionState,
    projection.taskSessionState,
  );
  const existingLogIds = new Set(session.logs.map((log) => log.id));
  const newLogs = projection.logs.filter((log) => !existingLogIds.has(log.id));
  if (
    progress === session.progress &&
    taskSessionState === session.taskSessionState &&
    newLogs.length === 0
  ) {
    return session;
  }
  return {
    ...session,
    progress,
    taskSessionState,
    logs: newLogs.length === 0 ? session.logs : capList([...session.logs, ...newLogs], maxLogs),
  };
}

export function emptyAgentEventProjection(): AgentEventProjection {
  return { progress: null, taskSessionState: null, logs: [] };
}

function reconcileTaskSessionState(
  current: TaskSessionState | null,
  incoming: TaskSessionState | null,
): TaskSessionState | null {
  if (incoming === null || isTerminalTaskSessionState(current)) return current;
  return incoming;
}

function isTerminalTaskSessionState(state: TaskSessionState | null): boolean {
  return (
    state === "succeeded" || state === "failed" || state === "blocked" || state === "cancelled"
  );
}
