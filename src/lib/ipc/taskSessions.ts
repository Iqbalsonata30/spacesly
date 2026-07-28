import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

const TASK_SESSION_POLL_INTERVAL_MS = 500;

export type TaskSessionState =
  "queued" | "running" | "cancelling" | "succeeded" | "failed" | "blocked" | "cancelled";

export type TaskSessionEventKind = "lifecycle" | "activity" | "progress" | "runtime" | "tool";

export type TaskSessionKind = "agent" | "chat" | "edit";

export type TaskSessionEnvelope = {
  schema_version: 1;
  session: {
    workspace_id: string;
    kind: TaskSessionKind;
    subject_id: string | null;
    conversation_id: string | null;
    execution_run_id: string | null;
    context_digest: string;
    runtime_profile_id: string;
    model: string;
    connector_ids: string[];
    requested_capabilities: string[];
    prompt_template_version: string;
    context_revision: string | null;
    rules_revision: string | null;
    skills_revision: string | null;
  };
};

export type AgentRuntimeProfile = {
  id: string;
  runtime: "opencode";
  model: string;
  opencode_command: string;
  agent_rules: string;
  agent_skills: string;
  temperature: number;
  connector_ids: string[];
  prompt_template_version: string;
  rules_revision: string;
  skills_revision: string;
};

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

/** Lists durable Task Sessions currently retained by the scheduler projection. */
export function listTaskSessions(): Promise<TaskSessionSnapshot[]> {
  return invokeWithPolicy("list_task_sessions", {}, IPC_POLICIES.taskSessionRead);
}

/** Lists non-secret Agent runtime profiles available for future Task Session submissions. */
export function listAgentRuntimeProfiles(): Promise<AgentRuntimeProfile[]> {
  return invokeWithPolicy("list_agent_runtime_profiles", {}, IPC_POLICIES.taskSessionRead);
}

/** Saves one non-secret Agent runtime profile; provider/MCP secrets remain in the secrets store. */
export function saveAgentRuntimeProfile(
  profile: AgentRuntimeProfile,
): Promise<AgentRuntimeProfile> {
  return invokeWithPolicy(
    "save_agent_runtime_profile",
    { profile },
    IPC_POLICIES.taskSessionMutation,
  );
}

/** Submits one Agent Task Session with explicit capability grants; this mutation is never retried. */
export function submitTaskSession(
  label: string,
  envelope: TaskSessionEnvelope,
  grantedCapabilities: string[],
): Promise<TaskSessionSnapshot> {
  return invokeWithPolicy(
    "submit_task_session",
    { label, envelope, grantedCapabilities },
    IPC_POLICIES.taskSessionMutation,
  );
}

/** Requests cooperative cancellation for one queued or running Task Session. */
export function cancelTaskSession(sessionId: number): Promise<boolean> {
  return invokeWithPolicy(
    "cancel_task_session",
    { sessionId },
    IPC_POLICIES.taskSessionMutation,
  );
}

/** Removes a retained Task Session projection after terminal completion. */
export function removeTaskSession(sessionId: number): Promise<boolean> {
  return invokeWithPolicy(
    "remove_task_session",
    { sessionId },
    IPC_POLICIES.taskSessionMutation,
  );
}

/** Returns the latest durable projection for one Task Session, if it is still retained. */
export function getTaskSession(sessionId: number): Promise<TaskSessionSnapshot | null> {
  return invokeWithPolicy("get_task_session", { sessionId }, IPC_POLICIES.taskSessionRead);
}

/** Replays a bounded page of durable Task Session events after a monotonic sequence cursor. */
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

/** Polls one Task Session projection and emits update hints until explicitly unlistened. */
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

/** Combines initial durable replay with update polling for one Task Session timeline. */
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
