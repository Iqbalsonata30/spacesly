import { applyTaskSessionEventPage, createTaskSessionReplay } from "../src/lib/taskSessionReplay";
import {
  taskToolCallIdentity,
  type TaskSessionEvent,
  type TaskSessionEventPage,
} from "../src/lib/ipc/taskSessions";
import { promptTaskCandidateFromEvent } from "../src/lib/promptTaskSessions";
import {
  AgentTaskSessionTimeoutError,
  agentTaskCandidateFromEvent,
  agentTaskCapabilities,
  ensureOpenCodeAgentProfile,
  executionRepositoryContext,
  continueAgentTaskSession,
  executeAgentTaskSession,
  planAgentTaskConnectors,
  prepareAgentTaskSession,
  resumeAgentTaskSession,
  validateAgentTaskSessionResult,
  waitForAgentTaskSession,
  type AgentTaskSessionDependencies,
} from "../src/lib/agentTaskSessions";
import type { AiWorkerConfig, ExecutionContract } from "../src/lib/ipc/agent";
import type {
  AgentRuntimeProfile,
  TaskSessionResult,
  TaskSessionSnapshot,
} from "../src/lib/ipc/taskSessions";
import {
  cancelWorkspaceChatRun,
  confirmLegacyWorkspaceChatCancellation,
  createWorkspaceChatRun,
  settleWorkspaceChatCancellation,
  updateWorkspaceChatRun,
  workspaceChatRunFor,
  type WorkspaceChatRuns,
} from "../src/lib/workspaceChatRuns";
import {
  agentTaskCardProjection,
  agentWorkflowCheckpoint,
  agentWorkflowRecoveryDecision,
  createAgentRunSession,
  runningAgentSessions,
} from "../src/lib/agentRun";

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function event(
  sequence: number,
  kind: TaskSessionEvent["kind"],
  payload: unknown,
  progress: TaskSessionEvent["progress"] = null,
  sessionId = 7,
): TaskSessionEvent {
  return {
    id: sequence,
    session_id: sessionId,
    attempt_id: sequence > 1 ? 1 : null,
    fencing_token: sequence > 1 ? 1 : 0,
    sequence,
    kind,
    payload,
    progress,
    created_at: sequence,
  };
}

function page(events: TaskSessionEvent[], hasMore = false): TaskSessionEventPage {
  return {
    events,
    next_cursor: events.at(-1)?.sequence ?? 0,
    has_more: hasMore,
  };
}

const initial = createTaskSessionReplay(7);
const first = applyTaskSessionEventPage(
  initial,
  page([
    event(1, "lifecycle", { state: "queued" }, { phase: "queued", completed: 0, total: null }),
    event(
      2,
      "lifecycle",
      { state: "running" },
      {
        phase: "executing",
        completed: 0,
        total: null,
      },
    ),
  ]),
);
assertEqual(first.gapDetected, false, "ordered replay should not report a gap");
assertEqual(
  { cursor: first.replay.cursor, state: first.replay.state, phase: first.replay.progress?.phase },
  { cursor: 2, state: "running", phase: "executing" },
  "ordered replay should project lifecycle and progress",
);

const duplicate = applyTaskSessionEventPage(
  first.replay,
  page([
    event(2, "lifecycle", { state: "running" }),
    event(
      3,
      "progress",
      { message: "verified" },
      {
        phase: "verifying",
        completed: 1,
        total: 1,
      },
    ),
  ]),
);
assertEqual(
  { cursor: duplicate.replay.cursor, eventCount: duplicate.replay.events.length },
  { cursor: 3, eventCount: 3 },
  "duplicate events should be ignored without duplicating materialized history",
);

const gap = applyTaskSessionEventPage(
  duplicate.replay,
  page([event(5, "lifecycle", { state: "succeeded" })]),
);
assertEqual(
  { gap: gap.gapDetected, cursor: gap.replay.cursor },
  { gap: true, cursor: 3 },
  "a gap should leave the cursor at the last applied event",
);

let rejectedForeignSession = false;
try {
  applyTaskSessionEventPage(initial, page([event(1, "activity", {}, null, 8)]));
} catch {
  rejectedForeignSession = true;
}
assertEqual(rejectedForeignSession, true, "replay should reject cross-session event contamination");

