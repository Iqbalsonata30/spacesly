<script lang="ts">
  import AgentConsolePanel from "$lib/components/AgentConsolePanel.svelte";
  import type { AgentApprovalRequest } from "$lib/agentRun";

  let approval = $state<AgentApprovalRequest | null>({
    id: "approval-test",
    operation: "kubernetes_resources_create",
    argumentsDigest: "digest-test",
    risk: "mutation",
    label: "Create ConfigMap",
    category: "kubernetes",
    target: "spacesly/spacesly-cm",
    capability: null,
    status: "approving",
  });
  let approvalClicks = $state(0);
</script>

<AgentConsolePanel
  style="position: relative; width: min(900px, 100vw); height: 100vh;"
  title="Approval recovery"
  status="Blocked"
  progress={50}
  logs={[]}
  transcript={[]}
  output=""
  result={null}
  executionRun={null}
  runStatus="blocked"
  terminalLines={[]}
  terminalInput=""
  runCardId="card-test"
  {approval}
  cancelPending={false}
  onClose={() => {}}
  onCancel={() => {}}
  onTerminalInputChange={() => {}}
  onSubmitTerminalInput={() => {}}
  onOpenCard={() => {}}
  onMarkBlockedDone={() => {}}
  onApprove={() => {
    approvalClicks += 1;
    approval = null;
  }}
  onDecline={() => {}}
/>

<output id="approval-clicks">{approvalClicks}</output>
