import {
  applySavedSnapshot,
  createDocumentSession,
  createRecoveredDocumentSession,
  documentSnapshot,
  markDocumentSaved,
  markDocumentExternalConflict,
  replaceDocument,
} from "../src/lib/editorDocument";
import { EditorState } from "@codemirror/state";
import { createEditorCommandRegistry } from "../src/lib/editorCommands";
import { prettierPluginGroupForParser } from "../src/lib/editorFormatting";
import { fileTreeNavigationIndex } from "../src/lib/fileBrowser";
import { workspaceFileChangeIsStructural } from "../src/lib/filesFeature";
import { lspConfigForPath } from "../src/lib/lspConfig";
import {
  workspaceChatActionDescription,
  workspaceChatActionRequiresConfirmation,
  workspaceContextRevision,
} from "../src/lib/workspaceChat";
import { aiEditProposalIsStale, applyAiEditHunks, createAiEditProposal } from "../src/lib/aiEdit";
import {
  lspPositionToOffset,
  lspTextEditsToChanges,
  offsetToLspPosition,
  shouldPollLspDiagnostics,
} from "../src/lib/lspEditor";
import {
  canNavigateEditor,
  createEditorNavigation,
  editorNavigationTarget,
  pushEditorLocation,
} from "../src/lib/editorNavigation";
import {
  createWorkspaceChatSession,
  loadUiState,
  type WorkspaceChatMessage,
} from "../src/lib/uiState";
import { formatJiraExecutionComment } from "../src/lib/jiraComment";
import { timelineActivity, timelineActivities } from "../src/lib/agentTimeline";
import {
  applyAgentEventProjection,
  emptyAgentEventProjection,
  mergeAgentEventProjection,
  projectAgentTaskSessionEvent,
} from "../src/lib/agentEventProjection";
import { createAgentRunSession } from "../src/lib/agentRun";
import { relativeTimeLabel } from "../src/lib/relativeTime";
import {
  defaultSettings,
  loadSettings,
  normalizeSettings,
  parseEnvText,
  saveSettings,
  summarizeMcpCapabilities,
} from "../src/lib/settings";

function assertEqual(actual: unknown, expected: unknown, message: string) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

assertEqual(
  parseEnvText("API_URL=https://service.example\nAPI_TOKEN=secret"),
  { API_URL: "https://service.example", API_TOKEN: "secret" },
  "generic MCP environment should be parsed as the complete textarea value",
);

const capabilityCatalog = summarizeMcpCapabilities(
  [
    {
      name: "future_publish_artifact",
      input_schema: {
        type: "object",
        properties: { artifact_path: {}, "not valid secret": {} },
      },
    },
    { name: "../../invalid", input_schema: null },
  ],
  123,
);
assertEqual(
  capabilityCatalog,
  {
    schemaVersion: 1,
    testedAt: 123,
    tools: [
      {
        name: "future_publish_artifact",
        risk: "unknown",
        argumentNames: ["artifact_path"],
      },
    ],
  },
  "MCP capability summaries should remain bounded, canonical, and free of raw schemas",
);
const normalizedCatalog = normalizeSettings({
  mcpServers: [
    {
      id: "future",
      name: "Future MCP",
      kind: "generic",
      command: "future-mcp",
      capabilityCatalog,
    },
  ],
}).mcpServers[0]?.capabilityCatalog;
assertEqual(
  normalizedCatalog,
  capabilityCatalog,
  "persisted MCP capability summaries should survive settings normalization",
);
assertEqual(parseEnvText(""), {}, "clearing generic MCP environment should remove every key");
const migratedAtlassianConnector = normalizeSettings({
  mcpServers: [
    {
      id: "jira-default",
      name: "Jira MCP",
      kind: "jira",
      command: "mcp-atlassian",
      args: [],
      env: {},
      domains: ["jira"],
      intentTerms: ["jira"],
    },
  ],
}).mcpServers[0];
assertEqual(
  {
    domains: migratedAtlassianConnector?.domains,
    confluenceIntent: migratedAtlassianConnector?.intentTerms.includes("confluence"),
  },
  { domains: ["jira", "confluence"], confluenceIntent: true },
  "the bundled Atlassian connector should migrate to explicit Jira and Confluence routing",
);

const originalLocalStorage = globalThis.localStorage;
const settingsStorage = new Map<string, string>();
const memoryLocalStorage = {
  get length() {
    return settingsStorage.size;
  },
  clear() {
    settingsStorage.clear();
  },
  getItem(key: string) {
    return settingsStorage.get(key) ?? null;
  },
  key(index: number) {
    return [...settingsStorage.keys()][index] ?? null;
  },
  removeItem(key: string) {
    settingsStorage.delete(key);
  },
  setItem(key: string, value: string) {
    settingsStorage.set(key, value);
  },
} satisfies Storage;
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: memoryLocalStorage,
});
const settingsWithoutMcp = structuredClone(defaultSettings);
settingsWithoutMcp.mcpServers = [];
saveSettings(settingsWithoutMcp);
assertEqual(loadSettings().mcpServers, [], "removing the final MCP connector should persist");
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: originalLocalStorage,
});