const chatCandidate = promptTaskCandidateFromEvent(
  event(4, "runtime", {
    type: "chat_result_candidate",
    authoritative: false,
    conversation_id: "conversation-1",
    message: "hello",
  }),
);
assertEqual(
  chatCandidate,
  {
    kind: "chat",
    attemptId: 1,
    conversationId: "conversation-1",
    message: "hello",
  },
  "chat candidate should preserve its fenced attempt and conversation",
);

assertEqual(
  promptTaskCandidateFromEvent(
    event(5, "runtime", {
      type: "edit_result_candidate",
      authoritative: true,
      file_path: "src/main.ts",
      summary: "updated",
      content: "next",
    }),
  ),
  null,
  "candidate parser should reject incorrectly authoritative payloads",
);

assertEqual(
  taskToolCallIdentity({
    tool_call_id: "tool-1",
    tool_name: "jira_search",
    status: "succeeded",
    risk: null,
    arguments_digest: null,
    display_context: null,
    attempt_id: 11,
    fencing_token: 1,
    started_sequence: 1,
    completed_sequence: 2,
    updated_at: 2,
  }) ===
    taskToolCallIdentity({
      tool_call_id: "tool-1",
      tool_name: "jira_search",
      status: "running",
      risk: null,
      arguments_digest: null,
      display_context: null,
      attempt_id: 12,
      fencing_token: 2,
      started_sequence: 3,
      completed_sequence: null,
      updated_at: 3,
    }),
  false,
  "tool-call display identity should remain distinct across retries",
);

const config: AiWorkerConfig = {
  workspace_id: "workspace-a",
  runtime: "opencode",
  provider_name: "OpenAI",
  provider_id: "openai",
  base_url: "https://example.test",
  api_style: "openai_responses",
  model: "gpt-5",
  opencode_command: "opencode",
  opencode_model: "openai/gpt-5",
  opencode_workdir: "/workspace",
  opencode_auto_approve: false,
  agent_rules: "Use evidence.",
  agent_skills: "Verify changes.",
  temperature: 0.2,
  mcp_servers: [
    { name: "Z", secret_id: "zeta", command: ["zeta"] },
    { name: "A", secret_id: "alpha", command: ["alpha"] },
  ],
};

assertEqual(
  executionRepositoryContext(
    { ...config, opencode_workdir: "/home/iqbalsonata" },
    {
      is_git_repo: true,
      repo_root: "/home/iqbalsonata/BRI",
      current_branch: "main",
      branches: ["main"],
      head_commit: "abc",
      upstream_branch: null,
      dirty_worktree: false,
      ahead_count: 0,
      behind_count: 0,
    },
    "/home/iqbalsonata/BRI",
  ).root_path,
  "/home/iqbalsonata",
  "configured Agent workdir should override the open Git workspace in execution contracts",
);
const contract = {
  contract_id: "contract-1",
  version: 1,
  task_id: "card-1",
  workspace_id: "workspace-a",
  created_at: "2026-07-29T00:00:00.000Z",
  objective: { summary: "Implement", success_criteria: ["tested"] },
  task_context: { description: "Implement it", execution_detail: "queued" },
  ticket: {
    provider: "local",
    key: null,
    url: null,
    title: "Implement",
    labels: [],
    status: null,
    updated_at: null,
    fetched_at: null,
  },
  workflow: [],
  completed_steps: [],
  current_step: "worker.execute",
  remaining_steps: [],
  repository: { root_path: "/workspace", branch: "main", head_commit: "abc" },
  constraints: {
    execution_only: true,
    planning_completed: true,
    must_not_read_jira_for_planning: true,
    must_not_classify_ticket: true,
    must_not_regenerate_workflow: true,
    must_not_rediscover_repository: true,
    may_modify_files: true,
    may_update_jira: false,
  },
  runtime_inputs: { operator_notes: null, previous_output: null },
} satisfies ExecutionContract;
const authoritativeResult: import("../src/lib/ipc/agent").AiWorkerTaskResult = {
  summary: "Implemented",
  evidence: ["tests passed"],
  details: [],
  next: [],
  completion_status: "completed",
  blocked_reason: null,
};

