import type { AiWorkerConfig, AiWorkerTaskResult, ExecutionContract } from "$lib/ipc/agent";
import { appendConversationMessage } from "$lib/ipc/agent";
import {
  cancelTaskSession,
  digestAgentExecutionContract,
  getTaskSession,
  getTaskSessionResult,
  listTaskSessionEvents,
  onTaskSessionUpdated,
  saveImmutableAgentRuntimeProfile,
  submitTaskSession,
  type AgentRuntimeProfile,
  type TaskSessionEnvelopeV1,
  type TaskSessionEvent,
  type TaskSessionResult,
  type TaskSessionSnapshot,
  type TaskSessionState,
} from "$lib/ipc/taskSessions";

/** Immutable prompt/template revision for production OpenCode Agent Task Sessions. */
export const AGENT_TASK_TEMPLATE_VERSION = "agent-task-v1";

const DEFAULT_TIMEOUT_MS = 10 * 60_000;
const BUILTIN_CAPABILITIES = ["workspace_read", "workspace_write", "shell", "git"];

/** Immutable profile identity and authority requested by one Agent submission. */
export type AgentTaskProfileBinding = {
  runtimeProfileId: string;
  model: string;
  connectorIds: string[];
  capabilities: string[];
  rulesRevision: string;
  skillsRevision: string;
};

/** Durable conversation context and envelope prepared before scheduler submission. */
export type PreparedAgentTaskSession = {
  conversationId: string;
  contextMessageId: string;
  envelope: TaskSessionEnvelopeV1;
  grantedCapabilities: string[];
};

/** Terminal Task Session projection paired with its authoritative Agent result. */
export type AgentTaskSessionExecution = {
  session: TaskSessionSnapshot;
  result: AiWorkerTaskResult;
};

/** Injectable IPC boundary used by the Task Session orchestration and pure tests. */
export type AgentTaskSessionDependencies = {
  appendMessage: typeof appendConversationMessage;
  cancel: typeof cancelTaskSession;
  digestContract: typeof digestAgentExecutionContract;
  getResult: typeof getTaskSessionResult;
  getSession: typeof getTaskSession;
  listEvents: typeof listTaskSessionEvents;
  saveProfile: typeof saveImmutableAgentRuntimeProfile;
  submit: typeof submitTaskSession;
  watch: typeof onTaskSessionUpdated;
};

/** Replay callbacks, timeout, and optional dependency overrides for one execution wait. */
export type ExecuteAgentTaskSessionOptions = {
  timeoutMs?: number;
  cancellationTimeoutMs?: number;
  onEvent?: (event: TaskSessionEvent) => void;
  onSubmitted?: (session: TaskSessionSnapshot) => void;
  dependencies?: Partial<AgentTaskSessionDependencies>;
};

/** Error raised after a Task Session wait times out and cooperative cancellation is requested. */
export class AgentTaskSessionTimeoutError extends Error {
  readonly sessionId: number;
  readonly cancellationRequested: boolean;
  readonly cancelled: boolean;
  readonly terminalState: TaskSessionState | null;

  constructor(
    sessionId: number,
    timeoutMs: number,
    cancellationRequested: boolean,
    terminalState: TaskSessionState | null,
  ) {
    super(
      terminalState
        ? `Agent Task Session ${sessionId} timed out after ${timeoutMs}ms and ended as ${terminalState}.`
        : `Agent Task Session ${sessionId} timed out after ${timeoutMs}ms; cancellation remains unconfirmed.`,
    );
    this.name = "AgentTaskSessionTimeoutError";
    this.sessionId = sessionId;
    this.cancellationRequested = cancellationRequested;
    this.cancelled = terminalState === "cancelled";
    this.terminalState = terminalState;
  }
}

const defaultDependencies: AgentTaskSessionDependencies = {
  appendMessage: appendConversationMessage,
  cancel: cancelTaskSession,
  digestContract: digestAgentExecutionContract,
  getResult: getTaskSessionResult,
  getSession: getTaskSession,
  listEvents: listTaskSessionEvents,
  saveProfile: saveImmutableAgentRuntimeProfile,
  submit: submitTaskSession,
  watch: onTaskSessionUpdated,
};

