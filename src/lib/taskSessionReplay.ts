import type {
  TaskProgress,
  TaskSessionEvent,
  TaskSessionEventPage,
  TaskSessionState,
} from "./ipc/taskSessions";

export type TaskSessionReplayState = {
  sessionId: number;
  cursor: number;
  state: TaskSessionState | null;
  progress: TaskProgress | null;
  events: TaskSessionEvent[];
};

export type TaskSessionReplayResult = {
  replay: TaskSessionReplayState;
  gapDetected: boolean;
};

export function createTaskSessionReplay(sessionId: number): TaskSessionReplayState {
  return { sessionId, cursor: 0, state: null, progress: null, events: [] };
}

export function applyTaskSessionEventPage(
  replay: TaskSessionReplayState,
  page: TaskSessionEventPage,
): TaskSessionReplayResult {
  let next = replay;
  for (const event of page.events) {
    if (event.session_id !== replay.sessionId) {
      throw new Error(
        `Task Session event ${event.sequence} belongs to ${event.session_id}, expected ${replay.sessionId}.`,
      );
    }
    if (event.sequence <= next.cursor) continue;
    if (event.sequence !== next.cursor + 1) return { replay: next, gapDetected: true };
    next = applyEvent(next, event);
  }
  return { replay: next, gapDetected: false };
}

function applyEvent(
  replay: TaskSessionReplayState,
  event: TaskSessionEvent,
): TaskSessionReplayState {
  return {
    ...replay,
    cursor: event.sequence,
    state: lifecycleState(event) ?? replay.state,
    progress: event.progress ?? replay.progress,
    events: [...replay.events, event],
  };
}

function lifecycleState(event: TaskSessionEvent): TaskSessionState | null {
  if (event.kind !== "lifecycle" || !isRecord(event.payload)) return null;
  const state = event.payload.state;
  return isTaskSessionState(state) ? state : null;
}

function isTaskSessionState(value: unknown): value is TaskSessionState {
  return (
    value === "queued" ||
    value === "running" ||
    value === "cancelling" ||
    value === "succeeded" ||
    value === "failed" ||
    value === "blocked" ||
    value === "cancelled"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
