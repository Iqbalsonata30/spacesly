<script lang="ts">
  type AgentRunStatus = "idle" | "running" | "completed" | "blocked";
  type AgentOutputView = {
    status: "complete" | "blocked" | "working" | "unknown";
    summary: string;
    evidence: string[];
    details: string[];
  };

  let { output, runStatus } = $props<{
    output: string;
    runStatus: AgentRunStatus;
  }>();

  let view = $derived(parseAgentOutput(output, runStatus));

  function parseAgentOutput(value: string, statusValue: AgentRunStatus): AgentOutputView {
    const text = value.trim();
    if (!text || text === "Waiting for Agent output..." || text === "Agent is processing the task context...") {
      return {
        status: statusValue === "blocked" ? "blocked" : statusValue === "completed" ? "complete" : "working",
        summary: text || "Waiting for Agent output...",
        evidence: [],
        details: [],
      };
    }

    const sections = labelledSections(text);
    const explicitStatus = sections.STATUS?.join(" ").toLowerCase() ?? "";
    const parsedStatus = explicitStatus.includes("complete")
      ? "complete"
      : explicitStatus.includes("blocked")
        ? "blocked"
        : statusValue === "completed"
          ? "complete"
          : statusValue === "blocked"
            ? "blocked"
            : "unknown";
    const fallbackLines = text.split(/\n+/).map((line) => line.trim()).filter(Boolean);

    return {
      status: parsedStatus,
      summary: sections.SUMMARY?.join(" ") || fallbackLines[0] || "No summary returned.",
      evidence: cleanOutputLines(sections.EVIDENCE ?? []),
      details: cleanOutputLines(sections.DETAILS ?? fallbackLines.slice(1)),
    };
  }

  function labelledSections(value: string): Record<string, string[]> {
    const sections: Record<string, string[]> = {};
    let activeKey: string | null = null;

    for (const rawLine of value.split("\n")) {
      const line = rawLine.trimEnd();
      const match = line.match(/^(STATUS|SUMMARY|EVIDENCE|DETAILS):\s*(.*)$/i);
      if (match) {
        activeKey = match[1].toUpperCase();
        sections[activeKey] = match[2] ? [match[2].trim()] : [];
        continue;
      }

      if (activeKey && line.trim()) {
        sections[activeKey].push(line.trim());
      }
    }

    return sections;
  }

  function cleanOutputLines(lines: string[]): string[] {
    return lines
      .flatMap((line) => line.split("\n"))
      .map((line) => line.trim().replace(/^[-*]\s*/, ""))
      .filter(Boolean)
      .slice(0, 8);
  }
</script>

<div class="agent-output-card">
  <div class={`agent-output-status ${view.status}`}>
    <span></span>
    <strong>{view.status}</strong>
  </div>

  <section class="agent-output-section summary">
    <p>Summary</p>
    <strong>{view.summary}</strong>
  </section>

  {#if view.evidence.length > 0}
    <section class="agent-output-section">
      <p>Evidence</p>
      <ul>
        {#each view.evidence as line}
          <li>{line}</li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if view.details.length > 0}
    <section class="agent-output-section">
      <p>Details</p>
      <ul>
        {#each view.details as line}
          <li>{line}</li>
        {/each}
      </ul>
    </section>
  {/if}

  <details class="agent-output-raw">
    <summary>Raw output</summary>
    <pre>{output}</pre>
  </details>
</div>

<style>
  .agent-output-card {
    display: grid;
    align-content: start;
    gap: 10px;
    min-height: 0;
    overflow-y: auto;
    padding: 10px;
  }

  .agent-output-status {
    display: inline-flex;
    width: fit-content;
    align-items: center;
    gap: 8px;
    border: 1px solid #34313d;
    border-radius: 999px;
    padding: 6px 10px;
    background: #1a1920;
    color: #aaa1c3;
  }

  .agent-output-status span {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: currentColor;
  }

  .agent-output-status strong {
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .agent-output-status.complete {
    border-color: rgba(185, 214, 170, 0.34);
    color: #b9d6aa;
  }

  .agent-output-status.blocked {
    border-color: rgba(240, 176, 170, 0.34);
    color: #f0b0aa;
  }

  .agent-output-status.working {
    border-color: rgba(184, 214, 228, 0.3);
    color: #b8d6e4;
  }

  .agent-output-section {
    display: grid;
    gap: 6px;
    border: 1px solid #24222b;
    border-radius: 10px;
    padding: 10px;
    background: #15141b;
  }

  .agent-output-section.summary {
    border-color: rgba(184, 214, 228, 0.18);
    background: linear-gradient(135deg, rgba(37, 42, 49, 0.58), #15141b);
  }

  .agent-output-section p {
    margin: 0;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .agent-output-section strong {
    color: #f1edf5;
    font-size: 13px;
    line-height: 1.45;
  }

  .agent-output-section ul {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .agent-output-section li {
    position: relative;
    padding-left: 16px;
    color: #c9c1d6;
    font-size: 12px;
    line-height: 1.45;
  }

  .agent-output-section li::before {
    position: absolute;
    top: 0.62em;
    left: 2px;
    width: 5px;
    height: 5px;
    border-radius: 999px;
    background: #b8d6e4;
    content: "";
  }

  .agent-output-raw {
    border: 1px solid #24222b;
    border-radius: 10px;
    background: #0e0e14;
  }

  .agent-output-raw summary {
    cursor: pointer;
    padding: 9px 10px;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .agent-output-raw pre {
    max-height: 220px;
    margin: 0;
    overflow: auto;
    border-top: 1px solid #24222b;
    padding: 10px;
    color: #b9b2c8;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
  }
</style>