/** Returns sorted connector IDs and the exact capability grant set owned by those connectors. */
export function agentTaskCapabilities(config: AiWorkerConfig): {
  connectorIds: string[];
  capabilities: string[];
} {
  const connectorIds = config.mcp_servers.map((server) => server.secret_id.trim());
  if (connectorIds.some((id) => !id) || new Set(connectorIds).size !== connectorIds.length) {
    throw new Error("Agent Task Session connector IDs must be non-empty and unique.");
  }
  connectorIds.sort();
  return {
    connectorIds,
    capabilities: [...BUILTIN_CAPABILITIES, ...connectorIds.map((id) => `external_tools:${id}`)],
  };
}

/** Saves and returns a content-addressed immutable OpenCode Agent profile. */
export async function ensureOpenCodeAgentProfile(
  config: AiWorkerConfig,
  dependencies: Partial<AgentTaskSessionDependencies> = {},
): Promise<AgentTaskProfileBinding> {
  if (config.runtime !== "opencode") {
    throw new Error("Agent Task Sessions require the OpenCode runtime.");
  }
  const deps = { ...defaultDependencies, ...dependencies };
  const { connectorIds, capabilities } = agentTaskCapabilities(config);
  const rulesRevision = `sha256:${await sha256(config.agent_rules)}`;
  const skillsRevision = `sha256:${await sha256(config.agent_skills)}`;
  const identity = await sha256(
    JSON.stringify({
      runtime: "opencode",
      model: config.opencode_model.trim(),
      opencodeCommand: config.opencode_command.trim(),
      rulesRevision,
      skillsRevision,
      temperature: config.temperature,
      connectorIds,
      promptTemplateVersion: AGENT_TASK_TEMPLATE_VERSION,
    }),
  );
  const profile: AgentRuntimeProfile = {
    id: `agent-${identity}`,
    runtime: "opencode",
    model: config.opencode_model.trim(),
    opencode_command: config.opencode_command.trim(),
    agent_rules: config.agent_rules,
    agent_skills: config.agent_skills,
    temperature: config.temperature,
    connector_ids: connectorIds,
    prompt_template_version: AGENT_TASK_TEMPLATE_VERSION,
    rules_revision: rulesRevision,
    skills_revision: skillsRevision,
  };
  await deps.saveProfile(profile);
  return {
    runtimeProfileId: profile.id,
    model: profile.model,
    connectorIds,
    capabilities,
    rulesRevision,
    skillsRevision,
  };
}

/** Returns the durable conversation ID shared by every continuation of one workspace card. */
export async function agentConversationId(workspaceId: string, cardId: string): Promise<string> {
  return `agent-card-${await sha256(JSON.stringify([workspaceId, cardId]))}`;
}

/** Creates the durable context message and V1 envelope required to submit an Agent Task Session. */
export async function prepareAgentTaskSession(
  config: AiWorkerConfig,
  cardId: string,
  cardTitle: string,
  executionRunId: string,
  contract: ExecutionContract,
  workspaceRevision: number,
  dependencies: Partial<AgentTaskSessionDependencies> = {},
): Promise<PreparedAgentTaskSession> {
  const deps = { ...defaultDependencies, ...dependencies };
  const profile = await ensureOpenCodeAgentProfile(config, deps);
  const contextDigest = await deps.digestContract(contract);
  const conversationId = await agentConversationId(config.workspace_id, cardId);
  const contextMessageId = `agent-context-${contextDigest.replace(/^sha256:/, "")}`;
  await deps.appendMessage(config.workspace_id, conversationId, cardTitle, {
    id: contextMessageId,
    role: "user",
    text: JSON.stringify({
      schema_version: 1,
      type: "agent_execution_context",
      execution_contract: contract,
    }),
  });
  const envelope: TaskSessionEnvelopeV1 = {
    schema_version: 1,
    session: {
      workspace_id: config.workspace_id,
      kind: "agent",
      subject_id: cardId,
      conversation_id: conversationId,
      execution_run_id: executionRunId,
      context_digest: contextDigest,
      runtime_profile_id: profile.runtimeProfileId,
      model: profile.model,
      connector_ids: profile.connectorIds,
      requested_capabilities: profile.capabilities,
      prompt_template_version: AGENT_TASK_TEMPLATE_VERSION,
      context_revision: workspaceRevision.toString(),
      rules_revision: profile.rulesRevision,
      skills_revision: profile.skillsRevision,
    },
  };
  return { conversationId, contextMessageId, envelope, grantedCapabilities: profile.capabilities };
}

