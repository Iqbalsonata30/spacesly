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

  let runStatus = $state<AgentRunStatus>("blocked");
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
  logs={leaseRecoveryLogs}
  transcript={[]}
  output=""
  result={{
    summary: "The task needs operator action.",
    evidence: [],
    details: [],
    next: [],
    completion_status: "blocked",
    blocked_reason: "Raw model-authored fallback should not replace backend guidance.",
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