assertEqual(
  workspaceContextRevision("same context"),
  workspaceContextRevision("same context"),
  "workspace context revisions should be stable",
);

const timestampNow = new Date(2026, 0, 1, 4, 30, 18).getTime();
assertEqual(
  relativeTimeLabel("04:30:18 AM", timestampNow),
  "Just now",
  "fresh activity should be relative",
);
assertEqual(
  relativeTimeLabel("04:29:58 AM", timestampNow),
  "20 sec ago",
  "recent activity should show seconds",
);
assertEqual(
  relativeTimeLabel("04:25:18 AM", timestampNow),
  "5 min ago",
  "older activity should show minutes",
);

const contextActivity = timelineActivity({
  id: "log-1",
  at: "04:30:18 AM",
  tone: "info",
  label: "context",
  message: [
    "STATUS: Running",
    "SUMMARY: Exported structured context for APP-1.",
    "EVIDENCE:",
    "- Runtime: OpenCode gpt-5.5",
    "DETAILS:",
    "- Sections: SUMMARY, EVIDENCE, DETAILS",
  ].join("\n"),
});
assertEqual(contextActivity.title, "Preparing Task", "timeline should explain progress");
assertEqual(
  contextActivity.summary,
  "Gathering the information needed to begin.",
  "timeline summary should avoid raw runtime labels",
);
assertEqual(
  contextActivity.sections[0].title,
  "Evidence",
  "technical evidence should move into details",
);

const collapsedActivities = timelineActivities([
  {
    id: "log-2",
    at: "04:30:19 AM",
    tone: "info",
    label: "progress",
    message:
      "SUMMARY: Task Session progress: activity.\nDETAILS:\n- Progress phase: executing_runtime",
  },
  {
    id: "log-3",
    at: "04:30:20 AM",
    tone: "info",
    label: "progress",
    message:
      "SUMMARY: Task Session progress: text_delta.\nDETAILS:\n- Progress phase: executing_runtime",
  },
]);
assertEqual(collapsedActivities.length, 1, "repeated progress updates should collapse");
assertEqual(collapsedActivities[0].repeatCount, 2, "collapsed updates should retain count");
assertEqual(
  collapsedActivities[0].title,
  "Executing Task",
  "progress should describe the business activity",
);
assertEqual(
  timelineActivity({
    id: "log-4",
    at: "04:30:21 AM",
    tone: "info",
    label: "files",
    message: "SUMMARY: Completed: Reading src/main.rs.",
  }).status,
  "completed",
  "completed tool activity should not appear as still running",
);

const lifecycleActivities = timelineActivities([
  {
    id: "queued",
    at: "04:30:01 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered queued.",
  },
  {
    id: "running",
    at: "04:30:02 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered running.",
  },
  {
    id: "progress",
    at: "04:30:03 AM",
    tone: "info",
    label: "progress",
    message: "SUMMARY: Task Session progress.\nDETAILS:\n- Progress phase: executing_runtime",
  },
  {
    id: "committing",
    at: "04:30:04 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered committing.",
  },
  {
    id: "succeeded",
    at: "04:30:05 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered succeeded.",
  },
]);
assertEqual(
  lifecycleActivities.map(({ title }) => title),
  ["Execution Completed", "Saving Results", "Executing Task", "Agent Started", "Queued"],
  "lifecycle events should become concise user activities with the latest action first",
);
assertEqual(
  lifecycleActivities.some(({ title, summary }) =>
    /task session|runtime|lifecycle|projection/i.test(`${title} ${summary}`),
  ),
  false,
  "collapsed activities should not expose runtime terminology",
);
assertEqual(
  lifecycleActivities.filter(({ status }) => status === "running").length,
  0,
  "successful completion should finalize every running activity",
);
assertEqual(
  lifecycleActivities.slice(1).every(({ status }) => status === "completed"),
  true,
  "successful completion should leave all preceding lifecycle activities completed",
);

const activeLifecycleActivities = timelineActivities([
  {
    id: "active-queued",
    at: "04:30:01 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered queued.",
  },
  {
    id: "active-running",
    at: "04:30:02 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered running.",
  },
  {
    id: "active-progress",
    at: "04:30:03 AM",
    tone: "info",
    label: "progress",
    message: "SUMMARY: Task Session progress.\nDETAILS:\n- Progress phase: executing_runtime",
  },
]);
assertEqual(
  activeLifecycleActivities.filter(({ status }) => status === "running").length,
  1,
  "only the latest business activity should remain running",
);
assertEqual(
  activeLifecycleActivities[0].title,
  "Executing Task",
  "the newest business activity should own the running state",
);

const failedLifecycleActivities = timelineActivities([
  {
    id: "failed-running",
    at: "04:31:01 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered running.",
  },
  {
    id: "failed-progress",
    at: "04:31:02 AM",
    tone: "info",
    label: "progress",
    message: "SUMMARY: Task Session progress.\nDETAILS:\n- Progress phase: executing_runtime",
  },
  {
    id: "failed-terminal",
    at: "04:31:03 AM",
    tone: "error",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered failed.",
  },
]);
assertEqual(
  failedLifecycleActivities.filter(({ status }) => status === "running").length,
  0,
  "failed completion should leave no running activities",
);
assertEqual(
  failedLifecycleActivities[0].status,
  "failed",
  "the latest failed activity should be failed",
);

