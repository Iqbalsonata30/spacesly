<script lang="ts">
  import type { AiWorkerTaskResult } from "$lib/ipc";
  import type { AgentRunStatus } from "$lib/agentRun";

  type AgentOutputStatus = "complete" | "blocked" | "timeout" | "working" | "unknown";
  type AgentOutputView = {
    status: AgentOutputStatus;
    label: string;
    summary: string;
    outcome: string;
    evidence: string[];
    changes: string[];
    nextSteps: string[];
  };

  let { output, result, runStatus } = $props<{
    output: string;
    result: AiWorkerTaskResult | null;
    runStatus: AgentRunStatus;
  }>();

  let view = $derived(parseAgentOutput(output, result, runStatus));

  function parseAgentOutput(
    value: string,
    resultValue: AiWorkerTaskResult | null,
    statusValue: AgentRunStatus,
  ): AgentOutputView {
    if (resultValue) {
      const status = resultValue.completion_status === "completed" ? "complete" : "blocked";
      const nextSteps =
        resultValue.next.length > 0
          ? resultValue.next
          : nextStepLines(status, resultValue.details, resultValue.evidence);

      return {
        status,
        label: statusLabel(status),
        summary: resultValue.summary,
        outcome: outcomeText(status, resultValue.summary),
        evidence: resultValue.evidence.slice(0, 6),
        changes: resultValue.details.slice(0, 5),
        nextSteps,
      };
    }

    const text = value.trim();
    const waiting = !text || text === "Waiting for Agent output...";

    if (waiting) {
      const status = displayStatus(statusValue);
      return {
        status,
        label: statusLabel(status),
        summary: text || "Waiting for Agent output...",
        outcome:
          status === "working"
            ? "The Agent is still preparing or executing this task."
            : "No structured result is available yet.",
        evidence: [],
        changes: [],
        nextSteps:
          status === "working" ? ["Wait for the Agent to return evidence or a blocker."] : [],
      };
    }

    const fallbackLines = text
      .split(/\n+/)
      .map((line) => line.trim())
      .filter(Boolean);
    const status = displayStatus(statusValue);
    const summary = fallbackLines[0] || "No summary returned.";
    const detailLines = fallbackLines.slice(1, 8);
    const changes = detailLines.filter(isChangeLine).slice(0, 5);
    const nextSteps = nextStepLines(status, detailLines, []);

    return {
      status,
      label: statusLabel(status),
      summary,
      outcome: outcomeText(status, summary),
      evidence: [],
      changes: changes.length > 0 ? changes : detailLines.slice(0, 4),
      nextSteps,
    };
  }

  function isChangeLine(value: string): boolean {
    return /(changed|updated|created|deleted|patched|deployed|rebuilt|transitioned|commented|verified|committed|pushed|file|jira|bamboo|ocp|pod|deployment)/i.test(
      value,
    );
  }

  function nextStepLines(
    status: AgentOutputStatus,
    details: string[],
    evidence: string[],
  ): string[] {
    if (status === "complete")
      return ["Review the evidence, then keep or sync the completed Jira state."];
    if (status === "timeout")
      return ["Check the Agent runtime output, then continue or retry from this card."];
    if (status === "working") return ["Wait for the Agent to return evidence or a blocker."];

    const blockers = [...details, ...evidence].filter((line) =>
      /(need|needs|blocked|approve|approval|missing|failed|cannot|can't|uncommitted|unpushed|permission)/i.test(
        line,
      ),
    );
    return blockers.length > 0
      ? blockers.slice(0, 3)
      : [
          "Add an operator note or approval in the Agent console, then continue the Agent on this card.",
        ];
  }

  function statusLabel(status: AgentOutputStatus): string {
    if (status === "complete") return "Completed";
    if (status === "blocked") return "Needs attention";
    if (status === "timeout") return "Timed out";
    if (status === "working") return "Working";
    return "Review needed";
  }

  function outcomeText(status: AgentOutputStatus, summary: string): string {
    if (status === "complete") return "The Agent reported the task complete with evidence.";
    if (status === "blocked") return "The Agent stopped before marking the task Done.";
    if (status === "timeout") return "Spacesly stopped waiting before a structured result arrived.";
    if (status === "working") return "The Agent is still running.";
    return summary;
  }

  function displayStatus(status: AgentRunStatus): AgentOutputStatus {
    if (status === "completed") return "complete";
    if (status === "blocked") return "blocked";
    if (status === "timeout") return "timeout";
    return "working";
  }
