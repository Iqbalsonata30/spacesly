<script lang="ts">
  import AgentConsolePanel from "$lib/components/AgentConsolePanel.svelte";
  import type { AgentApprovalRequest, AgentRunStatus } from "$lib/agentRun";

  let runStatus = $state<AgentRunStatus>("blocked");
  let approval = $state<AgentApprovalRequest | null>({
    id: "operator-guidance-approval",
    operation: "kubernetes_resources_create",
    argumentsDigest: "a".repeat(64),
    risk: "mutation",
    label: "Create ConfigMap",
    category: "kubernetes",
    target: "prerelease/payroll-config",
    capability: null,
    status: "pending",
  });
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
  logs={[]}
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
