<script lang="ts">
  import {
    AlertCircle,
    Check,
    ChevronDown,
    Circle,
    Clock3,
    ExternalLink,
    FileCode2,
    Loader2,
    Wrench,
    X,
  } from "lucide-svelte";
  import type { AgentPhase } from "$lib/boardWorkflow";
  import type { AiWorkerTaskResult } from "$lib/ipc";
  import type {
    AgentRunLog,
    AgentRunStatus,
    AgentSessionEvent,
    AgentTerminalLine,
  } from "$lib/agentRun";

  type Props = {
    style?: string;
    title: string;
    status: string;
    progress: number;
    phases: AgentPhase[];
    logs: AgentRunLog[];
    transcript: AgentSessionEvent[];
    output: string;
    result: AiWorkerTaskResult | null;
    runStatus: AgentRunStatus;
    terminalLines: AgentTerminalLine[];
    terminalInput: string;
    runCardId: string | null;
    onClose: () => void;
    onTerminalInputChange: (value: string) => void;
    onSubmitTerminalInput: () => void;
    onOpenCard: (cardId: string) => void;
    onMarkBlockedDone: (cardId: string) => void;
  };

  let {
    style,
    title,
    status: _status,
    progress,
    phases,
    logs,
    transcript,
    output,
    result,
    runStatus,
    terminalLines,
    terminalInput,
    runCardId,
    onClose,
    onTerminalInputChange,
    onSubmitTerminalInput,
    onOpenCard,
    onMarkBlockedDone,
  }: Props = $props();

  let technicalOpen = $state(false);
  let attentionOpen = $state(true);
  let expandedActivities = $state<Record<string, boolean>>({});

  let isWorking = $derived(runStatus === "running");
  let isBlocked = $derived(runStatus === "blocked" || runStatus === "timeout");
  let isComplete = $derived(runStatus === "completed");
  let statusLabel = $derived(
    isComplete
      ? "Completed"
      : isBlocked
        ? "Needs your attention"
        : isWorking
          ? "Working"
          : "Preparing",
  );
  let userActivity = $derived(userFacingActivity(runStatus, progress));
  let summary = $derived(result?.summary?.trim() || userFacingSummary(runStatus));
  let attentionMessage = $derived(
    runStatus === "timeout"
      ? "The Agent stopped waiting for a response. Review the task and continue or retry when ready."
      : result?.blocked_reason?.trim() ||
          "The Agent paused before it could finish. Review the task for the next action.",
  );
  let resultItems = $derived(
    result ? usefulResultLines([...result.details, ...result.evidence]).slice(0, 8) : [],
  );
  let warnings = $derived(
    (result?.details ?? [])
      .filter((line) =>
        /(warning|warn|failed|error|blocked|missing|uncommitted|unpushed)/i.test(line),
      )
      .map(cleanLine)
      .filter(Boolean)
      .slice(0, 4),
  );

  function phaseTitle(phase: AgentPhase): string {
    return (
      {
        prepare: "Planning",
        jira: "Repository",
        model: "Ready to work",
        execute: "Executing task",
        writeback: "Reviewing changes",
        done: "Completed",
      }[phase.key] ?? phase.label
    );
  }

  function phaseDescription(phase: AgentPhase): string {
    return (
      {
        prepare: "Understanding the task and preparing a plan.",
        jira: "Loading the task workspace and repository context.",
        model: "The Agent is ready with the requested context.",
        execute: "Making and verifying the requested changes.",
        writeback: "Recording the result and preparing the final handoff.",
        done: "The task is complete and ready for review.",
      }[phase.key] ?? "Task stage."
    );
  }

  function phaseIcon(phase: AgentPhase) {
    if (phase.state === "done") return Check;
    if (phase.state === "active") return Loader2;
    if (phase.state === "blocked" || phase.state === "timeout") return AlertCircle;
    return Circle;
  }

  function latestActivities() {
    return logs.slice(-8).reverse();
  }

  function humanActivity(log: AgentRunLog): string {
    const labels: Record<string, string> = {
      start: "Agent started",
      continue: "Agent continued the task",
      board: "Task moved into active work",
      model: "Agent is ready to work",
      jira: "Task status updated",
      local: "Task prepared",
      context: "Task context prepared",
      agent: "Agent returned a result",
      blocked: "Task paused",
      timeout: "Agent response timed out",
      approval: "Approval recorded",
      operator: "Note added",
      "manual-done": "Task marked complete",
    };
    return (
      labels[log.label] ??
      (log.tone === "error" ? "Task needs attention" : "Task activity recorded")
    );
  }

  function userFacingActivity(
    currentStatus: AgentRunStatus,
    currentProgress: number,
  ): { title: string; detail: string } {
    if (currentStatus === "completed")
      return { title: "Task complete", detail: "The result is ready for your review." };
    if (currentStatus === "blocked")
      return {
        title: "Waiting for your input",
        detail: "Review the attention item below to continue.",
      };
    if (currentStatus === "timeout")
      return {
        title: "Waiting for a response",
        detail: "You can review the task and retry when ready.",
      };
    if (currentProgress < 15)
      return { title: "Understanding the task", detail: "The Agent is preparing a plan." };
    if (currentProgress < 35)
      return { title: "Getting ready", detail: "The Agent is gathering the context it needs." };
    if (currentProgress < 75)
      return {
        title: "Working on the task",
        detail: "The Agent is making and checking the requested changes.",
      };
    if (currentProgress < 94)
      return {
        title: "Reviewing the result",
        detail: "The Agent is checking its work and preparing the handoff.",
      };
    return { title: "Finishing up", detail: "The Agent is saving the final result." };
  }

  function userFacingSummary(currentStatus: AgentRunStatus): string {
    if (currentStatus === "completed") return "Task completed successfully.";
    if (currentStatus === "blocked") return "The Agent needs your input to continue.";
    if (currentStatus === "timeout") return "The Agent did not return a result in time.";
    return "The Agent is working on this task.";
  }

  function cleanLine(value: string): string {
    return value
      .replace(/^[-*]\s*/, "")
      .replace(/^(EVIDENCE|DETAILS|NEXT|SUMMARY|STATUS):\s*/i, "")
      .trim();
  }

  function usefulResultLines(values: string[]): string[] {
    const hidden =
      /(runtime|operator notes|completion status|blocked reason|evidence lines|detail lines|jira transition|issue transitioned|comment target|sections:|local workspace projection|task description clipped)/i;
    const useful =
      /(file|changed|updated|created|deleted|modified|test|passed|verified|commit|push|pull request|artifact|deploy|built|lint|warning|error)/i;
    return values
      .map(cleanLine)
      .filter((line) => line.length > 0 && !hidden.test(line) && useful.test(line))
      .filter((line, index, all) => all.indexOf(line) === index);
  }

  function toggleActivity(id: string) {
    expandedActivities = { ...expandedActivities, [id]: !expandedActivities[id] };
  }

  function resultLabel(value: string): string {
    if (/^(changed|updated|created|deleted|modified|file)/i.test(value)) return "Files and changes";
    if (/(test|verified|passed|lint|check)/i.test(value)) return "Verification";
    if (/(commit|push|pull request|pr)/i.test(value)) return "Delivery";
    return "Outcome detail";
  }
