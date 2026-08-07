import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const TASK_SESSION_UPDATE_EVENT = "task-session-update";
const taskSessionActivityHandlers = new Set<(update: TaskSessionUpdate) => void>();
let taskSessionActivityUnlisten: Promise<UnlistenFn> | null = null;

export type TaskSessionState =
  | "queued"
  | "running"
  | "cancelling"
  | "committing"
  | "succeeded"
  | "failed"
  | "blocked"
  | "cancelled";

/** Availability state exposed by the backend scheduler's shared health snapshot. */
export type SchedulerHealthStatus = "healthy" | "degraded" | "stopping" | "stopped";

/** Read-only scheduler health that does not depend on scheduler command responsiveness. */
export type SchedulerHealth = {
  status: SchedulerHealthStatus;
  last_error: string | null;
  last_error_at: number | null;
  consecutive_errors: number;
  pending_worker_completions: number;
  pending_projections: number;
};

export type AgentTaskResult = {
  summary: string;
  evidence: string[];
  details: string[];
  next: string[];
  completion_status: "completed" | "blocked";
  blocked_reason: string | null;
};

export type ChatTaskResult = { conversation_id: string; message: string };

export type EditTaskResult = { file_path: string; summary: string; content: string };

export type TaskExecutionOutput =
  | { kind: "none" }
  | { kind: "agent"; result: AgentTaskResult }
  | { kind: "chat"; result: ChatTaskResult }
  | { kind: "edit"; result: EditTaskResult };

export type TaskSessionResult = {
  session_id: number;
  output: TaskExecutionOutput;
  terminal_state: TaskSessionState;
  projection_error: string | null;
  projected_at: number | null;
  finalized_at: number | null;
};

export type TaskSessionEventKind = "lifecycle" | "activity" | "progress" | "runtime" | "tool";

export type TaskSessionKind = "agent" | "chat" | "edit";

