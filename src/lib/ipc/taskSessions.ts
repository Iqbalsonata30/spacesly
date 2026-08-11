import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { recordIpcEvent } from "$lib/performance";

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
  objective_results: Array<{
    objective_id: string;
    completion_status: "completed" | "blocked";
    evidence: string[];
    blocked_reason: string | null;
  }>;
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
  governance_schema_version: number;
  skill_catalog: import("$lib/ipc/agent").AgentSkillRuntimeDefinition[];
};

export type GovernanceResolutionStatus = "authoritative" | "legacy_unavailable";

export type TaskGovernanceResolution = {
  schema_version: number;
  task_session_id: number;
  resolved_at: number;
  status: GovernanceResolutionStatus;
  rules: {
    normalization_version: string;
    final_digest: string;
    entries: Array<{
      rule_id: string;
      scope: "platform" | "global" | "workspace" | "task";
      source: string;
      revision: string;
      precedence: number;
      digest: string;
    }>;
    snapshot: string;
  };
  skills: {
    catalog_revision: string | null;
    selected_skill_ids: string[];
    entries: Array<{
      skill_id: string;
      selected: boolean;
      trigger: string;
      matched_domains: string[];
      matched_intents: string[];
      priority: number;
      reason: string;
      selection_order: number | null;
    }>;
    snapshot: string;
  };
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

export type ResourceMutationState =
  "reserved" | "succeeded" | "failed" | "uncertain" | "superseded";

export type ResourceOperationIdentity = {
  schema_version: number;
  connector: string;
  operation: string;
  resource: {
    api_version: string;
    kind: string;
    namespace: string | null;
    name: string;
  };
  environment_fingerprint: string;
  mutation_fingerprint: string;
  key: string;
};

export type ResourceMutationEvidence = {
  identity: ResourceOperationIdentity;
  lookup: {
    status: "already_satisfied" | "drift_detected" | "incompatible" | "unavailable";
    observed_fingerprint: string | null;
    observed_version: string | null;
  };
  execution: {
    status: "executed" | "skipped" | "blocked" | "conflict";
    resulting_fingerprint: string | null;
    resulting_version: string | null;
  };
  retry_resume_status:
    | "first_execution"
    | "already_complete"
    | "reconciled_after_drift"
    | "awaiting_approval"
    | "awaiting_operator"
    | "conflict";
};

export type ResourceMutationRecord = {
  mutation_id: number;
  operation_key: string;
  identity: ResourceOperationIdentity;
  connector_id: string;
  tool_name: string;
  state: ResourceMutationState;
  session_id: number;
  attempt_id: number;
  attempt: number;
  fencing_token: number;
  evidence: ResourceMutationEvidence | null;
  failure_kind: string | null;
  failure_code: string | null;
  revision: number;
  reserved_at: number;
  resolved_at: number | null;
  superseded_at: number | null;
  supersede_reason: string | null;
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

export type TaskExecutionTraceEntry = {
  sequence: number;
  attempt_id: number | null;
  assignment_attempt: number | null;
  fencing_token: number;
  event_type: string;
  created_at: number;
  state: string | null;
  stage: string | null;
  duration_us: number | null;
  outcome: string | null;
  worker_id: number | null;
  runtime_id: string | null;
  opencode_session_id: string | null;
  tool_call_id: string | null;
  tool_name: string | null;
  tool_success: boolean | null;
  input_tokens: number | null;
  output_tokens: number | null;
  recovery: string | null;
  approval_operation: string | null;
};

export type TaskExecutionTracePage = {
  schema_version: number;
  trace_id: string;
  task_session_id: number;
  subject_id: string | null;
  execution_run_id: string | null;
  runtime_profile_id: string | null;
  model: string | null;
  opencode_session_id: string | null;
  coverage: "complete" | "partial";
  unknown_fields: string[];
  entries: TaskExecutionTraceEntry[];
  next_cursor: number;
  has_more: boolean;
};

export type ContextContributionKind =
  | "system_instructions"
  | "rules"
  | "skills"
  | "task"
  | "workspace"
  | "external"
  | "tool_definitions"
  | "conversation";

export type ContextContribution = {
  kind: ContextContributionKind;
  source: string;
  revision: string | null;
  digest: string | null;
  stored_content_bytes: number | null;
  estimated_tokens: number | null;
  token_measurement: "chars_div4_estimate" | "unavailable";
  item_count: number | null;
  note: string;
};

export type TaskContextInspection = {
  schema_version: number;
  task_session_id: number;
  status: "partial" | "legacy_unavailable" | "corrupt";
  identity: {
    kind: TaskSessionKind | null;
    state: TaskSessionState;
    workspace_id: string | null;
    subject_id: string | null;
    conversation_id: string | null;
    execution_run_id: string | null;
    runtime_profile_id: string | null;
    model: string | null;
    prompt_template_version: string | null;
    context_digest: string | null;
    context_revision: string | null;
    rules_revision: string | null;
    skills_revision: string | null;
    opencode_session_id: string | null;
  };
  known_stored_content_bytes: number;
  known_estimated_tokens: number;
  total_is_partial: boolean;
  contributions: ContextContribution[];
  rules: {
    status: string;
    normalization_version: string | null;
    final_digest: string | null;
    entries: Array<{
      rule_id: string;
      scope: "platform" | "global" | "workspace" | "task";
      source: string;
      revision: string;
      precedence: number;
      digest: string;
    }>;
    truncated: boolean;
  };
  skills: {
    status: string;
    catalog_revision: string | null;
    selected_skill_ids: string[];
    entries: Array<{
      skill_id: string;
      selected: boolean;
      trigger: string;
      matched_domains: string[];
      matched_intents: string[];
      priority: number;
      reason: string;
      selection_order: number | null;
    }>;
    truncated: boolean;
  };
  connectors: TaskMcpConnectorContext[];
  unknown_fields: string[];
};

export type TaskExecutionManifest = {
  schema_version: number;
  task_session_id: number;
  assignment_attempt_id: number;
  assignment_attempt: number;
  worker_id: number;
  fencing_token: number;
  started_at: number;
  kind: TaskSessionKind;
  workspace_id: string;
  subject_id: string | null;
  conversation_id: string | null;
  execution_run_id: string | null;
  context_digest: string;
  context_revision: string | null;
  runtime: string;
  runtime_profile_id: string;
  runtime_id: string;
  model: string;
  model_configuration: {
    provider_id: string;
    api_style: string;
    temperature: string;
  };
  prompt_template_version: string;
  rules_revision: string | null;
  skills_revision: string | null;
  rules: TaskGovernanceResolution["rules"]["entries"];
  rules_digest: string;
  rule_facts: {
    schema_version: number;
    compiler_version: string;
    source_digest: string;
    repositories: Array<{
      id: string;
      remote_url: string;
      local_path: string | null;
      backend_path: string | null;
      frontend_path: string | null;
    }>;
    protected_branches: Array<{
      branches: string[];
      operations: string[];
      approval_required: boolean;
    }>;
    deployment_targets: Array<{
      label: string;
      target: string;
      branch: string;
      namespace: string;
    }>;
    warnings: string[];
  };
  task_examination: {
    schema_version: number;
    examiner_version: string;
    contract_digest: string;
    status: "ready" | "blocked";
    objectives: string[];
    resources: Array<{ kind: string; value: string; source: string }>;
    capability_catalog: Array<{
      capability: string;
      provider: string;
      connector_id: string | null;
      discovery: string;
      granted: boolean;
    }>;
    connector_capabilities: Array<{
      connector_id: string;
      status: "declared" | "available" | "unavailable";
      tools: Array<{ name: string; risk: string; argument_names: string[] }>;
      error: string | null;
      warnings: string[];
    }>;
    capability_mappings: Array<{
      connector_id: string;
      reason: string;
      planned_tools: string[];
      verified_tools: string[];
      status: "declared" | "connector_verified" | "tools_verified" | "stale";
    }>;
    semantic_planner: {
      status: "model" | "fallback";
      planner_version: string;
      model: string | null;
      objective_count: number;
    } | null;
    objective_checkpoints?: Array<{
      objective_id: string;
      evidence: string[];
      tool_receipts: Array<{
        tool_call_id: string;
        tool_name: string;
        risk: string;
        arguments_digest: string;
      }>;
      source_attempt_id: number;
      recorded_at: number;
    }>;
    required_capabilities: string[];
    unresolved_requirements: string[];
    mutations: string[];
    approval_boundaries: string[];
    warnings: string[];
  };
  skills_catalog_revision: string | null;
  skills: TaskGovernanceResolution["skills"]["entries"];
  connectors: TaskMcpConnectorContext[];
  tool_permission_mode: string;
  unknown_fields: string[];
  opencode_session_id: string | null;
  coverage: "partial";
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

/** Returns the persisted backend-authoritative Rules and Skills resolution for one session. */
export function getTaskSessionGovernance(
  sessionId: number,
): Promise<TaskGovernanceResolution | null> {
  return invokeWithPolicy(
    "get_task_session_governance",
    { sessionId },
    IPC_POLICIES.taskSessionRead,
  );
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

/** Continues the same interrupted Agent Task Session and durable OpenCode session. */
export function continueInterruptedTaskSession(
  sessionId: number,
  label: string,
  envelope: TaskSessionEnvelope,
  grantedCapabilities: string[],
): Promise<TaskSessionSnapshot> {
  return invokeWithPolicy(
    "continue_interrupted_task_session",
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

/** Returns secret-free resource mutation ledger history for one Task Session. */
export function listTaskSessionResourceMutations(
  sessionId: number,
): Promise<ResourceMutationRecord[]> {
  return invokeWithPolicy(
    "list_task_session_resource_mutations",
    { sessionId },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Explicitly releases one retained fence; this mutation is never retried automatically. */
export function supersedeTaskSessionResourceMutation(
  sessionId: number,
  mutationId: number,
  expectedKey: string,
  expectedRevision: number,
  reason: string,
): Promise<ResourceMutationRecord> {
  return invokeWithPolicy(
    "supersede_task_session_resource_mutation",
    { sessionId, mutationId, expectedKey, expectedRevision, reason },
    IPC_POLICIES.taskSessionMutation,
  );
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

/** Reads a safe, indexed, bounded developer execution trace after a raw journal cursor. */
export function listTaskSessionExecutionTrace(
  sessionId: number,
  afterSequence = 0,
  limit = 100,
): Promise<TaskExecutionTracePage> {
  return invokeWithPolicy(
    "list_task_session_execution_trace",
    { sessionId, afterSequence, limit },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Returns a safe metadata-only projection of the context available to one Task Session. */
export function getTaskSessionContextInspection(sessionId: number): Promise<TaskContextInspection> {
  return invokeWithPolicy(
    "get_task_session_context_inspection",
    { sessionId },
    IPC_POLICIES.taskSessionRead,
  );
}

/** Returns the latest durable, assignment-fenced Execution Manifest for one Task Session. */
export function getTaskSessionExecutionManifest(
  sessionId: number,
): Promise<TaskExecutionManifest | null> {
  return invokeWithPolicy(
    "get_task_session_execution_manifest",
    { sessionId },
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
      recordIpcEvent(TASK_SESSION_UPDATE_EVENT, event.payload);
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
