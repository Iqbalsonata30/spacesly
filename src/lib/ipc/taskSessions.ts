import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

const TASK_SESSION_POLL_INTERVAL_MS = 500;

export type TaskSessionState =
  "queued" | "running" | "cancelling" | "succeeded" | "failed" | "cancelled";

export type TaskSessionEventKind = "lifecycle" | "activity" | "progress" | "runtime" | "tool";

export type TaskProgress = {
  phase: string;
  completed: number;
  total: number | null;
};

export type TaskRequest = {
  label: string;
  payload: string;
};

export type TaskSessionSnapshot = {
  id: number;
  request: TaskRequest;
  state: TaskSessionState;
  worker_id: number | null;
  dispatch_sequence: number | null;
  attempt: number;
  attempt_id: number | null;
  fencing_token: number;
  lease_expires_at: number | null;
  progress: TaskProgress | null;
  last_event_sequence: number;
  error: string | null;
  created_at: number;
  started_at: number | null;
  completed_at: number | null;
};

export type TaskSessionEvent = {
  id: number;
  session_id: number;
  attempt_id: number | null;
  fencing_token: number;
  sequence: number;
  kind: TaskSessionEventKind;
  payload: unknown;
  progress: TaskProgress | null;
  created_at: number;
};

export type TaskSessionEventPage = {
  events: TaskSessionEvent[];
  next_cursor: number;
  has_more: boolean;
};

export type TaskSessionUpdate = {
  session_id: number;
  latest_sequence: number;
};

export type TaskSessionSubscription = {
  initialPage: TaskSessionEventPage;
  unlisten: () => void;
  acknowledge: (sequence: number) => void;
};

export type TaskSessionUpdateWatch = {
  unlisten: () => void;
  acknowledge: (sequence: number) => void;
};

export function listTaskSessions(): Promise<TaskSessionSnapshot[]> {
  return invokeWithPolicy("list_task_sessions", {}, IPC_POLICIES.taskSessionRead);
}

export function getTaskSession(sessionId: number): Promise<TaskSessionSnapshot | null> {
  return invokeWithPolicy("get_task_session", { sessionId }, IPC_POLICIES.taskSessionRead);
}

export function listTaskSessionEvents(
  sessionId: number,
  afterSequence: number,
  limit = 100,
): Promise<TaskSessionEventPage> {
  return invokeWithPolicy(
    "list_task_session_events",
    { sessionId, afterSequence, limit },
    IPC_POLICIES.taskSessionRead,
  );
}

export function onTaskSessionUpdated(
  sessionId: number,
  handler: (update: TaskSessionUpdate) => void,
  initialSequence = 0,
): Promise<TaskSessionUpdateWatch> {
  let active = true;
  let polling = false;
  let acknowledgedSequence = initialSequence;
  const poll = async (): Promise<void> => {
    if (!active || polling) return;
    polling = true;
    try {
      const snapshot = await getTaskSession(sessionId);
      if (active && snapshot && snapshot.last_event_sequence > acknowledgedSequence) {
        handler({ session_id: sessionId, latest_sequence: snapshot.last_event_sequence });
      }
    } catch {
      // Durable replay on the next successful poll remains authoritative.
    } finally {
      polling = false;
    }
  };
  const interval = setInterval(() => void poll(), TASK_SESSION_POLL_INTERVAL_MS);
  void poll();
  return Promise.resolve({
    unlisten: () => {
      active = false;
      clearInterval(interval);
    },
    acknowledge: (sequence: number) => {
      acknowledgedSequence = Math.max(acknowledgedSequence, sequence);
    },
  });
}

export async function subscribeTaskSessionReplay(
  sessionId: number,
  afterSequence: number,
  handler: (update: TaskSessionUpdate) => void,
  limit = 100,
): Promise<TaskSessionSubscription> {
  let ready = false;
  const watch = await onTaskSessionUpdated(
    sessionId,
    (update) => {
      if (ready) handler(update);
    },
    afterSequence,
  );
  try {
    const initialPage = await listTaskSessionEvents(sessionId, afterSequence, limit);
    ready = true;
    return { initialPage, ...watch };
  } catch (error) {
    watch.unlisten();
    throw error;
  }
}