/** Submits an Agent Task Session and resolves from its authoritative staged result. */
export async function executeAgentTaskSession(
  label: string,
  prepared: PreparedAgentTaskSession,
  options: ExecuteAgentTaskSessionOptions = {},
): Promise<AgentTaskSessionExecution> {
  const deps = { ...defaultDependencies, ...options.dependencies };
  const submitted = await deps.submit(label, prepared.envelope, prepared.grantedCapabilities);
  options.onSubmitted?.(submitted);
  return waitForAgentTaskSession(submitted.id, { ...options, dependencies: deps });
}

/** Replays one retained Agent Task Session without resubmitting it. */
export async function waitForAgentTaskSession(
  sessionId: number,
  options: ExecuteAgentTaskSessionOptions = {},
): Promise<AgentTaskSessionExecution> {
  const deps = { ...defaultDependencies, ...options.dependencies };
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  let cursor = 0;
  let terminalAttemptId: number | null = null;
  let terminalFencingToken: number | null = null;
  let candidateAttemptId: number | null = null;
  let hintedSequence = 0;
  let generation = 0;
  let wake: (() => void) | null = null;
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    wake?.();
    wake = null;
  }, timeoutMs);
  const watch = await deps
    .watch(sessionId, (update) => {
      hintedSequence = Math.max(hintedSequence, update.latest_sequence);
      generation += 1;
      wake?.();
      wake = null;
    })
    .catch((reason) => {
      clearTimeout(timer);
      throw reason;
    });

  try {
    while (true) {
      if (timedOut) {
        timedOut = false;
        const cancellationRequested = await deps.cancel(sessionId).catch(() => false);
        const deadline = Date.now() + (options.cancellationTimeoutMs ?? 5_000);
        let terminalState: TaskSessionState | null = null;
        while (Date.now() < deadline) {
          const cancellingSnapshot = await deps.getSession(sessionId).catch(() => null);
          if (!cancellingSnapshot) break;
          if (isTerminal(cancellingSnapshot.state)) {
            terminalState = cancellingSnapshot.state;
            break;
          }
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        throw new AgentTaskSessionTimeoutError(
          sessionId,
          timeoutMs,
          cancellationRequested,
          terminalState,
        );
      }
      let page;
      do {
        page = await deps.listEvents(sessionId, cursor, 500);
        for (const event of page.events) {
          if (event.session_id !== sessionId) {
            throw new Error("Task Session replay contained an event from another session.");
          }
          if (event.sequence <= cursor) continue;
          if (event.sequence !== cursor + 1) {
            throw new Error(`Task Session event gap after sequence ${cursor}.`);
          }
          cursor = event.sequence;
          options.onEvent?.(event);
          if (agentTaskCandidateFromEvent(event)) candidateAttemptId = event.attempt_id;
          const state = lifecycleState(event);
          if (state && isTerminal(state)) {
            terminalAttemptId = event.attempt_id;
            terminalFencingToken = event.fencing_token;
          }
        }
      } while (page.has_more);
      watch.acknowledge(cursor);

      const snapshot = await deps.getSession(sessionId);
      if (!snapshot) throw new Error("Task Session was removed before completion.");
      if (isTerminal(snapshot.state)) {
        if (cursor < snapshot.last_event_sequence) continue;
        const authoritative = await deps.getResult(sessionId);
        return validateAgentTaskSessionResult(
          snapshot,
          authoritative,
          terminalAttemptId,
          terminalFencingToken,
          candidateAttemptId,
        );
      }
      if (hintedSequence <= cursor) {
        const observedGeneration = generation;
        await new Promise<void>((resolve) => {
          const reconcile = setTimeout(resolveWake, 1_000);
          function resolveWake() {
            clearTimeout(reconcile);
            if (wake === resolveWake) wake = null;
            resolve();
          }
          wake = resolveWake;
          if (generation !== observedGeneration || hintedSequence > cursor || timedOut)
            resolveWake();
        });
      }
    }
  } catch (reason) {
    if (!(reason instanceof AgentTaskSessionTimeoutError)) {
      const snapshot = await deps.getSession(sessionId).catch(() => null);
      if (snapshot && !isTerminal(snapshot.state)) {
        await deps.cancel(sessionId).catch(() => false);
      }
    }
    throw reason;
  } finally {
    clearTimeout(timer);
    watch.unlisten();
  }
}