const replayedTerminalActivities = timelineActivities([
  {
    id: "replay-running",
    at: "04:32:01 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered running.",
  },
  {
    id: "replay-succeeded",
    at: "04:32:02 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered succeeded.",
  },
  {
    id: "replay-stale",
    at: "04:32:03 AM",
    tone: "info",
    label: "progress",
    message: "SUMMARY: Task Session progress.\nDETAILS:\n- Progress phase: executing_runtime",
  },
  {
    id: "replay-succeeded",
    at: "04:32:02 AM",
    tone: "info",
    label: "lifecycle",
    message: "SUMMARY: Task Session entered succeeded.",
  },
]);
assertEqual(
  replayedTerminalActivities.filter(({ status }) => status === "running").length,
  0,
  "replay after terminal state should never restore a running activity",
);
assertEqual(
  replayedTerminalActivities.filter(({ title }) => title === "Execution Completed").length,
  1,
  "duplicate durable terminal events should be idempotent",
);

const jiraActivities = timelineActivities([
  {
    id: "jira-start",
    at: "04:30:06 AM",
    tone: "info",
    label: "tool",
    message: "SUMMARY: Tool started: Reading Jira issue APP-1.\nEVIDENCE:\n- Assignment attempt: 2",
  },
  {
    id: "jira-complete",
    at: "04:30:07 AM",
    tone: "info",
    label: "tool",
    message:
      "SUMMARY: Tool completed; task still running: Reading Jira issue APP-1.\nEVIDENCE:\n- Event sequence: 7",
  },
]);
assertEqual(jiraActivities.length, 1, "tool start and completion should update one activity");
assertEqual(
  jiraActivities[0].title,
  "Reading Jira Ticket",
  "tool context should become a business action",
);
assertEqual(
  jiraActivities[0].status,
  "completed",
  "merged tool activity should use its latest status",
);
assertEqual(
  jiraActivities[0].sections.some((section) =>
    section.lines.some((line) => /Assignment attempt|Event sequence/.test(line)),
  ),
  true,
  "technical metadata should remain available in expanded sections",
);

const interleavedJiraActivities = timelineActivities([
  {
    id: "jira-interleaved-start",
    at: "04:30:06 AM",
    tone: "info",
    label: "tool",
    message: "SUMMARY: Tool started: Reading Jira issue APP-1.",
  },
  {
    id: "workspace-interleaved",
    at: "04:30:07 AM",
    tone: "info",
    label: "tool",
    message: "SUMMARY: Tool started: Reading src/main.rs.",
  },
  {
    id: "jira-interleaved-complete",
    at: "04:30:08 AM",
    tone: "info",
    label: "tool",
    message: "SUMMARY: Tool completed; task still running: Reading Jira issue APP-1.",
  },
]);
assertEqual(
  interleavedJiraActivities.filter(({ title }) => title === "Reading Jira Ticket").length,
  1,
  "non-adjacent tool completion should reconcile the original business activity",
);
assertEqual(
  interleavedJiraActivities.find(({ title }) => title === "Reading Jira Ticket")?.status,
  "completed",
  "non-adjacent tool completion should finalize the original activity",
);
assertEqual(
  interleavedJiraActivities.filter(({ status }) => status === "running").length,
  1,
  "non-adjacent completion should preserve only the current running activity",
);

assertEqual(
  timelineActivities([
    {
      id: "runtime",
      at: "04:30:08 AM",
      tone: "info",
      label: "runtime",
      message: "SUMMARY: Agent runtime started.",
    },
    {
      id: "board",
      at: "04:30:09 AM",
      tone: "info",
      label: "board",
      message: "SUMMARY: Board updated.",
    },
  ]).length,
  0,
  "internal runtime and board synchronization events should not create activities",
);