const intentConfig: AiWorkerConfig = {
  ...config,
  mcp_servers: [
    {
      name: "Jira",
      secret_id: "jira",
      command: ["jira"],
      domains: ["jira"],
      intent_terms: ["jira", "ticket", "deploy", "deployment", "prerelease"],
    },
    {
      name: "Kubernetes",
      secret_id: "kubernetes",
      command: ["kubernetes"],
      domains: ["kubernetes"],
      intent_terms: ["kubernetes", "pod", "deploy", "deployment", "prerelease"],
    },
    {
      name: "Bamboo",
      secret_id: "bamboo",
      command: ["bamboo"],
      domains: ["bamboo"],
      intent_terms: ["bamboo", "bamboo build", "deploy", "deployment", "prerelease"],
    },
    {
      name: "Bitbucket",
      secret_id: "bitbucket",
      command: ["bitbucket"],
      domains: ["bitbucket"],
      intent_terms: ["bitbucket", "pull request", "pr", "review pr"],
    },
    {
      name: "Unrelated",
      secret_id: "unrelated",
      command: ["unrelated"],
      domains: ["unrelated"],
      intent_terms: ["unrelated service"],
    },
  ],
};

function contractFor(summary: string, provider: "jira" | "local" = "local"): ExecutionContract {
  return {
    ...contract,
    objective: { ...contract.objective, summary },
    task_context: { ...contract.task_context, description: summary },
    ticket: { ...contract.ticket, provider, title: summary },
  };
}

assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Create README.md")).connectorIds,
  [],
  "filesystem tasks should not load external connector schemas",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Create README.md", "jira")).connectorIds,
  [],
  "a linked ticket should not add Jira schemas to a filesystem-only task",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Commit the local changes")).connectorIds,
  [],
  "Git-only tasks should remain on the built-in Git capability",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Build a small UI component")).connectorIds,
  [],
  "ordinary code build language should not select Bamboo",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Deploy service to prerelease")).connectorIds,
  ["bamboo", "jira", "kubernetes"],
  "deployment tasks should select only matching operational domains",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Review PR #42")).connectorIds,
  ["bitbucket"],
  "pull-request tasks should select only the configured Bitbucket domain",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Update the linked issue", "jira"))
    .connectorIds,
  ["jira"],
  "structured Jira tasks should select only the Jira domain",
);
assertEqual(
  planAgentTaskConnectors(intentConfig, contractFor("Inspect the unrelated service")).connectorIds,
  ["unrelated"],
  "declared generic connector intent should remain available",
);

assertEqual(
  agentTaskCapabilities(config),
  {
    connectorIds: ["alpha", "zeta"],
    capabilities: [
      "workspace_read",
      "workspace_write",
      "shell",
      "git",
      "external_tools:alpha",
      "external_tools:zeta",
    ],
  },
  "connectors and external capability grants should be sorted and match exactly",
);

const savedProfiles: AgentRuntimeProfile[] = [];
await ensureOpenCodeAgentProfile(config, {
  saveProfile: async (profile) => {
    savedProfiles.push(profile);
    return profile;
  },
});
const savedProfile = savedProfiles[0];
assertEqual(
  {
    idIsContentAddressed: savedProfile?.id.startsWith("agent-") ?? false,
    connectors: savedProfile?.connector_ids,
    template: savedProfile?.prompt_template_version,
    workdir: savedProfile?.opencode_workdir,
  },
  {
    idIsContentAddressed: true,
    connectors: ["alpha", "zeta"],
    template: "agent-task-v1",
    workdir: "/workspace",
  },
  "profile identity should include canonical connectors and template version",
);

const alternateWorkdirProfiles: AgentRuntimeProfile[] = [];
await ensureOpenCodeAgentProfile(
  { ...config, opencode_workdir: "/workspace/other" },
  {
    saveProfile: async (profile) => {
      alternateWorkdirProfiles.push(profile);
      return profile;
    },
  },
);
assertEqual(
  alternateWorkdirProfiles[0]?.id === savedProfile?.id,
  false,
  "profile identity should bind the configured working directory",
);