</script>

<aside class="agent-console-v2" aria-label="Agent workspace" {style}>
  <header class="agent-hero">
    <div class="hero-topline">
      <span class={`status-orb ${runStatus}`}><span></span></span>
      <span class="eyebrow">Agent workspace</span>
      <button class="close-button" type="button" aria-label="Close Agent console" onclick={onClose}
        ><X size={16} /></button
      >
    </div>
    <div class="hero-copy">
      <div class="hero-status">{statusLabel}</div>
      <h2>{title}</h2>
      <p>{summary}</p>
    </div>
    <div class="hero-progress">
      <div class="progress-meta">
        <span
          >{isComplete
            ? "Ready to review"
            : isBlocked
              ? "Action required"
              : "Current activity"}</span
        ><strong>{progress}%</strong>
      </div>
      <div class="progress-track"><span style={`width: ${progress}%`}></span></div>
      <div class="activity-now">
        <span class="pulse-dot"></span><strong>{userActivity.title}</strong><span
          >{userActivity.detail}</span
        >
      </div>
    </div>
    {#if runCardId}
      <div class="hero-actions">
        <button type="button" class="quiet-button" onclick={() => onOpenCard(runCardId)}
          ><ExternalLink size={14} /> Open task</button
        >
        {#if isComplete}<span class="completion-note"
            ><Check size={14} /> Verified result available</span
          >{/if}
      </div>
    {/if}
  </header>

  {#if isBlocked}
    <section
      class="attention-card"
      class:collapsed={!attentionOpen}
      aria-label="Needs your attention"
    >
      <button
        class="section-heading attention-heading"
        type="button"
        onclick={() => (attentionOpen = !attentionOpen)}
      >
        <span><AlertCircle size={15} /> Needs your attention</span><ChevronDown
          size={15}
          class={attentionOpen ? "" : "rotated"}
        />
      </button>
      {#if attentionOpen}
        <div class="attention-body">
          <p>{attentionMessage}</p>
          <div class="attention-actions">
            {#if runCardId}<button
                type="button"
                class="primary-action"
                onclick={() => onOpenCard(runCardId)}>Review task</button
              >{/if}
            {#if runCardId && runStatus === "blocked"}<button
                type="button"
                class="secondary-action"
                onclick={() => onMarkBlockedDone(runCardId)}>Mark done manually</button
              >{/if}
          </div>
        </div>
      {/if}
    </section>
  {/if}

  <section class="console-section timeline-section" aria-label="Execution timeline">
    <div class="section-heading static-heading">
      <span>Execution timeline</span><small
        >{isComplete ? "Finished" : isBlocked ? "Paused" : "In progress"}</small
      >
    </div>
    <div class="timeline">
      {#each phases as phase, index (phase.key)}
        {@const Icon = phaseIcon(phase)}
        <article
          class:active={phase.state === "active"}
          class:done={phase.state === "done"}
          class:blocked={phase.state === "blocked" || phase.state === "timeout"}
          class="timeline-item"
        >
          <div class="timeline-rail">
            <span class={`timeline-icon ${phase.state}`}><Icon size={13} /></span
            >{#if index < phases.length - 1}<span class="timeline-line"></span>{/if}
          </div>
          <div class="timeline-content">
            <div class="timeline-title">
              <strong>{phaseTitle(phase)}</strong><span
                >{phase.state === "done"
                  ? "Completed"
                  : phase.state === "active"
                    ? "Running"
                    : phase.state === "blocked" || phase.state === "timeout"
                      ? "Needs attention"
                      : "Pending"}</span
              >
            </div>
            {#if phase.state === "active" || phase.state === "blocked" || phase.state === "timeout"}<p
              >
                {phaseDescription(phase)}
              </p>{/if}
          </div>
        </article>
      {/each}
    </div>
  </section>

  {#if result || isComplete || isBlocked}
    <section class="console-section result-section" aria-label="Result">
      <div class="section-heading static-heading">
        <span>Result</span><small
          >{isComplete ? "Ready to review" : isBlocked ? "Incomplete" : "Waiting"}</small
        >
      </div>
      <div class="result-summary">
        <span class={`result-mark ${isComplete ? "success" : isBlocked ? "danger" : "neutral"}`}
          >{#if isComplete}<Check size={16} />{:else if isBlocked}<AlertCircle
              size={16}
            />{:else}<Clock3 size={16} />{/if}</span
        >
        <div>
          <strong
            >{result?.summary ??
              (isBlocked
                ? "The Agent needs input before it can finish."
                : "Waiting for execution...")}</strong
          >
          <p>
            {isComplete
              ? "Review the activity and changes below before closing this task."
              : attentionMessage}
          </p>
        </div>
      </div>
      {#if resultItems.length > 0 || warnings.length > 0}
        <div class="result-grid">
          {#each resultItems.slice(0, 6) as item, index (item + index)}
            <div class="result-item">
              <FileCode2 size={14} />
              <div><span>{resultLabel(item)}</span><strong>{cleanLine(item)}</strong></div>
            </div>
          {/each}
          {#if warnings.length > 0}<div class="result-item warning">
              <AlertCircle size={14} />
              <div>
                <span>Warnings</span><strong
                  >{warnings.length} item{warnings.length === 1 ? "" : "s"} need review</strong
                >
              </div>
            </div>{/if}
        </div>
      {/if}
    </section>
  {/if}

  <section class="console-section activity-section" aria-label="Activity feed">
    <div class="section-heading static-heading">
      <span>Latest activity</span><small>{logs.length} event{logs.length === 1 ? "" : "s"}</small>
    </div>
    <div class="activity-feed">
      {#if latestActivities().length === 0}
        <div class="empty-activity">The Agent will show meaningful progress here as it works.</div>
      {:else}
        {#each latestActivities() as log (log.id)}
          <article
            class="activity-item"
            class:error={log.tone === "error"}
            class:success={log.tone === "success"}
          >
            <span class="activity-marker"><span></span></span>
            <div class="activity-copy">
              <div><strong>{humanActivity(log)}</strong><time>{log.at}</time></div>
              {#if expandedActivities[log.id]}<details open>
                  <summary>Details</summary>
                  <pre>{log.message}</pre>
                </details>{/if}
            </div>
            <button
              class="activity-expand"
              type="button"
              aria-label="Show activity details"
              onclick={() => toggleActivity(log.id)}
              ><ChevronDown size={14} class={expandedActivities[log.id] ? "rotated" : ""} /></button
            >
          </article>
        {/each}
      {/if}
    </div>
  </section>

  <section class="technical-drawer" class:open={technicalOpen} aria-label="Technical console">
    <button class="technical-toggle" type="button" onclick={() => (technicalOpen = !technicalOpen)}
      ><span><Wrench size={14} /> Technical console</span><ChevronDown
        size={15}
        class={technicalOpen ? "rotated" : ""}
      /></button
    >
    {#if technicalOpen}
      <div class="technical-body">
        <details open>
          <summary>Runtime output</summary>
          <pre>{output || "No raw output yet."}</pre>
        </details>
        <details>
          <summary>Session events ({transcript.length})</summary>
          <div class="technical-events">
            {#each transcript as event (event.id)}<div>
                <time>{new Date(event.at).toLocaleTimeString()}</time><span>{event.type}</span>
                <p>{event.text}</p>
              </div>{/each}
          </div>
        </details>
        <details>
          <summary>Terminal ({terminalLines.length})</summary>
          <pre>{terminalLines.map((line) => `${line.prompt}$ ${line.text}`).join("\n") ||
              "No terminal activity."}</pre>
        </details>
        <form
          class="operator-form"
          onsubmit={(event) => {
            event.preventDefault();
            onSubmitTerminalInput();
          }}
        >
          <input
            placeholder="Add a note or approval"
            value={terminalInput}
            oninput={(event) => onTerminalInputChange(event.currentTarget.value)}
          /><button type="submit">Send</button>
        </form>
      </div>
    {/if}
  </section>
</aside>

<style>
  .agent-console-v2 {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
    overflow: auto;
    background: #101017;
    color: #f4f0f8;
  }
  .agent-hero {
    padding: 18px 18px 16px;
    border-bottom: 1px solid #282530;
    background:
      radial-gradient(circle at 100% 0%, rgba(117, 94, 255, 0.18), transparent 38%),
      linear-gradient(145deg, #1b1923, #121119);
  }
  .hero-topline,
  .progress-meta,
  .activity-now,
  .hero-actions,
  .section-heading,
  .timeline-title,
  .result-summary,
  .result-item,
  .activity-item,
  .activity-item > div:first-of-type,
  .technical-toggle,
  .operator-form {
    display: flex;
    align-items: center;
  }
  .hero-topline {
    gap: 8px;
  }
  .eyebrow,
  .section-heading,
  .result-item span,
  .empty-activity {
    color: #9790ae;
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.13em;
    text-transform: uppercase;
  }
  .status-orb {
    display: inline-flex;
    width: 9px;
    height: 9px;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: #625b70;
  }
  .status-orb span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: #c9c1d7;
  }
  .status-orb.running {
    background: #8268ff;
    box-shadow: 0 0 0 5px rgba(130, 104, 255, 0.12);
  }
  .status-orb.running span {
    background: #fff;
    animation: agent-pulse 1.8s ease-in-out infinite;
  }
  .status-orb.completed {
    background: #6fcb8b;
  }
  .status-orb.blocked,
  .status-orb.timeout {
    background: #e48e76;
  }
  .close-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    margin-left: auto;
    border: 1px solid #353140;
    border-radius: 9px;
    background: transparent;
    color: #aaa1bb;
  }
  .hero-copy {
    margin-top: 20px;
  }
  .hero-status {
    color: #a898ff;
    font-size: 12px;
    font-weight: 900;
  }
  .hero-copy h2 {
    margin: 6px 0 0;
    font-size: 20px;
    line-height: 1.12;
    letter-spacing: -0.03em;
  }
  .hero-copy p {
    max-height: 42px;
    margin: 8px 0 0;
    overflow: hidden;
    color: #b4adbf;
    font-size: 12px;
    line-height: 1.45;
  }
  .hero-progress {
    margin-top: 22px;
  }
  .progress-meta {
    justify-content: space-between;
    color: #9891aa;
    font-size: 11px;
    font-weight: 800;
  }
  .progress-meta strong {
    color: #f7f2fb;
    font-size: 15px;
  }
  .progress-track {
    height: 5px;
    margin-top: 8px;
    overflow: hidden;
    border-radius: 999px;
    background: #302b3a;
  }
  .progress-track span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, #775cff, #b19cff);
    transition: width 0.35s ease;
  }
  .activity-now {
    gap: 7px;
    margin-top: 11px;
    color: #aaa3b5;
    font-size: 11px;
  }
  .activity-now strong {
    color: #eee8f3;
  }
  .pulse-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #927cff;
  }
  .hero-actions {
    gap: 12px;
    margin-top: 16px;
  }
  .quiet-button,
  .primary-action,
  .secondary-action,
  .operator-form button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 30px;
    border-radius: 9px;
    padding: 0 10px;
    font-size: 11px;
    font-weight: 900;
  }
  .quiet-button,
  .secondary-action {
    border: 1px solid #393442;
    background: #211e2b;
    color: #dbd4e6;
  }
  .completion-note {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: #82d39a;
    font-size: 11px;
    font-weight: 850;
  }
  .console-section,
  .attention-card {
    margin: 12px 14px 0;
    border: 1px solid #292631;
    border-radius: 13px;
    background: #17151e;
  }
  .section-heading {
    justify-content: space-between;
    min-height: 38px;
    padding: 0 12px;
    border-bottom: 1px solid #292631;
  }
  .section-heading.static-heading small {
    color: #777087;
    font-size: 10px;
    font-weight: 800;
    letter-spacing: 0;
    text-transform: none;
  }
  .attention-heading {
    width: 100%;
    justify-content: space-between;
    border: 0;
    color: #f0b19a;
    background: transparent;
    text-align: left;
  }
  .attention-heading span {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .attention-card {
    border-color: rgba(221, 128, 101, 0.34);
    background: rgba(100, 48, 44, 0.15);
  }
  .attention-body {
    padding: 12px;
  }
  .attention-body p {
    margin: 0;
    color: #d8c5c4;
    font-size: 12px;
    line-height: 1.5;
  }
  .attention-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 12px;
  }
  .primary-action {
    border: 0;
    background: #795fff;
    color: #fff;
  }
  .timeline {
    padding: 12px;
  }
  .timeline-item {
    display: flex;
    min-height: 45px;
  }
  .timeline-rail {
    position: relative;
    display: flex;
    flex: 0 0 25px;
    justify-content: center;
  }
  .timeline-icon {
    z-index: 1;
    display: inline-flex;
    width: 21px;
    height: 21px;
    align-items: center;
    justify-content: center;
    border: 1px solid #393442;
    border-radius: 50%;
    background: #1a1821;
    color: #777087;
  }
  .timeline-icon.done {
    border-color: rgba(115, 205, 139, 0.5);
    color: #79cf91;
  }
  .timeline-icon.active {
    border-color: #866fff;
    color: #b0a2ff;
  }
  .timeline-icon.blocked,
  .timeline-icon.timeout {
    border-color: rgba(225, 134, 106, 0.5);
    color: #e79a80;
  }
  .timeline-line {
    position: absolute;
    top: 21px;
    bottom: -2px;
    width: 1px;
    background: #36313e;
  }
  .timeline-content {
    flex: 1;
    min-width: 0;
    padding: 1px 0 13px 10px;
  }
  .timeline-title {
    justify-content: space-between;
    gap: 10px;
  }
  .timeline-title strong {
    color: #e9e3ed;
    font-size: 12px;
  }
  .timeline-title span {
    color: #787080;
    font-size: 10px;
    font-weight: 800;
  }
  .timeline-content p {
    margin: 4px 0 0;
    color: #8f879a;
    font-size: 11px;
    line-height: 1.35;
  }
  .result-summary {
    gap: 10px;
    padding: 13px;
  }
  .result-mark {
    display: inline-flex;
    flex: 0 0 auto;
    width: 31px;
    height: 31px;
    align-items: center;
    justify-content: center;
    border-radius: 10px;
    background: #25212e;
    color: #b6aaff;
  }
  .result-mark.success {
    background: rgba(94, 180, 117, 0.16);
    color: #83d397;
  }
  .result-mark.danger {
    background: rgba(220, 115, 91, 0.16);
    color: #eea286;
  }
  .result-summary strong {
    color: #eee8f3;
    font-size: 12px;
  }
  .result-summary p {
    margin: 4px 0 0;
    color: #918999;
    font-size: 11px;
    line-height: 1.35;
  }
  .result-grid {
    display: grid;
    gap: 7px;
    padding: 0 12px 12px;
  }
  .result-item {
    align-items: flex-start;
    gap: 8px;
    padding: 9px;
    border: 1px solid #2d2936;
    border-radius: 9px;
    color: #9c90d8;
  }
  .result-item > div {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .result-item span {
    letter-spacing: 0.06em;
  }
  .result-item strong {
    overflow: hidden;
    color: #d8d1df;
    font-size: 11px;
    font-weight: 750;
    line-height: 1.35;
    text-overflow: ellipsis;
  }
  .result-item.warning {
    color: #e49a80;
  }
  .activity-feed {
    padding: 5px 12px 10px;
  }
  .activity-item {
    gap: 8px;
    min-width: 0;
    padding: 9px 0;
    border-bottom: 1px solid #28242f;
  }
  .activity-item:last-child {
    border-bottom: 0;
  }
  .activity-marker {
    display: inline-flex;
    flex: 0 0 14px;
    width: 14px;
    height: 14px;
    align-items: center;
    justify-content: center;
    border: 1px solid #5a5267;
    border-radius: 50%;
  }
  .activity-marker span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: #8b79ea;
  }
  .activity-item.success .activity-marker {
    border-color: #66b77d;
  }
  .activity-item.success .activity-marker span {
    background: #7cd394;
  }
  .activity-item.error .activity-marker {
    border-color: #d17762;
  }
  .activity-item.error .activity-marker span {
    background: #e48e76;
  }
  .activity-copy {
    flex: 1;
    min-width: 0;
  }
  .activity-copy > div {
    justify-content: space-between;
    gap: 8px;
  }
  .activity-copy strong {
    display: block;
    overflow: hidden;
    color: #d9d2df;
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .activity-copy time {
    flex: 0 0 auto;
    color: #70687a;
    font-size: 10px;
  }
  .activity-expand {
    display: inline-flex;
    border: 0;
    background: transparent;
    color: #777080;
  }
  .activity-copy details {
    margin-top: 6px;
  }
  .activity-copy summary {
    color: #978ca5;
    font-size: 10px;
    cursor: pointer;
  }
  .activity-copy pre,
  .technical-body pre {
    margin: 6px 0 0;
    overflow: auto;
    color: #a9a0b2;
    font:
      10px/1.45 ui-monospace,
      SFMono-Regular,
      monospace;
    white-space: pre-wrap;
  }
  .technical-drawer {
    margin: 12px 14px 14px;
    border: 1px solid #292631;
    border-radius: 13px;
    background: #121119;
  }
  .technical-toggle {
    justify-content: space-between;
    width: 100%;
    min-height: 42px;
    border: 0;
    padding: 0 12px;
    background: transparent;
    color: #9991a6;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.02em;
    text-align: left;
  }
  .technical-toggle span {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .technical-body {
    display: grid;
    gap: 9px;
    padding: 0 12px 12px;
  }
  .technical-body details {
    padding: 8px 0;
    border-top: 1px solid #292631;
  }
  .technical-body summary {
    color: #a39aaa;
    font-size: 10px;
    font-weight: 850;
    cursor: pointer;
  }
  .technical-events {
    display: grid;
    gap: 7px;
    max-height: 160px;
    margin-top: 7px;
    overflow: auto;
  }
  .technical-events div {
    padding: 7px;
    border-radius: 8px;
    background: #191720;
  }
  .technical-events time,
  .technical-events span {
    color: #756d7f;
    font-size: 9px;
  }
  .technical-events span {
    margin-left: 8px;
  }
  .technical-events p {
    margin: 4px 0 0;
    color: #aaa1b2;
    font-size: 10px;
    line-height: 1.4;
  }
  .operator-form {
    gap: 7px;
    margin-top: 3px;
  }
  .operator-form input {
    flex: 1;
    min-width: 0;
    border: 1px solid #322d3a;
    border-radius: 8px;
    padding: 8px 9px;
    background: #0f0e14;
    color: #eee8f3;
    font-size: 11px;
  }
  .operator-form button {
    border: 0;
    background: #5f4bd1;
    color: #fff;
  }
  .rotated {
    transform: rotate(-180deg);
  }
  @keyframes agent-pulse {
    0%,
    100% {
      opacity: 0.45;
      transform: scale(0.8);
    }
    50% {
      opacity: 1;
      transform: scale(1.15);
    }
  }
</style>