const projectionSession = createAgentRunSession(
  "card-1",
  "Projection",
  "running",
  55,
  "",
  null,
  [],
  [],
  null,
  [],
  null,
);
let pendingProjection = emptyAgentEventProjection();
for (let sequence = 1; sequence <= 120; sequence += 1) {
  pendingProjection = mergeAgentEventProjection(
    pendingProjection,
    projectAgentTaskSessionEvent(
      {
        id: sequence,
        session_id: 1,
        attempt_id: 1,
        fencing_token: 1,
        sequence,
        kind: "runtime",
        payload: { type: "text_delta", text: "token" },
        progress: { phase: "executing_runtime", completed: 1, total: null },
        created_at: sequence,
      },
      `delta-${sequence}`,
      "04:30:10 AM",
    ),
  );
}
assertEqual(pendingProjection.logs.length, 0, "text deltas should not create presentation logs");
const traceProjection = projectAgentTaskSessionEvent(
  {
    id: 121,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 121,
    kind: "runtime",
    payload: {
      type: "execution_trace_stage",
      schema_version: 1,
      stage: "runtime_preparation",
      duration_us: 420_000,
    },
    progress: null,
    created_at: 121,
  },
  "trace-121",
  "04:30:10 AM",
);
assertEqual(
  traceProjection.logs.length,
  0,
  "developer trace events should not leak into user-facing Activity",
);
const recoveryProjection = projectAgentTaskSessionEvent(
  {
    id: 122,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 122,
    kind: "runtime",
    payload: {
      type: "runtime_recovery_decision",
      failure_class: "transient_transport",
      action: "retry_current_assignment",
      reason: "The failure is transient and safe to retry.",
    },
    progress: null,
    created_at: 122,
  },
  "recovery-122",
  "04:30:10 AM",
);
assertEqual(recoveryProjection.logs.length, 1, "runtime recovery should be visible in Activity");
assertEqual(
  (recoveryProjection.logs[0]?.message ?? "").includes("retrying safely"),
  true,
  "runtime recovery should explain the bounded retry",
);
const leaseRecoveryFenceProjection = projectAgentTaskSessionEvent(
  {
    id: 123,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 123,
    kind: "lifecycle",
    payload: {
      state: "blocked",
      reason: "recovery_uncertain_mutation",
      recovery: "operator_reconciliation",
      action: "operator_reconciliation",
      uncertain_mutation_count: 1,
    },
    progress: { phase: "blocked", completed: 0, total: null },
    created_at: 123,
  },
  "lease-recovery-123",
  "04:30:10 AM",
);
assertEqual(
  (leaseRecoveryFenceProjection.logs[0]?.message ?? "").includes(
    "Recovery stopped before reassignment",
  ),
  true,
  "lease recovery should explain why an uncertain mutation prevents reassignment",
);
assertEqual(
  (leaseRecoveryFenceProjection.logs[0]?.message ?? "").includes(
    "No replacement Worker was allowed",
  ),
  true,
  "lease recovery should explain that replacement authority was not issued",
);
const leaseRecoveryActivities = timelineActivities(leaseRecoveryFenceProjection.logs);
assertEqual(
  leaseRecoveryActivities[0]?.title,
  "Recovery Paused Safely",
  "uncertain-mutation lease recovery should remain visible in the user-facing timeline",
);
assertEqual(
  leaseRecoveryActivities[0]?.summary.includes("Recovery stopped before reassignment"),
  true,
  "uncertain-mutation lease recovery should preserve its safe user-facing reason",
);
const connectorRecoveryProjection = projectAgentTaskSessionEvent(
  {
    id: 124,
    session_id: 1,
    attempt_id: 2,
    fencing_token: 2,
    sequence: 124,
    kind: "runtime",
    payload: {
      type: "connector_session_recovered",
      schema_version: 1,
      provider: "confluence",
      connector_id: "corporate-confluence",
      operation_risk: "read",
      connector_attempts: 2,
    },
    progress: null,
    created_at: 124,
  },
  "connector-recovery-124",
  "04:30:11 AM",
);
const connectorRecoveryActivities = timelineActivities(connectorRecoveryProjection.logs);
assertEqual(
  connectorRecoveryActivities[0]?.title,
  "Connector Session Recovered",
  "bounded read-only connector recovery should remain visible in Activity",
);
assertEqual(
  connectorRecoveryActivities[0]?.summary.includes("read-only verification resumed safely"),
  true,
  "connector recovery should explain the bounded safe replay",
);
assertEqual(
  connectorRecoveryProjection.logs[0]?.message.includes("No mutation authority was replayed"),
  true,
  "connector recovery should state that mutation authority was not expanded",
);
const subtaskPreparationProjection = projectAgentTaskSessionEvent(
  {
    id: 125,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 125,
    kind: "runtime",
    payload: {
      type: "subtask_contracts_prepared",
      schema_version: 1,
      subtask_count: 2,
      dormant_fence_count: 2,
      tool_call_budget: 64,
      mutation_call_budget: 1,
      scheduler_state: "dormant",
      activation_gate: "closed",
      budget_admission: "atomic_before_forward",
      dispatch_lifecycle: "staged",
      lease_recovery: "fail_closed",
      authority_scope: "deterministic_objective_subset",
      grant_policy: "objective_tool_operations_v3",
      evidence_gate: "independent_attestation_required",
      evidence_aggregation: "all_verified_and_completed",
      verifier_binding_policy: "objective_terminal_state_v2",
      assigned_verifier_count: 2,
      unassigned_verifier_count: 0,
      verified_subtask_count: 0,
      rejected_subtask_count: 0,
      parent_capability_count: 4,
      aggregate_capability_grants: 5,
      aggregate_connector_operation_grants: 3,
      narrowed_subtask_count: 2,
      authority_active: false,
      delegation_allowed: false,
      execution_enabled: false,
    },
    progress: null,
    created_at: 125,
  },
  "subtask-preparation-125",
  "04:30:12 AM",
);
const subtaskPreparationActivities = timelineActivities(subtaskPreparationProjection.logs);
assertEqual(
  subtaskPreparationActivities[0]?.title,
  "Subtask Authority Prepared",
  "prepared subtask authority should remain visible in Activity",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Git/Kubernetes verifiers assigned to exact objectives: 2",
  ),
  true,
  "subtask preparation should expose only the safe assigned verifier count",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Scheduler attestations reject verifier identities or methods not assigned by the subtask contract",
  ),
  true,
  "subtask preparation should explain exact verifier admission",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Kubernetes verifier authority binds a digested Deployment identity and remains read-only",
  ),
  true,
  "subtask preparation should explain Kubernetes verifier identity and authority",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Dispatch lifecycle: staged; specialized Workers remain disabled",
  ),
  true,
  "subtask preparation should explain that dispatch lifecycle is staged only",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Lease expiry, cancellation, and parent completion revoke staged authority",
  ),
  true,
  "subtask preparation should explain fail-closed lifecycle recovery",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Grant policy: deterministic per-objective capabilities and connector operations",
  ),
  true,
  "subtask preparation should explain deterministic authority narrowing",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes("Exact connector operation grants: 3"),
  true,
  "subtask preparation should expose only the safe aggregate connector operation count",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Unlisted connector tools are rejected before forwarding",
  ),
  true,
  "subtask preparation should explain exact connector operation enforcement",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Evidence gate: independent verifier attestation required",
  ),
  true,
  "subtask preparation should explain the independent evidence gate",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Parent aggregation requires every subtask to be verified and completed",
  ),
  true,
  "subtask preparation should explain the all-objective aggregation boundary",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Evidence payloads remain digest-only; Worker claims cannot satisfy this gate",
  ),
  true,
  "subtask preparation should distinguish verifier evidence from Worker claims",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Built-in mutation authority requires local scope and a mutation objective",
  ),
  true,
  "subtask preparation should explain the built-in mutation gate",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Objectives with narrowed authority: 2 of 2",
  ),
  true,
  "subtask preparation should expose only the safe aggregate narrowing count",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Subtasks cannot delegate authority to another agent",
  ),
  true,
  "subtask preparation should explain its non-delegable authority boundary",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes("No additional Worker was started"),
  true,
  "subtask preparation should not imply that multi-agent execution is enabled",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes("Dormant fencing identities: 2"),
  true,
  "subtask preparation should expose scheduler-owned dormant fencing",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes("Tool authority active: no"),
  true,
  "subtask preparation should expose that dormant records cannot call tools",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes("Activation gate: closed"),
  true,
  "subtask preparation should expose that scheduler dispatch remains unavailable",
);
assertEqual(
  subtaskPreparationProjection.logs[0]?.message.includes(
    "Tool budgets will be charged atomically before forwarding",
  ),
  true,
  "subtask preparation should explain the staged budget boundary",
);
const objectiveCheckpointProjection = projectAgentTaskSessionEvent(
  {
    id: 124,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 124,
    kind: "runtime",
    payload: {
      type: "objective_checkpointed",
      objective_id: "deploy-bamboo-build",
      evidence: ["Bamboo build 842 completed successfully."],
      tool_receipt_count: 1,
      tool_receipts: [
        {
          tool_call_id: "bamboo-842",
          tool_name: "bamboo_trigger_build",
          risk: "mutation",
          arguments_digest: "a".repeat(64),
        },
      ],
    },
    progress: null,
    created_at: 124,
  },
  "checkpoint-124",
  "04:30:10 AM",
);
assertEqual(
  (objectiveCheckpointProjection.logs[0]?.message ?? "").includes(
    "will not be repeated if the task continues",
  ),
  true,
  "objective checkpoints should explain continuation behavior",
);
assertEqual(
  (objectiveCheckpointProjection.logs[0]?.message ?? "").includes("Bound successful tool calls: 1"),
  true,
  "objective checkpoints should expose authoritative tool receipt counts",
);
const capabilityRepairProjection = projectAgentTaskSessionEvent(
  {
    id: 123,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 123,
    kind: "runtime",
    payload: {
      type: "capability_repair_decision",
      repairable: true,
      failed_tool: "jira_get_issue",
      connector_id: "jira",
      allowed_alternatives: ["jira_read_issue"],
    },
    progress: null,
    created_at: 123,
  },
  "capability-repair-123",
  "04:30:10 AM",
);
assertEqual(
  (capabilityRepairProjection.logs[0]?.message ?? "").includes("jira_read_issue"),
  true,
  "capability repair should expose only the bounded live alternatives",
);
assertEqual(
  pendingProjection.progress,
  55,
  "repeated deltas should retain one latest progress value",
);
assertEqual(
  applyAgentEventProjection(projectionSession, pendingProjection, 120) === projectionSession,
  true,
  "unchanged progress should preserve session identity and avoid reactive publication",
);
let changingProjection = emptyAgentEventProjection();
for (const [sequence, completed] of [10, 50, 90].entries()) {
  changingProjection = mergeAgentEventProjection(
    changingProjection,
    projectAgentTaskSessionEvent(
      {
        id: sequence + 1,
        session_id: 1,
        attempt_id: 1,
        fencing_token: 1,
        sequence: sequence + 1,
        kind: "progress",
        payload: {},
        progress: { phase: "executing_runtime", completed, total: 100 },
        created_at: sequence + 1,
      },
      `progress-${sequence}`,
      "04:30:11 AM",
    ),
  );
}
assertEqual(changingProjection.logs.length, 1, "same-frame progress logs should coalesce");
assertEqual(
  applyAgentEventProjection(projectionSession, changingProjection, 120).progress,
  67,
  "one publication should expose the latest meaningful progress",
);
const terminalProjection = projectAgentTaskSessionEvent(
  {
    id: 200,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 200,
    kind: "lifecycle",
    payload: { state: "succeeded" },
    progress: null,
    created_at: 200,
  },
  "terminal-200",
  "04:30:12 AM",
);
const duplicateTerminalProjection = mergeAgentEventProjection(
  terminalProjection,
  terminalProjection,
);
assertEqual(
  duplicateTerminalProjection.logs.length,
  1,
  "replaying one durable event should not duplicate its activity log",
);
const terminalSession = applyAgentEventProjection(projectionSession, terminalProjection, 120);
assertEqual(
  applyAgentEventProjection(terminalSession, terminalProjection, 120) === terminalSession,
  true,
  "applying the same terminal projection twice should preserve session identity",
);
const staleRunningProjection = projectAgentTaskSessionEvent(
  {
    id: 199,
    session_id: 1,
    attempt_id: 1,
    fencing_token: 1,
    sequence: 199,
    kind: "lifecycle",
    payload: { state: "running" },
    progress: null,
    created_at: 199,
  },
  "stale-running-199",
  "04:30:11 AM",
);
assertEqual(
  mergeAgentEventProjection(terminalProjection, staleRunningProjection).taskSessionState,
  "succeeded",
  "terminal projection state should not regress during replay reconciliation",
);
assertEqual(
  applyAgentEventProjection(terminalSession, staleRunningProjection, 120) === terminalSession,
  true,
  "terminal session state should reject stale active projections without publishing",
);
if (workspaceContextRevision("context a") === workspaceContextRevision("context b")) {
  throw new Error("workspace context revisions should change with context content");
}