export type TaskSessionEnvelopeV1Data = {
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

export type TaskSessionEnvelopeV1 = {
  schema_version: 1;
  session: TaskSessionEnvelopeV1Data;
};

export type TaskChatInputV2 = {
  kind: "chat";
  input: {
    message_id: string;
    message_sequence: number;
    message: string;
    terminal_context: string | null;
    session_context: string | null;
  };
};

export type TaskEditInputV2 = {
  kind: "edit";
  input: {
    file_path: string;
    instruction: string;
    content: string;
    selection: {
      start_line: number;
      start_character: number;
      end_line: number;
      end_character: number;
      text: string;
    } | null;
    context_files: Array<{ file_path: string; content: string }>;
    diagnostics: string[];
  };
};

export type TaskSessionInputV2 = TaskChatInputV2 | TaskEditInputV2;

export type TaskSessionEnvelopeV2 =
  | {
      schema_version: 2;
      session: {
        session: TaskSessionEnvelopeV1Data & { kind: "chat" };
        prompt_input: TaskChatInputV2;
      };
    }
  | {
      schema_version: 2;
      session: {
        session: TaskSessionEnvelopeV1Data & { kind: "edit" };
        prompt_input: TaskEditInputV2;
      };
    };

export type TaskSessionEnvelope = TaskSessionEnvelopeV1 | TaskSessionEnvelopeV2;

export type AgentRuntimeProfile = {
  id: string;
  runtime: "opencode";
  model: string;
  opencode_command: string;
  opencode_workdir: string | null;
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
  opencode_session_id: string | null;
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

export type TaskToolStatus = "running" | "succeeded" | "failed";

export type TaskToolCallState = {
  tool_call_id: string;
  tool_name: string;
  status: TaskToolStatus;
  risk: string | null;
  arguments_digest: string | null;
  display_context: unknown | null;
  attempt_id: number | null;
  fencing_token: number;
  started_sequence: number;
  completed_sequence: number | null;
  updated_at: number;
};

export type TaskToolState = {
  session_id: number;
  calls: TaskToolCallState[];
};

export type TaskMcpConnectorContext = {
  connector_id: string;
  capability: string;
  requested: boolean;
  granted: boolean;
};

export type TaskMcpContext = {
  session_id: number;
  workspace_id: string | null;
  runtime_profile_id: string | null;
  active_attempt_id: number | null;
  fencing_token: number;
  connectors: TaskMcpConnectorContext[];
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

/** Returns a collision-safe display identity for one attempt-scoped tool call. */
export function taskToolCallIdentity(call: TaskToolCallState): string {
  return JSON.stringify([call.attempt_id, call.fencing_token, call.tool_call_id]);
}

/** Lists durable Task Sessions currently retained by the scheduler projection. */
export function listTaskSessions(): Promise<TaskSessionSnapshot[]> {
  return invokeWithPolicy("list_task_sessions", {}, IPC_POLICIES.taskSessionRead);
}

/** Reads scheduler health directly from backend shared state. */
export function getSchedulerHealth(): Promise<SchedulerHealth> {
  return invokeWithPolicy("get_scheduler_health", {}, IPC_POLICIES.taskSessionRead);
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

/** Saves a content-addressed profile while rejecting conflicting content for an existing ID. */
export function saveImmutableAgentRuntimeProfile(
  profile: AgentRuntimeProfile,
): Promise<AgentRuntimeProfile> {
  return invokeWithPolicy(
    "save_immutable_agent_runtime_profile",
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

/** Requeues the same blocked Agent Task Session after structured UI approval. */
export function resumeTaskSessionAfterApproval(
  sessionId: number,
  label: string,
  envelope: TaskSessionEnvelope,
  grantedCapabilities: string[],
): Promise<TaskSessionSnapshot> {
  return invokeWithPolicy(
    "resume_task_session_after_approval",
    { sessionId, label, envelope, grantedCapabilities },
    IPC_POLICIES.taskSessionMutation,
  );
}

/** Returns the backend-canonical integrity digest for immutable Chat/Edit prompt input. */
export function digestTaskSessionPromptInput(input: TaskSessionInputV2): Promise<string> {
  return invokeWithPolicy(
    "digest_task_session_prompt_input",
    { input },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Returns the backend-canonical digest for an immutable Agent execution contract. */
export function digestAgentExecutionContract(contract: unknown): Promise<string> {
  return invokeWithPolicy(
    "digest_agent_execution_contract",
    { contract },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Requests cooperative cancellation for one queued or running Task Session. */
export function cancelTaskSession(sessionId: number): Promise<boolean> {
  return invokeWithPolicy("cancel_task_session", { sessionId }, IPC_POLICIES.taskSessionMutation);
}

/** Removes a retained Task Session projection after terminal completion. */
export function removeTaskSession(sessionId: number): Promise<boolean> {
  return invokeWithPolicy("remove_task_session", { sessionId }, IPC_POLICIES.taskSessionMutation);
}

/** Returns the latest durable projection for one Task Session, if it is still retained. */
export function getTaskSession(sessionId: number): Promise<TaskSessionSnapshot | null> {
  return invokeWithPolicy("get_task_session", { sessionId }, IPC_POLICIES.taskSessionRead);
}

/** Returns a staged or finalized authoritative typed result; available before terminal state. */
export function getTaskSessionResult(sessionId: number): Promise<TaskSessionResult | null> {
  return invokeWithPolicy("get_task_session_result", { sessionId }, IPC_POLICIES.taskSessionRead);
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

/** Returns current per-session tool state projected from the durable event journal. */
export function getTaskSessionToolState(sessionId: number): Promise<TaskToolState> {
  return invokeWithPolicy(
    "get_task_session_tool_state",
    { sessionId },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Returns non-secret per-session MCP connector context projected from envelope and grants. */
export function getTaskSessionMcpContext(sessionId: number): Promise<TaskMcpContext> {
  return invokeWithPolicy(
    "get_task_session_mcp_context",
    { sessionId },
    IPC_POLICIES.taskSessionRead,
  );
}

/**
 * Listens for best-effort post-commit Task Session hints through one shared Tauri listener.
 * Durable event replay remains authoritative because hints may be dropped or coalesced.
 */
export async function onTaskSessionActivity(
  handler: (update: TaskSessionUpdate) => void,
): Promise<UnlistenFn> {
  taskSessionActivityHandlers.add(handler);
  if (!taskSessionActivityUnlisten) {
    taskSessionActivityUnlisten = listen<unknown>(TASK_SESSION_UPDATE_EVENT, (event) => {
      if (!isTaskSessionUpdate(event.payload)) return;
      for (const subscriber of taskSessionActivityHandlers) subscriber(event.payload);
    });
  }
  try {
    await taskSessionActivityUnlisten;
  } catch (error) {
    taskSessionActivityHandlers.delete(handler);
    taskSessionActivityUnlisten = null;
    throw error;
  }
  let active = true;
  return () => {
    if (!active) return;
    active = false;
    taskSessionActivityHandlers.delete(handler);
    if (taskSessionActivityHandlers.size > 0 || !taskSessionActivityUnlisten) return;
    const unlisten = taskSessionActivityUnlisten;
    taskSessionActivityUnlisten = null;
    void unlisten.then((stop) => stop());
  };
}

/** Filters shared post-commit hints for one Task Session until explicitly unlistened. */
export function onTaskSessionUpdated(
  sessionId: number,
  handler: (update: TaskSessionUpdate) => void,
  initialSequence = 0,
): Promise<TaskSessionUpdateWatch> {
  let acknowledgedSequence = initialSequence;
  return onTaskSessionActivity((update) => {
    if (update.session_id === sessionId && update.latest_sequence > acknowledgedSequence) {
      handler(update);
    }
  }).then((unlisten) => ({
    unlisten,
    acknowledge(sequence: number) {
      acknowledgedSequence = Math.max(acknowledgedSequence, sequence);
    },
  }));
}

function isTaskSessionUpdate(value: unknown): value is TaskSessionUpdate {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.session_id === "number" &&
    Number.isSafeInteger(candidate.session_id) &&
    typeof candidate.latest_sequence === "number" &&
    Number.isSafeInteger(candidate.latest_sequence)
  );
}

/** Combines initial durable replay with shared post-commit hints for one Task Session timeline. */
export async function subscribeTaskSessionReplay(
  sessionId: number,
  afterSequence: number,
  handler: (update: TaskSessionUpdate) => void,
  limit = 100,
): Promise<TaskSessionSubscription> {
  let ready = false;
  let bufferedSequence = afterSequence;
  const watch = await onTaskSessionUpdated(
    sessionId,
    (update) => {
      bufferedSequence = Math.max(bufferedSequence, update.latest_sequence);
      if (ready) handler(update);
    },
    afterSequence,
  );
  try {
    const initialPage = await listTaskSessionEvents(sessionId, afterSequence, limit);
    ready = true;
    watch.acknowledge(initialPage.next_cursor);
    if (bufferedSequence > initialPage.next_cursor) {
      handler({ session_id: sessionId, latest_sequence: bufferedSequence });
    }
    return { initialPage, ...watch };
  } catch (error) {
    watch.unlisten();
    throw error;
  }
}
