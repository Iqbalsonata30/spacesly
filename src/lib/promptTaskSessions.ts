import type { AiWorkerConfig } from "$lib/ipc/agent";
import {
  digestTaskSessionPromptInput,
  getTaskSession,
  getTaskSessionResult,
  listTaskSessionEvents,
  onTaskSessionUpdated,
  saveImmutableAgentRuntimeProfile,
  submitTaskSession,
  type AgentRuntimeProfile,
  type TaskSessionEnvelopeV1Data,
  type TaskSessionEnvelopeV2,
  type TaskSessionEvent,
  type TaskSessionInputV2,
  type TaskSessionSnapshot,
  type ChatTaskResult,
  type EditTaskResult,
} from "$lib/ipc/taskSessions";

export const PROMPT_TASK_TEMPLATE_VERSION = "prompt-task-v2";

export type PromptTaskProfileBinding = {
  runtimeProfileId: string;
  model: string;
  rulesRevision: string;
  skillsRevision: string;
};

export type PromptTaskCandidate =
  | {
      kind: "chat";
      attemptId: number;
      conversationId: string;
      message: string;
    }
  | {
      kind: "edit";
      attemptId: number;
      filePath: string;
      summary: string;
      content: string;
    };

export type PromptTaskExecution = {
  session: TaskSessionSnapshot;
  result: ChatTaskResult | EditTaskResult;
};

/** Saves an immutable content-addressed OpenCode profile for a prompt Task Session. */
export async function ensureOpenCodePromptProfile(
  config: AiWorkerConfig,
): Promise<PromptTaskProfileBinding> {
  if (config.runtime !== "opencode") {
    throw new Error("Task Session prompt profiles currently require OpenCode.");
  }
  const opencodeCommand = config.opencode_command.trim();
  const opencodeModel = config.opencode_model.trim();
  const opencodeWorkdir = config.opencode_workdir?.trim() || null;
  const rulesRevision = `sha256:${await sha256(config.agent_rules)}`;
  const skillsRevision = `sha256:${await sha256(config.agent_skills)}`;
  const profileIdentity = await sha256(
    JSON.stringify({
      runtime: config.runtime,
      model: opencodeModel,
      command: opencodeCommand,
      opencodeWorkdir,
      rulesRevision,
      skillsRevision,
      temperature: config.temperature,
      promptTemplateVersion: PROMPT_TASK_TEMPLATE_VERSION,
    }),
  );
  const profile: AgentRuntimeProfile = {
    id: `prompt-${profileIdentity}`,
    runtime: "opencode",
    model: opencodeModel,
    opencode_command: opencodeCommand,
    opencode_workdir: opencodeWorkdir,
    agent_rules: config.agent_rules,
    agent_skills: config.agent_skills,
    temperature: config.temperature,
    connector_ids: [],
    prompt_template_version: PROMPT_TASK_TEMPLATE_VERSION,
    rules_revision: rulesRevision,
    skills_revision: skillsRevision,
  };
  await saveImmutableAgentRuntimeProfile(profile);
  return {
    runtimeProfileId: profile.id,
    model: profile.model,
    rulesRevision,
    skillsRevision,
  };
}

/** Builds a V2 envelope using the backend-canonical prompt-input digest. */
export async function createPromptTaskEnvelope(
  session: Omit<TaskSessionEnvelopeV1Data, "context_digest">,
  promptInput: TaskSessionInputV2,
): Promise<TaskSessionEnvelopeV2> {
  const contextDigest = await digestTaskSessionPromptInput(promptInput);
  if (session.kind === "chat" && promptInput.kind === "chat") {
    return {
      schema_version: 2,
      session: {
        session: { ...session, kind: "chat", context_digest: contextDigest },
        prompt_input: promptInput,
      },
    };
  }
  if (session.kind === "edit" && promptInput.kind === "edit") {
    return {
      schema_version: 2,
      session: {
        session: { ...session, kind: "edit", context_digest: contextDigest },
        prompt_input: promptInput,
      },
    };
  }
  throw new Error("Task Session kind does not match its prompt input.");
}

/** Submits one V2 prompt task and resolves only after a matching successful terminal event. */
export async function executePromptTaskSession(
  label: string,
  envelope: TaskSessionEnvelopeV2,
  onEvent?: (event: TaskSessionEvent) => void,
  onSubmitted?: (session: TaskSessionSnapshot) => void,
): Promise<PromptTaskExecution> {
  const submitted = await submitTaskSession(label, envelope, []);
  onSubmitted?.(submitted);
  return waitForPromptTaskSession(submitted.id, envelope, onEvent);
}