assertEqual(
  applySavedSnapshot("saved plus newer edit", "saved"),
  { savedValue: "saved", dirty: true },
  "an older save must not clear newer edits",
);

assertEqual(
  applySavedSnapshot("saved", "saved"),
  { savedValue: "saved", dirty: false },
  "the persisted current snapshot should be clean",
);

assertEqual(
  {
    typescript: prettierPluginGroupForParser("typescript"),
    javascript: prettierPluginGroupForParser("babel"),
    json: prettierPluginGroupForParser("json"),
    css: prettierPluginGroupForParser("css"),
  },
  { typescript: "typescript", javascript: "babel", json: "babel", css: "postcss" },
  "Prettier should load only the plugin group required by the active parser",
);

const session = createDocumentSession({
  workspaceId: "workspace-one",
  path: "src/main.ts",
  name: "main.ts",
  content: "initial",
  version: "v1",
  rootRevision: 3,
  encoding: "utf8",
  lineEnding: "lf",
});
replaceDocument(session, "changed", "ai");
assertEqual(
  {
    id: session.id,
    snapshot: documentSnapshot(session),
    dirty: session.dirty,
    origin: session.lastOrigin,
  },
  {
    id: "workspace-one:3:src/main.ts",
    snapshot: { value: "changed", revision: 1 },
    dirty: true,
    origin: "ai",
  },
  "document sessions should retain identity, revisions, and transaction origin",
);