</script>

<article class={`agent-output-card ${view.status}`} aria-label="Agent result">
  <header class="agent-output-hero">
    <div class="agent-output-mark"><span></span></div>
    <div>
      <p>{view.label}</p>
      <h3>{view.summary}</h3>
      <strong>{view.outcome}</strong>
    </div>
  </header>

  {#if view.nextSteps.length > 0}
    <section class="agent-output-panel next">
      <p>{view.status === "blocked" ? "What needs attention" : "Next"}</p>
      <ul>
        {#each view.nextSteps as line}
          <li>{line}</li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if view.evidence.length > 0}
    <section class="agent-output-panel evidence">
      <p>Evidence</p>
      <ul>
        {#each view.evidence as line}
          <li>{line}</li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if view.changes.length > 0}
    <section class="agent-output-panel changes">
      <p>What happened</p>
      <ul>
        {#each view.changes as line}
          <li>{line}</li>
        {/each}
      </ul>
    </section>
  {/if}
</article>

<style>
  .agent-output-card {
    display: grid;
    align-content: start;
    gap: 10px;
    min-height: 0;
    overflow-y: auto;
    padding: 10px;
  }

  .agent-output-hero,
  .agent-output-panel {
    border: 1px solid #24222b;
    border-radius: 12px;
    background: #15141b;
  }

  .agent-output-hero {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
    gap: 10px;
    padding: 12px;
    background: linear-gradient(135deg, rgba(37, 42, 49, 0.72), #15141b);
  }

  .agent-output-mark {
    display: grid;
    width: 30px;
    height: 30px;
    place-items: center;
    border-radius: 999px;
    background: rgba(184, 214, 228, 0.1);
    color: #b8d6e4;
  }

  .agent-output-mark span {
    width: 10px;
    height: 10px;
    border-radius: 999px;
    background: currentColor;
    box-shadow: 0 0 0 5px rgba(184, 214, 228, 0.08);
  }

  .agent-output-card.complete .agent-output-mark {
    background: rgba(185, 214, 170, 0.12);
    color: #b9d6aa;
  }

  .agent-output-card.blocked .agent-output-mark {
    background: rgba(240, 176, 170, 0.12);
    color: #f0b0aa;
  }

  .agent-output-card.timeout .agent-output-mark {
    background: rgba(240, 190, 120, 0.12);
    color: #f0be78;
  }

  .agent-output-hero div:last-child {
    display: grid;
    gap: 5px;
    min-width: 0;
  }

  .agent-output-hero p,
  .agent-output-panel p {
    margin: 0;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .agent-output-hero h3 {
    margin: 0;
    color: #f1edf5;
    font-size: 15px;
    line-height: 1.35;
  }

  .agent-output-hero strong {
    color: #aaa1c3;
    font-size: 12px;
    font-weight: 700;
    line-height: 1.4;
  }

  .agent-output-panel {
    display: grid;
    gap: 8px;
    padding: 11px 12px;
  }

  .agent-output-panel.next {
    border-color: rgba(240, 176, 170, 0.24);
    background: rgba(55, 30, 32, 0.42);
  }

  .agent-output-card.timeout .agent-output-panel.next {
    border-color: rgba(240, 190, 120, 0.24);
    background: rgba(60, 44, 20, 0.38);
  }

  .agent-output-card.complete .agent-output-panel.next {
    border-color: rgba(185, 214, 170, 0.18);
    background: rgba(43, 56, 38, 0.32);
  }

  .agent-output-panel.evidence {
    border-color: rgba(184, 214, 228, 0.18);
  }

  ul {
    display: grid;
    gap: 7px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    position: relative;
    padding-left: 18px;
    color: #cfc8dc;
    font-size: 12px;
    line-height: 1.45;
  }

  li::before {
    position: absolute;
    top: 0.62em;
    left: 2px;
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: #b8d6e4;
    content: "";
  }

  .agent-output-panel.next li::before {
    background: #f0b0aa;
  }

  .agent-output-card.complete .agent-output-panel.next li::before {
    background: #b9d6aa;
  }

  .agent-output-card.timeout .agent-output-panel.next li::before {
    background: #f0be78;
  }
</style>