const appendedMessages: Array<{ conversationId: string; id: string; text: string }> = [];
const preparationDependencies: Partial<AgentTaskSessionDependencies> = {
  saveProfile: async (profile: AgentRuntimeProfile) => profile,
  digestContract: async () => "sha256:contractdigest",
  appendMessage: async (_workspaceId, conversationId, _title, message) => {
    appendedMessages.push({ conversationId, id: message.id, text: message.text });
    return { ...message, conversation_id: conversationId, sequence: 1, created_at: 1 };
  },
};
const prepared = await prepareAgentTaskSession(
  config,
  "card-1",
  "Implement",
  "run-1",
  contract,
  9,
  preparationDependencies,
);
const preparedAgain = await prepareAgentTaskSession(
  config,
  "card-1",
  "Implement",
  "run-1",
  contract,
  9,
  preparationDependencies,
);
assertEqual(
  {
    schema: prepared.envelope.schema_version,
    kind: prepared.envelope.session.kind,
    revision: prepared.envelope.session.context_revision,
    conversationStable: prepared.conversationId === preparedAgain.conversationId,
    messageStable: appendedMessages[0]?.id === appendedMessages[1]?.id,
    contractIncluded: appendedMessages[0]?.text.includes("contract-1") ?? false,
    selectedConnectors: prepared.envelope.session.connector_ids,
  },
  {
    schema: 1,
    kind: "agent",
    revision: "9",
    conversationStable: true,
    messageStable: true,
    contractIncluded: true,
    selectedConnectors: [],
  },
  "V1 envelope and durable context should be deterministic per card and contract",
);

const candidateEvent = {
  ...event(6, "runtime", {
    type: "agent_result_candidate",
    authoritative: false,
    result: authoritativeResult,
  }),
  attempt_id: 4,
  fencing_token: 7,
};
assertEqual(
  agentTaskCandidateFromEvent(candidateEvent),
  authoritativeResult,
  "Agent candidate parsing should accept only the diagnostic fenced shape",
);

function terminalSnapshot(id: number): TaskSessionSnapshot {
  return {
    id,
    request: { label: `session-${id}`, payload: "" },
    state: "succeeded",
    worker_id: 1,
    dispatch_sequence: 1,
    attempt: 1,
    attempt_id: 4,
    fencing_token: 7,
    opencode_session_id: "opencode-session-x",
    lease_expires_at: null,
    progress: null,
    last_event_sequence: 1,
    error: null,
    created_at: 1,
    started_at: 1,
    completed_at: 2,
  };
}

function taskResult(id: number): TaskSessionResult {
  return {
    session_id: id,
    output: { kind: "agent", result: { ...authoritativeResult } },
    terminal_state: "succeeded",
    projection_error: null,
    projected_at: 2,
    finalized_at: 2,
  };
}

assertEqual(
  validateAgentTaskSessionResult(terminalSnapshot(10), taskResult(10), 4, 7, 4).result,
  authoritativeResult,
  "authoritative result should pass exact terminal and attempt validation",
);
let staleAttemptRejected = false;
try {
  validateAgentTaskSessionResult(terminalSnapshot(10), taskResult(10), 4, 7, 3);
} catch {
  staleAttemptRejected = true;
}
assertEqual(staleAttemptRejected, true, "a stale candidate attempt should be rejected");

const failedSnapshot = {
  ...terminalSnapshot(11),
  state: "failed" as const,
  error: "External tool 'jira_search' failed. Cause: Connection refused.",
};
let failedTerminalError = "";
try {
  validateAgentTaskSessionResult(failedSnapshot, null, 4, 7, null);
} catch (reason) {
  failedTerminalError = reason instanceof Error ? reason.message : String(reason);
}
assertEqual(
  failedTerminalError,
  "External tool 'jira_search' failed. Cause: Connection refused.",
  "failed terminal without a completion outbox row should preserve its scheduler error",
);

let unexpectedFailedResultRejected = false;
try {
  validateAgentTaskSessionResult(failedSnapshot, taskResult(11), 4, 7, null);
} catch (reason) {
  unexpectedFailedResultRejected =
    reason instanceof Error &&
    reason.message.includes("authoritative result did not match its terminal projection") &&
    reason.message.includes('"terminal_state":"succeeded"') &&
    reason.message.includes('"terminal_state":"failed"');
}
assertEqual(
  unexpectedFailedResultRejected,
  true,
  "failed terminal must reject an unexpected structured authoritative result with a field diff",
);