markDocumentSaved(session, "changed");
assertEqual(session.dirty, false, "saving the current document session should clear dirty state");

const recoveredSession = createRecoveredDocumentSession({
  workspaceId: "workspace-one",
  path: "src/recovered.ts",
  name: "recovered.ts",
  content: "dirty recovered",
  persistedContent: "disk baseline",
  version: "v2",
  rootRevision: 4,
  encoding: "utf8",
  lineEnding: "lf",
  revision: 6,
  scrollTop: 120,
});
assertEqual(
  {
    value: documentSnapshot(recoveredSession),
    persisted: recoveredSession.persistedValue,
    dirty: recoveredSession.dirty,
    origin: recoveredSession.lastOrigin,
    scrollTop: recoveredSession.scrollTop,
  },
  {
    value: { value: "dirty recovered", revision: 6 },
    persisted: "disk baseline",
    dirty: true,
    origin: "restore",
    scrollTop: 120,
  },
  "recovered document sessions should preserve dirty content separately from disk baseline",
);

const statefulSession = createDocumentSession({
  workspaceId: "workspace-one",
  path: "src/state.ts",
  name: "state.ts",
  content: "initial",
  version: "v1",
  rootRevision: 3,
  encoding: "utf8-bom",
  lineEnding: "crlf",
});
statefulSession.state = EditorState.create({ doc: "initial" });
statefulSession.persistedDoc = statefulSession.state.doc;
replaceDocument(statefulSession, "incremental", "format");
assertEqual(
  {
    value: documentSnapshot(statefulSession).value,
    dirty: statefulSession.dirty,
    origin: statefulSession.lastOrigin,
  },
  { value: "incremental", dirty: true, origin: "format" },
  "CodeMirror state changes should remain in the document session",
);
markDocumentExternalConflict(statefulSession);
assertEqual(
  statefulSession.externalConflict,
  true,
  "external changes should be represented without replacing local document state",
);