/** Parses a fenced, non-authoritative Agent candidate for diagnostics and attempt validation only. */
export function agentTaskCandidateFromEvent(event: TaskSessionEvent): AiWorkerTaskResult | null {
  if (
    event.kind !== "runtime" ||
    event.attempt_id === null ||
    !isRecord(event.payload) ||
    event.payload.type !== "agent_result_candidate" ||
    event.payload.authoritative !== false ||
    !isAgentTaskResult(event.payload.result)
  ) {
    return null;
  }
  return event.payload.result;
}

/** Validates an authoritative result against the exact terminal assignment attempt. */
export function validateAgentTaskSessionResult(
  snapshot: TaskSessionSnapshot,
  authoritative: TaskSessionResult | null,
  terminalAttemptId: number | null,
  terminalFencingToken: number | null,
  candidateAttemptId: number | null,
): AgentTaskSessionExecution {
  if (!isTerminal(snapshot.state)) throw new Error("Task Session is not terminal.");
  if (
    snapshot.attempt_id === null ||
    terminalAttemptId !== snapshot.attempt_id ||
    terminalFencingToken !== snapshot.fencing_token
  ) {
    throw new Error("Task Session terminal event did not match its active assignment attempt.");
  }
  if (candidateAttemptId !== null && candidateAttemptId !== snapshot.attempt_id) {
    throw new Error("Task Session result candidate came from a stale assignment attempt.");
  }
  if (
    !authoritative ||
    authoritative.session_id !== snapshot.id ||
    authoritative.terminal_state !== snapshot.state
  ) {
    throw new Error("Task Session authoritative result did not match its terminal projection.");
  }
  if (authoritative.output.kind !== "agent") {
    throw new Error("Task Session did not stage an authoritative Agent result.");
  }
  return { session: snapshot, result: authoritative.output.result };
}

/** Extracts the V1 Agent envelope from a retained scheduler request for conservative recovery. */
export function agentEnvelopeFromSnapshot(
  snapshot: TaskSessionSnapshot,
): TaskSessionEnvelopeV1["session"] | null {
  try {
    const parsed = JSON.parse(snapshot.request.payload) as unknown;
    if (!isRecord(parsed) || parsed.schema_version !== 1 || !isRecord(parsed.session)) return null;
    if (parsed.session.kind !== "agent") return null;
    return parsed.session as TaskSessionEnvelopeV1["session"];
  } catch {
    return null;
  }
}

function lifecycleState(event: TaskSessionEvent): TaskSessionState | null {
  if (event.kind !== "lifecycle" || !isRecord(event.payload)) return null;
  return typeof event.payload.state === "string" && isTaskSessionState(event.payload.state)
    ? event.payload.state
    : null;
}

function isTerminal(state: TaskSessionState): boolean {
  return (
    state === "succeeded" || state === "failed" || state === "blocked" || state === "cancelled"
  );
}

function isTaskSessionState(value: string): value is TaskSessionState {
  return [
    "queued",
    "running",
    "cancelling",
    "committing",
    "succeeded",
    "failed",
    "blocked",
    "cancelled",
  ].includes(value);
}

function isAgentTaskResult(value: unknown): value is AiWorkerTaskResult {
  if (!isRecord(value)) return false;
  return (
    typeof value.summary === "string" &&
    isStringArray(value.evidence) &&
    isStringArray(value.details) &&
    isStringArray(value.next) &&
    (value.completion_status === "completed" || value.completion_status === "blocked") &&
    (value.blocked_reason === null || typeof value.blocked_reason === "string")
  );
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function sha256(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