const submittedIds: number[] = [];
let nextSessionId = 20;
const independentDependencies: Partial<AgentTaskSessionDependencies> = {
  submit: async () => {
    const id = nextSessionId++;
    submittedIds.push(id);
    return { ...terminalSnapshot(id), state: "queued", attempt_id: null, last_event_sequence: 0 };
  },
  watch: async () => ({ unlisten() {}, acknowledge() {} }),
  listEvents: async (sessionId) => ({
    events: [
      {
        ...event(1, "lifecycle", { state: "succeeded" }, null, sessionId),
        attempt_id: 4,
        fencing_token: 7,
      },
    ],
    next_cursor: 1,
    has_more: false,
  }),
  getSession: async (sessionId) => terminalSnapshot(sessionId),
  getResult: async (sessionId) => taskResult(sessionId),
};
const [firstExecution, secondExecution] = await Promise.all([
  executeAgentTaskSession("first", prepared, { dependencies: independentDependencies }),
  executeAgentTaskSession("second", prepared, { dependencies: independentDependencies }),
]);
assertEqual(
  [firstExecution.session.id, secondExecution.session.id].sort(),
  submittedIds.sort(),
  "concurrent cards should retain independent Task Session identities",
);

let resumedTaskSessionId: number | null = null;
const resumedExecution = await resumeAgentTaskSession(42, "approved", prepared, {
  dependencies: {
    ...independentDependencies,
    resume: async (sessionId) => {
      resumedTaskSessionId = sessionId;
      return {
        ...terminalSnapshot(sessionId),
        state: "queued",
        attempt_id: null,
        last_event_sequence: 0,
      };
    },
  },
});
assertEqual(
  {
    requestedId: resumedTaskSessionId,
    returnedId: resumedExecution.session.id,
    opencodeSessionId: resumedExecution.session.opencode_session_id,
  },
  { requestedId: 42, returnedId: 42, opencodeSessionId: "opencode-session-x" },
  "approval continuation should wait on the same durable Task Session identity",
);

let continuedTaskSessionId: number | null = null;
const continuedExecution = await continueAgentTaskSession(43, "continue", prepared, {
  dependencies: {
    ...independentDependencies,
    continueInterrupted: async (sessionId) => {
      continuedTaskSessionId = sessionId;
      return {
        ...terminalSnapshot(sessionId),
        state: "queued",
        attempt_id: null,
        last_event_sequence: 0,
      };
    },
  },
});
assertEqual(
  {
    requestedId: continuedTaskSessionId,
    returnedId: continuedExecution.session.id,
    opencodeSessionId: continuedExecution.session.opencode_session_id,
  },
  { requestedId: 43, returnedId: 43, opencodeSessionId: "opencode-session-x" },
  "generic continuation should retain both Task Session and OpenCode session identity",
);

let timeoutCancelledSession: number | null = null;
let timeoutRejected = false;
let timeoutCancellationRequested = false;
try {
  await waitForAgentTaskSession(99, {
    timeoutMs: 5,
    cancellationTimeoutMs: 100,
    dependencies: {
      watch: async () => ({ unlisten() {}, acknowledge() {} }),
      listEvents: async () => ({ events: [], next_cursor: 0, has_more: false }),
      getSession: async () =>
        timeoutCancellationRequested
          ? { ...terminalSnapshot(99), state: "cancelled" }
          : {
              ...terminalSnapshot(99),
              state: "running",
              completed_at: null,
              last_event_sequence: 0,
            },
      cancel: async (sessionId) => {
        timeoutCancelledSession = sessionId;
        timeoutCancellationRequested = true;
        return true;
      },
    },
  });
} catch (reason) {
  timeoutRejected = reason instanceof AgentTaskSessionTimeoutError && reason.cancelled;
}
assertEqual(
  { timeoutRejected, timeoutCancelledSession },
  { timeoutRejected: true, timeoutCancelledSession: 99 },
  "timeout should cooperatively cancel only its own Task Session",
);