const commands = createEditorCommandRegistry();
let commandRuns = 0;
const unregister = commands.register("editor.save", () => {
  commandRuns += 1;
});
assertEqual(commands.execute("editor.save"), true, "registered editor commands should execute");
unregister();
assertEqual(
  { commandRuns, handledAfterDispose: commands.execute("editor.save") },
  { commandRuns: 1, handledAfterDispose: false },
  "disposed editor commands should no longer execute",
);

const treeRows = [
  { entry: { name: "src", path: "src", is_dir: true, size: 0 }, depth: 0 },
  { entry: { name: "main.ts", path: "src/main.ts", is_dir: false, size: 10 }, depth: 1 },
  { entry: { name: "README.md", path: "README.md", is_dir: false, size: 10 }, depth: 0 },
];
const expandedTree = { src: [treeRows[1].entry] };
assertEqual(
  {
    child: fileTreeNavigationIndex(treeRows, 0, "ArrowRight", expandedTree),
    parent: fileTreeNavigationIndex(treeRows, 1, "ArrowLeft", expandedTree),
    end: fileTreeNavigationIndex(treeRows, 0, "End", expandedTree),
  },
  { child: 1, parent: 0, end: 2 },
  "file tree navigation should follow hierarchical keyboard semantics",
);

assertEqual(
  {
    modified: workspaceFileChangeIsStructural("modified"),
    created: workspaceFileChangeIsStructural("created"),
    removed: workspaceFileChangeIsStructural("removed"),
    renamed: workspaceFileChangeIsStructural("renamed"),
  },
  { modified: false, created: true, removed: true, renamed: true },
  "only structural workspace changes should rescan the explorer",
);

assertEqual(
  {
    rust: lspConfigForPath("src/main.rs")?.server_id,
    tsx: lspConfigForPath("src/App.tsx")?.language_id,
    unsupported: lspConfigForPath("README.md"),
  },
  { rust: "rust-analyzer", tsx: "typescriptreact", unsupported: null },
  "language server selection should be deterministic by document path",
);

const aiProposal = createAiEditProposal({
  documentId: "doc-1",
  path: "src/main.ts",
  baseRevision: 4,
  baseValue: "start\nold one\nmiddle\nold two\nend",
  proposedValue: "start\nnew one\nmiddle\nnew two\nend",
  summary: "Update both values",
});
assertEqual(
  aiProposal.hunks.length,
  2,
  "AI proposals should preserve independently reviewable hunks",
);
assertEqual(
  applyAiEditHunks(aiProposal.baseValue, aiProposal.hunks, new Set([aiProposal.hunks[1].id])),
  "start\nold one\nmiddle\nnew two\nend",
  "only selected AI hunks should be applied",
);
assertEqual(
  {
    current: aiEditProposalIsStale(aiProposal, "doc-1", 4),
    changed: aiEditProposalIsStale(aiProposal, "doc-1", 5),
  },
  { current: false, changed: true },
  "AI proposals should become stale when document revisions change",
);
const contextualProposal = createAiEditProposal({
  documentId: "doc-1",
  path: "src/main.ts",
  baseRevision: 4,
  baseValue: "old",
  proposedValue: "new",
  summary: "Use pinned context",
  contextRevisions: { "doc-context": 7 },
});
assertEqual(
  {
    current: aiEditProposalIsStale(contextualProposal, "doc-1", 4, { "doc-context": 7 }),
    changed: aiEditProposalIsStale(contextualProposal, "doc-1", 4, { "doc-context": 8 }),
    closed: aiEditProposalIsStale(contextualProposal, "doc-1", 4, {}),
  },
  { current: false, changed: true, closed: true },
  "AI proposals should become stale when pinned context changes or closes",
);

const unicodeDoc = EditorState.create({ doc: "a😀b\nnext" }).doc;
assertEqual(
  {
    position: offsetToLspPosition(unicodeDoc, 3),
    offset: lspPositionToOffset(unicodeDoc, { line: 0, character: 3 }),
  },
  { position: { line: 0, character: 3 }, offset: 3 },
  "LSP positions should use CodeMirror UTF-16 offsets",
);
assertEqual(
  lspTextEditsToChanges(unicodeDoc, [
    {
      range: { start: { line: 1, character: 0 }, end: { line: 1, character: 4 } },
      new_text: "done",
    },
    {
      range: { start: { line: 0, character: 3 }, end: { line: 0, character: 3 } },
      new_text: "!",
    },
  ]),
  [
    { from: 3, to: 3, insert: "!" },
    { from: 5, to: 9, insert: "done" },
  ],
  "LSP edits should be normalized into ordered atomic editor changes",
);
assertEqual(
  lspTextEditsToChanges(unicodeDoc, [
    {
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } },
      new_text: "first",
    },
    {
      range: { start: { line: 0, character: 2 }, end: { line: 0, character: 4 } },
      new_text: "second",
    },
  ]),
  null,
  "overlapping LSP edits should be rejected instead of partially applied",
);

assertEqual(
  {
    active: shouldPollLspDiagnostics(true, true),
    terminal: shouldPollLspDiagnostics(false, true),
    hidden: shouldPollLspDiagnostics(true, false),
  },
  { active: true, terminal: false, hidden: false },
  "LSP diagnostics polling should stop outside a visible editor",
);

