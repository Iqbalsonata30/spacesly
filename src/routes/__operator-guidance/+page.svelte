<script lang="ts">
  import { page } from "$app/state";
  import AgentConsolePanel from "$lib/components/AgentConsolePanel.svelte";
  import { projectAgentTaskSessionEvent } from "$lib/agentEventProjection";
  import type { AgentApprovalRequest, AgentRunStatus } from "$lib/agentRun";

  const leaseRecoveryLogs = projectAgentTaskSessionEvent(
    {
      id: 91,
      session_id: 41,
      attempt_id: 2,
      fencing_token: 3,
      sequence: 91,
      kind: "lifecycle",
      payload: {
        state: "blocked",
        reason: "recovery_uncertain_mutation",
        recovery: "operator_reconciliation",
        action: "operator_reconciliation",
        uncertain_mutation_count: 1,
      },
      progress: { phase: "blocked", completed: 0, total: null },
      created_at: 91,
    },
    "lease-recovery-91",
    "02:44:01 PM",
  ).logs;
  const connectorRecoveryLogs = projectAgentTaskSessionEvent(
    {
      id: 92,
      session_id: 42,
      attempt_id: 1,
      fencing_token: 1,
      sequence: 92,
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
      created_at: 92,
    },
    "connector-recovery-92",
    "02:44:02 PM",
  ).logs;
  const connectorRecovery = page.url.searchParams.get("connectorRecovery") === "1";

  let runStatus = $state<AgentRunStatus>(connectorRecovery ? "completed" : "blocked");
  let approval = $state<AgentApprovalRequest | null>(
    page.url.searchParams.get("approval") === "1"
      ? {
          id: "operator-guidance-approval",
          operation: "kubernetes_resources_create",
          argumentsDigest: "a".repeat(64),
          risk: "mutation",
          label: "Create ConfigMap",
          category: "kubernetes",
          target: "prerelease/payroll-config",
          capability: null,
          status: "pending",
        }
      : null,
  );
  let openClicks = $state(0);
  let continueClicks = $state(0);
  let retryFreshClicks = $state(0);
  let approvalClicks = $state(0);
</script>

<AgentConsolePanel
  style="position: relative; width: min(900px, 100vw); height: 100vh;"
  title="Deploy payroll configuration"
  status={runStatus}
  progress={58}
  logs={connectorRecovery ? connectorRecoveryLogs : leaseRecoveryLogs}
  transcript={[]}
  output=""
  result={{
    summary: connectorRecovery
      ? "The Confluence page was verified after connector recovery."
      : "The task needs operator action.",
    evidence: connectorRecovery ? ["Exact Confluence page identity verified."] : [],
    details: [],
    next: [],
    completion_status: connectorRecovery ? "completed" : "blocked",
    blocked_reason: connectorRecovery
      ? null
      : "Raw model-authored fallback should not replace backend guidance.",
    objective_results: [],
  }}
  executionRun={null}
  {runStatus}
  terminalLines={[]}
  terminalInput=""
  runCardId="operator-guidance-card"
  {approval}
  taskSessionId={41}
  cancelPending={false}
  onClose={() => {}}
  onCancel={() => {}}
  onTerminalInputChange={() => {}}
  onSubmitTerminalInput={() => {}}
  onOpenCard={() => (openClicks += 1)}
  onContinue={() => (continueClicks += 1)}
  onRetryFresh={() => (retryFreshClicks += 1)}
  onMarkBlockedDone={() => {}}
  onApprove={() => {
    approvalClicks += 1;
    approval = null;
    runStatus = "running";
  }}
  onDecline={() => {}}
/>

<output id="open-clicks">{openClicks}</output>
<output id="continue-clicks">{continueClicks}</output>
<output id="retry-fresh-clicks">{retryFreshClicks}</output>
<output id="approval-clicks">{approvalClicks}</output>
<output id="task-status">{runStatus}</output>