let conversationRuns: WorkspaceChatRuns = {
  first: {
    ...createWorkspaceChatRun(1),
    running: true,
    taskSessionId: 101,
    state: "running" as const,
  },
  second: {
    ...createWorkspaceChatRun(4),
    running: true,
    taskSessionId: 202,
    state: "queued" as const,
  },
};
conversationRuns = updateWorkspaceChatRun(conversationRuns, "first", (run) => ({
  ...run,
  streamingText: "first response",
  progress: { phase: "executing", completed: 1, total: 4 },
}));
conversationRuns = updateWorkspaceChatRun(conversationRuns, "second", (run) => ({
  ...run,
  streamingText: "second response",
  progress: { phase: "executing", completed: 3, total: 4 },
}));
assertEqual(
  {
    first: workspaceChatRunFor(conversationRuns, "first").streamingText,
    second: workspaceChatRunFor(conversationRuns, "second").streamingText,
    bothRunning: Object.values(conversationRuns).filter((run) => run.running).length,
  },
  { first: "first response", second: "second response", bothRunning: 2 },
  "two conversations should stream and progress independently while selection changes",
);
const conversationSelectionHistory = ["first", "second"];
assertEqual(
  {
    selectedText: workspaceChatRunFor(conversationRuns, conversationSelectionHistory.at(-1) ?? "")
      .streamingText,
    firstStillRunning: workspaceChatRunFor(conversationRuns, "first").running,
  },
  { selectedText: "second response", firstStillRunning: true },
  "switching conversations should only change the selected projection",
);
conversationRuns = cancelWorkspaceChatRun(conversationRuns, "first");
assertEqual(
  {
    firstRunning: conversationRuns.first.running,
    firstState: conversationRuns.first.state,
    firstTaskSessionId: conversationRuns.first.taskSessionId,
    secondRunning: conversationRuns.second.running,
    secondGeneration: conversationRuns.second.generation,
  },
  {
    firstRunning: true,
    firstState: "cancelling",
    firstTaskSessionId: 101,
    secondRunning: true,
    secondGeneration: 4,
  },
  "cancelling should retain identity and leave another conversation untouched",
);
const cancellationGeneration = conversationRuns.first.generation;
conversationRuns = settleWorkspaceChatCancellation(
  conversationRuns,
  "first",
  cancellationGeneration,
  null,
);
assertEqual(
  {
    running: conversationRuns.first.running,
    taskSessionId: conversationRuns.first.taskSessionId,
    state: conversationRuns.first.state,
  },
  { running: true, taskSessionId: 101, state: "cancelling" },
  "failed cancellation confirmation should retain identity and prevent a new turn",
);
conversationRuns = settleWorkspaceChatCancellation(
  conversationRuns,
  "first",
  cancellationGeneration,
  "cancelled",
);
assertEqual(
  {
    running: conversationRuns.first.running,
    taskSessionId: conversationRuns.first.taskSessionId,
    state: conversationRuns.first.state,
  },
  { running: false, taskSessionId: null, state: "cancelled" },
  "terminal cancellation should clear execution identity",
);

let cancellationNow = 0;
let cancellationPoll = 0;
assertEqual(
  await confirmLegacyWorkspaceChatCancellation("legacy-1", {
    cancel: async () => true,
    getRun: async () => ({
      run_id: "legacy-1",
      kind: "chat",
      status: ++cancellationPoll < 3 ? "cancelling" : "cancelled",
      created_at: 0,
      updated_at: cancellationPoll,
    }),
    sleep: async (milliseconds) => {
      cancellationNow += milliseconds;
    },
    now: () => cancellationNow,
    timeoutMs: 100,
    pollMs: 10,
  }),
  "cancelled",
  "legacy cancellation should wait for a terminal backend status",
);
assertEqual(
  await confirmLegacyWorkspaceChatCancellation("legacy-timeout", {
    cancel: async () => true,
    getRun: async () => ({
      run_id: "legacy-timeout",
      kind: "chat",
      status: "cancelling",
      created_at: 0,
      updated_at: 0,
    }),
    sleep: async (milliseconds) => {
      cancellationNow += milliseconds;
    },
    now: () => cancellationNow,
    timeoutMs: 20,
    pollMs: 10,
  }),
  null,
  "legacy cancellation timeout should preserve cancelling identity",
);