assertEqual(
  {
    select: workspaceChatActionRequiresConfirmation({ type: "select_card", card_id: "card-1" }),
    delete: workspaceChatActionRequiresConfirmation({ type: "delete_card", card_id: "card-1" }),
    agent: workspaceChatActionRequiresConfirmation({ type: "start_agent", ticket: "ABC-1" }),
    description: workspaceChatActionDescription({
      type: "move_card",
      ticket: "ABC-1",
      target: "done",
    }),
  },
  { select: false, delete: true, agent: true, description: "Move ABC-1 to Done" },
  "mutating chat actions should require explicit review",
);

let navigation = createEditorNavigation();
navigation = pushEditorLocation(navigation, { path: "src/main.ts", line: 4, character: 2 });
navigation = pushEditorLocation(navigation, { path: "src/item.ts", line: 8, character: 0 });
const navigationBack = editorNavigationTarget(navigation, -1);
assertEqual(
  {
    canBack: canNavigateEditor(navigation, -1),
    canForward: canNavigateEditor(navigation, 1),
    target: navigationBack?.location,
    backCanForward: navigationBack ? canNavigateEditor(navigationBack.state, 1) : false,
  },
  {
    canBack: true,
    canForward: false,
    target: { path: "src/main.ts", line: 4, character: 2 },
    backCanForward: true,
  },
  "editor definition navigation should preserve back and forward history",
);

const chatSessions = Array.from({ length: 7 }, (_, index) => {
  const session = createWorkspaceChatSession([], `Chat ${index + 1}`);
  return { ...session, id: `session-${index + 1}`, updatedAt: index + 1 };
});
const chatStorage = {
  getItem: () =>
    JSON.stringify({
      workspaceChatSessions: chatSessions,
      workspaceChatActiveSessionId: "session-7",
      workspaceChatMessages: [],
    }),
} as unknown as Storage;
const loadedChatState = loadUiState(chatStorage, { chatMessages: [] });
assertEqual(
  loadedChatState.workspaceChatSessions.map((session) => session.id),
  ["session-7", "session-6", "session-5", "session-4", "session-3", "session-2"],
  "chat session retention should keep the six newest sessions",
);

const emptySession = createWorkspaceChatSession([], "Empty");
const fullSession = createWorkspaceChatSession(
  [
    {
      id: "full-message",
      role: "user",
      text: "Full session history",
    } satisfies WorkspaceChatMessage,
  ],
  "Full",
);
fullSession.id = "session-full";
const isolatedStorage = {
  getItem: () =>
    JSON.stringify({
      workspaceChatSessions: [emptySession, fullSession],
      workspaceChatActiveSessionId: emptySession.id,
      workspaceChatMessages: fullSession.messages,
    }),
} as unknown as Storage;
const isolatedState = loadUiState(isolatedStorage, { chatMessages: [] });
assertEqual(
  isolatedState.workspaceChatSessions.find((session) => session.id === emptySession.id)?.messages,
  [],
  "explicitly empty chat sessions should not inherit another session history",
);

const jiraComment = formatJiraExecutionComment({
  request: "Deploy the three services to prerelease",
  result: {
    summary: "Three services were deployed and the required configuration was verified.",
    evidence: [
      "All pods are ready",
      "contract_id=contract-123 current_step=worker.execute",
      "Automated checks passed",
    ],
    details: ["Added BG_MAPPING_SERVICE to the required ConfigMaps", "worker.execute completed"],
    next: [],
    completion_status: "completed",
    blocked_reason: null,
  },
  runtime: "OpenCode",
  model: "deepseek-v4-flash-free",
  environment: "prerelease",
  revision: "9a66bb095",
});
assertEqual(
  [
    jiraComment.indexOf("h3. Executive Summary"),
    jiraComment.indexOf("h3. Result"),
    jiraComment.indexOf("h3. What Changed"),
    jiraComment.indexOf("h3. Verification"),
    jiraComment.indexOf("h3. Required Action"),
  ].every(
    (index, position, values) => index >= 0 && (position === 0 || index > values[position - 1]),
  ),
  true,
  "Jira execution comments should use outcome-first section ordering",
);
assertEqual(
  {
    success: jiraComment.includes("✅ Success"),
    none: jiraComment.includes("None."),
    hidesInternalState:
      !jiraComment.includes("contract-123") && !jiraComment.includes("worker.execute"),
    technicalEvidence: jiraComment.includes("Technical Evidence"),
  },
  { success: true, none: true, hidesInternalState: true, technicalEvidence: true },
  "Jira execution comments should prioritize readable outcomes and hide internal state",
);

const partialJiraComment = formatJiraExecutionComment({
  request: "Update the deployment configuration",
  result: {
    summary: "Configuration was updated, but deployment verification was blocked.",
    evidence: ["Configuration file was updated"],
    details: [],
    next: [],
    completion_status: "blocked",
    blocked_reason: "Deployment health could not be verified.",
  },
  runtime: "OpenCode",
  model: "model",
  environment: "prerelease",
});
assertEqual(
  {
    partial: partialJiraComment.includes("⚠️ Partial Success"),
    action: partialJiraComment.includes("Deployment health could not be verified."),
  },
  { partial: true, action: true },
  "blocked execution comments should communicate partial outcome and required action",
);