/** Replays a retained Chat/Edit Task Session without submitting or repeating runtime work. */
export async function waitForPromptTaskSession(
  sessionId: number,
  envelope: TaskSessionEnvelopeV2,
  onEvent?: (event: TaskSessionEvent) => void,
): Promise<PromptTaskExecution> {
  let cursor = 0;
  let candidate: PromptTaskCandidate | null = null;
  let succeededAttemptId: number | null = null;
  let hintedSequence = 0;
  let generation = 0;
  let wake: (() => void) | null = null;
  const watch = await onTaskSessionUpdated(sessionId, (update) => {
    hintedSequence = Math.max(hintedSequence, update.latest_sequence);
    generation += 1;
    wake?.();
    wake = null;
  });

  try {
    while (true) {
      let page;
      do {
        page = await listTaskSessionEvents(sessionId, cursor, 500);
        for (const event of page.events) {
          if (event.sequence <= cursor) continue;
          if (event.sequence !== cursor + 1) {
            throw new Error(`Task Session event gap after sequence ${cursor}.`);
          }
          cursor = event.sequence;
          onEvent?.(event);
          candidate = promptTaskCandidateFromEvent(event) ?? candidate;
          if (isSucceededLifecycle(event)) succeededAttemptId = event.attempt_id;
        }
      } while (page.has_more);
      watch.acknowledge(cursor);

      const snapshot = await getTaskSession(sessionId);
      if (!snapshot) throw new Error("Task Session was removed before completion.");
      if (isTerminal(snapshot.state)) {
        if (cursor < snapshot.last_event_sequence) continue;
        if (snapshot.state !== "succeeded") {
          throw new Error(snapshot.error ?? `Task Session ended as ${snapshot.state}.`);
        }
        if (candidate && candidate.attemptId !== succeededAttemptId) {
          throw new Error("Prompt result candidate came from a stale assignment attempt.");
        }
        const authoritative = await getTaskSessionResult(sessionId);
        if (!authoritative || authoritative.session_id !== sessionId) {
          throw new Error("Successful Task Session has no authoritative result.");
        }
        if (
          authoritative.terminal_state !== snapshot.state ||
          authoritative.finalized_at === null
        ) {
          throw new Error("Prompt Task Session result is not finalized with its terminal state.");
        }
        if (authoritative.output.kind === "chat") {
          if (
            envelope.session.session.kind !== "chat" ||
            authoritative.output.result.conversation_id !== envelope.session.session.conversation_id
          ) {
            throw new Error("Authoritative Chat result does not match its conversation.");
          }
          return { session: snapshot, result: authoritative.output.result };
        }
        if (
          authoritative.output.kind === "edit" &&
          envelope.session.session.kind === "edit" &&
          envelope.session.prompt_input.kind === "edit"
        ) {
          if (
            authoritative.output.result.file_path !== envelope.session.prompt_input.input.file_path
          ) {
            throw new Error("Authoritative Edit result does not match its target file.");
          }
          return { session: snapshot, result: authoritative.output.result };
        }
        throw new Error("Prompt Task Session returned an unexpected authoritative result kind.");
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
          if (generation !== observedGeneration || hintedSequence > cursor) resolveWake();
        });
      }
    }
  } finally {
    watch.unlisten();
  }
}

/** Parses a non-authoritative Chat/Edit candidate from one fenced runtime event. */
export function promptTaskCandidateFromEvent(event: TaskSessionEvent): PromptTaskCandidate | null {
  if (event.kind !== "runtime" || event.attempt_id === null || !isRecord(event.payload))
    return null;
  if (
    event.payload.type === "chat_result_candidate" &&
    event.payload.authoritative === false &&
    typeof event.payload.conversation_id === "string" &&
    typeof event.payload.message === "string"
  ) {
    return {
      kind: "chat",
      attemptId: event.attempt_id,
      conversationId: event.payload.conversation_id,
      message: event.payload.message,
    };
  }
  if (
    event.payload.type === "edit_result_candidate" &&
    event.payload.authoritative === false &&
    typeof event.payload.file_path === "string" &&
    typeof event.payload.summary === "string" &&
    typeof event.payload.content === "string"
  ) {
    return {
      kind: "edit",
      attemptId: event.attempt_id,
      filePath: event.payload.file_path,
      summary: event.payload.summary,
      content: event.payload.content,
    };
  }
  return null;
}

function isSucceededLifecycle(event: TaskSessionEvent): boolean {
  return (
    event.kind === "lifecycle" && isRecord(event.payload) && event.payload.state === "succeeded"
  );
}

function isTerminal(state: TaskSessionSnapshot["state"]): boolean {
  return (
    state === "succeeded" || state === "failed" || state === "blocked" || state === "cancelled"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function sha256(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