let replayA = createTaskSessionReplay(301);
let replayB = createTaskSessionReplay(302);
replayA = applyTaskSessionEventPage(
  replayA,
  page([event(1, "lifecycle", { state: "running" }, null, 301)]),
).replay;
replayB = applyTaskSessionEventPage(
  replayB,
  page([
    event(1, "lifecycle", { state: "queued" }, null, 302),
    event(2, "progress", {}, { phase: "preparing", completed: 1, total: 2 }, 302),
  ]),
).replay;
replayA = applyTaskSessionEventPage(
  replayA,
  page([event(2, "progress", {}, { phase: "working", completed: 8, total: 10 }, 301)]),
).replay;
assertEqual(
  {
    a: [replayA.state, replayA.progress?.phase, replayA.events.length],
    b: [replayB.state, replayB.progress?.phase, replayB.events.length],
  },
  { a: ["running", "working", 2], b: ["queued", "preparing", 2] },
  "interleaved session events should retain independent timeline and progress projections",
);
const boundedReplay = applyTaskSessionEventPage(
  createTaskSessionReplay(303),
  page([
    event(1, "activity", {}, null, 303),
    event(2, "activity", {}, null, 303),
    event(3, "activity", {}, null, 303),
  ]),
  2,
).replay;
assertEqual(
  { cursor: boundedReplay.cursor, sequences: boundedReplay.events.map((entry) => entry.sequence) },
  { cursor: 3, sequences: [2, 3] },
  "durable replay display events should remain bounded without moving the cursor backwards",
);

const cardSessions = {
  a: createAgentRunSession("a", "First", "running", 25, "", null, [], [], null, [], null, 1),
  b: createAgentRunSession("b", "Second", "running", 75, "", null, [], [], null, [], null, 2),
};
cardSessions.a.taskSessionState = "committing";
assertEqual(
  {
    projection: agentTaskCardProjection(cardSessions.a),
    runningCards: runningAgentSessions(cardSessions).map((session) => session.cardId),
  },
  {
    projection: { status: "committing", progress: 25, running: true },
    runningCards: ["a", "b"],
  },
  "task-card helpers should project each authoritative per-card session",
);

const recoveredRun = {
  run_id: "run-recovered",
  contract,
  status: "running" as const,
  current_step_ids: ["worker.verify"],
  step_runs: {
    "worker.execute": {
      step_id: "worker.execute",
      status: "completed" as const,
      attempt: 1,
      started_at: "2026-07-29T00:00:00Z",
      completed_at: "2026-07-29T00:01:00Z",
      summary: "Implemented",
    },
    "worker.verify": {
      step_id: "worker.verify",
      status: "pending" as const,
      attempt: 0,
      started_at: null,
      completed_at: null,
      summary: null,
    },
  },
  started_at: "2026-07-29T00:00:00Z",
  completed_at: null,
};
assertEqual(
  agentWorkflowCheckpoint(recoveredRun),
  "agent_result_committed",
  "recovery should block after the authoritative Agent result without repeating workflow effects",
);
assertEqual(
  agentWorkflowCheckpoint({
    ...recoveredRun,
    current_step_ids: ["jira.comment.result"],
    step_runs: {
      ...recoveredRun.step_runs,
      "jira.comment.result": {
        step_id: "jira.comment.result",
        status: "interrupted",
        attempt: 1,
        started_at: "2026-07-29T00:02:00Z",
        completed_at: null,
        summary: "Jira Done transition completed; completion comment pending.",
      },
    },
  }),
  "jira_transition_completed",
  "recovery should resume after a persisted Jira transition without repeating it",
);
assertEqual(
  agentWorkflowRecoveryDecision("jira_writeback_started").safe,
  false,
  "ambiguous Jira writeback must block instead of replaying transition or comment",
);
assertEqual(
  agentWorkflowRecoveryDecision("jira_transition_completed").safe,
  true,
  "durably confirmed Jira transition may continue to the not-yet-started comment",
);
assertEqual(
  agentWorkflowCheckpoint({
    ...recoveredRun,
    current_step_ids: ["jira.comment.result"],
    step_runs: {
      ...recoveredRun.step_runs,
      "jira.comment.result": {
        step_id: "jira.comment.result",
        status: "running",
        attempt: 1,
        started_at: "2026-07-29T00:03:00Z",
        completed_at: null,
        summary: "Jira completion comment started; confirmation pending.",
      },
    },
  }),
  "jira_writeback_started",
  "an unconfirmed Jira comment must recover as ambiguous and never replay automatically",
);
