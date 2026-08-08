<script lang="ts">
  import { AlertCircle, Check, ChevronDown, Circle, Loader2, X } from "lucide-svelte";
  import type { AiWorkerTaskResult } from "$lib/ipc";
  import { timelineActivities, type TimelineActivity } from "$lib/agentTimeline";
  import type {
    AgentRunLog,
    AgentApprovalRequest,
    AgentRunStatus,
    AgentSessionEvent,
    AgentTerminalLine,
    ExecutionRun,
  } from "$lib/agentRun";
  import { relativeTimeLabel } from "$lib/relativeTime";

  type Props = {
    style?: string;
    title: string;
    status: string;
    progress: number;
    logs: AgentRunLog[];
    transcript: AgentSessionEvent[];
    output: string;
    result: AiWorkerTaskResult | null;
    executionRun: ExecutionRun | null;
    runStatus: AgentRunStatus;
    terminalLines: AgentTerminalLine[];
    terminalInput: string;
    runCardId: string | null;
    approval: AgentApprovalRequest | null;
    cancelPending: boolean;
    onClose: () => void;
    onCancel: (cardId: string) => void;
    onTerminalInputChange: (value: string) => void;
    onSubmitTerminalInput: () => void;
    onOpenCard: (cardId: string) => void;
    onMarkBlockedDone: (cardId: string) => void;
    onApprove: (cardId: string) => void;
    onDecline: (cardId: string) => void;
  };

  let {
    style,
    title,
    status: _status,
    progress,
    logs,
    transcript,
    output,
    result,
    executionRun,
    runStatus,
    terminalLines,
    terminalInput,
    runCardId,
    approval,
    cancelPending,
    onClose,
    onCancel,
    onTerminalInputChange,
    onSubmitTerminalInput,
    onOpenCard,
    onMarkBlockedDone,
    onApprove,
    onDecline,
  }: Props = $props();

  let technicalOpen = $state(false);
  let attentionOpen = $state(true);
  let expandedActivities = $state<Record<string, boolean>>({});
  let timelineNow = $state(Date.now());

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
  let latestActivityItems = $derived(timelineActivities(logs, 10));

  $effect(() => {
    if (!isWorking) return;
    const timer = window.setInterval(() => {
      timelineNow = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });

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
      return {
        title: "Loading execution contract",
        detail: "The planned work is being prepared for execution.",
      };
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

  function statusText(activity: TimelineActivity): string {
    if (activity.status === "completed") return "Completed";
    if (activity.status === "failed") return "Failed";
    if (activity.status === "waiting") return "Waiting";
    if (activity.status === "running") return "Running";
    if (activity.status === "cancelled") return "Cancelled";
    return "Recorded";
  }

  function resultLabel(value: string): string {
    if (/^(changed|updated|created|deleted|modified|file)/i.test(value)) return "Files and changes";
    if (/(test|verified|passed|lint|check)/i.test(value)) return "Verification";
    if (/(commit|push|pull request|pr)/i.test(value)) return "Delivery";
    return "Outcome detail";
  }

  function stepStatusLabel(status: string): string {
    return (
      {
        pending: "Pending",
        ready: "Ready",
        running: "Running",
        completed: "Completed",
        blocked: "Blocked",
        failed: "Failed",
        skipped: "Skipped",
      }[status] ?? status
    );
  }

  function stepSummary(status: string): string {
    return (
      {
        pending: "Waiting for the previous step to finish.",
        ready: "Ready to begin.",
        running: "This step is currently in progress.",
        completed: "This step finished successfully.",
        blocked: "This step needs attention before the task can continue.",
        failed: "This step did not complete successfully.",
        skipped: "This step was not needed.",
      }[status] ?? "Execution status is being determined."
    );
  }

  function stepIcon(status: string) {
    if (status === "completed" || status === "skipped") return Check;
    if (status === "running" || status === "ready") return Loader2;
    if (status === "blocked" || status === "failed") return AlertCircle;
    return Circle;
  }
</script>

<aside class="agent-console-v2" aria-label="Agent workspace" {style}>
  <header class="agent-hero">
    <div class="hero-topline">
      <span class="eyebrow">Agent workspace</span>
      {#if isWorking && runCardId}
        <button
          class="cancel-button"
          type="button"
          disabled={cancelPending}
          onclick={() => onCancel(runCardId)}>{cancelPending ? "Stopping..." : "Stop"}</button
        >
      {/if}
      <button class="close-button" type="button" aria-label="Close Agent console" onclick={onClose}
        ><X size={16} aria-hidden="true" /></button
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
      <div class="progress-track"><span style={`transform: scaleX(${progress / 100})`}></span></div>
      <div class="activity-now">
        <strong>{userActivity.title}</strong><span>{userActivity.detail}</span>
      </div>
    </div>
    {#if runCardId}
      <div class="hero-actions">
        <button type="button" class="quiet-button" onclick={() => onOpenCard(runCardId)}
          >Open task</button
        >
        {#if isComplete}<span class="completion-note">Verified result available</span>{/if}
      </div>
    {/if}
  </header>

  {#if approval && runCardId}
    <section class="approval-card" aria-label="Action approval required" aria-live="polite">
      <div class="approval-heading">
        <div>
          <span class="approval-eyebrow">Approval required</span>
          <h3>{approval.label}</h3>
        </div>
        <span class={`risk-badge ${approval.risk}`}>{approval.risk}</span>
      </div>
      <p>
        Review this action before allowing the Agent to continue. Approval applies only to this
        exact operation and arguments in the next run.
      </p>
      <dl class="approval-details">
        <div>
          <dt>Operation</dt>
          <dd>{approval.operation}</dd>
        </div>
        {#if approval.target}<div>
            <dt>Target</dt>
            <dd>{approval.target}</dd>
          </div>{/if}
        <div>
          <dt>Category</dt>
          <dd>{approval.category}</dd>
        </div>
      </dl>
      {#if approval.status === "declined"}
        <div class="approval-declined">Declined — no action was authorized.</div>
      {:else}
        <div class="approval-actions">
          <button
            type="button"
            class="decline-action"
            disabled={isWorking}
            onclick={() => onDecline(runCardId)}>Decline</button
          >
          <button
            type="button"
            class="approve-action"
            disabled={isWorking}
            title={isWorking ? "Waiting for the Agent to pause safely" : undefined}
            onclick={() => onApprove(runCardId)}
            >{approval.status === "approving" && isWorking
              ? "Approving…"
              : "Approve & Continue"}</button
          >
        </div>
      {/if}
    </section>
  {/if}

  {#if isBlocked}
    <section
      class="attention-card"
      class:collapsed={!attentionOpen}
      aria-label="Needs your attention"
    >
      <button
        class="section-heading attention-heading"
        type="button"
        aria-expanded={attentionOpen}
        onclick={() => (attentionOpen = !attentionOpen)}
      >
        <span><AlertCircle size={16} aria-hidden="true" /> Needs your attention</span><ChevronDown
          size={16}
          aria-hidden="true"
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

  {#if executionRun}
    <section class="console-section timeline-section" aria-label="Execution contract steps">
      <div class="section-heading static-heading">
        <span>Execution plan</span><small>{executionRun.contract.workflow.length} steps</small>
      </div>
      <div class="timeline">
        {#each executionRun.contract.workflow as step, index (step.step_id)}
          {@const stepRun = executionRun.step_runs[step.step_id]}
          {@const Icon = stepIcon(stepRun?.status ?? "pending")}
          <article
            class:active={stepRun?.status === "running" || stepRun?.status === "ready"}
            class:done={stepRun?.status === "completed" || stepRun?.status === "skipped"}
            class:blocked={stepRun?.status === "blocked" || stepRun?.status === "failed"}
            class="timeline-item"
          >
            <div class="timeline-rail">
              <span class={`timeline-icon ${stepRun?.status ?? "pending"}`}
                ><Icon size={14} aria-hidden="true" /></span
              >{#if index < executionRun.contract.workflow.length - 1}<span class="timeline-line"
                ></span>{/if}
            </div>
            <div class="timeline-content">
              <div class="timeline-title">
                <strong>{step.title}</strong><span
                  >{stepStatusLabel(stepRun?.status ?? "pending")}</span
                >
              </div>
              <p>{stepRun?.summary ?? stepSummary(stepRun?.status ?? "pending")}</p>
            </div>
          </article>
        {/each}
      </div>
    </section>
  {/if}

  {#if result || isComplete || isBlocked}
    <section class="console-section result-section" aria-label="Result">
      <div class="section-heading static-heading">
        <span>Result</span><small
          >{isComplete ? "Ready to review" : isBlocked ? "Incomplete" : "Waiting"}</small
        >
      </div>
      <div class="result-summary">
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
              <div><span>{resultLabel(item)}</span><strong>{cleanLine(item)}</strong></div>
            </div>
          {/each}
          {#if warnings.length > 0}<div class="result-item warning">
              <AlertCircle size={16} aria-hidden="true" />
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
      <span>Activity Log</span><small
        >{latestActivityItems.length} activit{latestActivityItems.length === 1 ? "y" : "ies"}</small
      >
    </div>
    <div class="activity-feed" aria-live="polite">
      {#if latestActivityItems.length === 0}
        <div class="empty-activity">The Agent will show meaningful progress here as it works.</div>
      {:else}
        {#each latestActivityItems as activity (activity.id)}
          <article
            class="activity-item"
            class:error={activity.status === "failed"}
            class:success={activity.status === "completed"}
            class:waiting={activity.status === "waiting"}
            class:minor={activity.importance === "minor"}
          >
            <span class="activity-marker" aria-hidden="true"><span></span></span>
            <div class="activity-copy">
              <div class="activity-heading">
                <strong>{activity.title}</strong>
                <button
                  class="activity-expand"
                  type="button"
                  aria-label={expandedActivities[activity.id]
                    ? "Hide Technical Details"
                    : "Show Technical Details"}
                  aria-expanded={expandedActivities[activity.id] ?? false}
                  title="Technical Details"
                  onclick={() => toggleActivity(activity.id)}
                  ><ChevronDown
                    size={14}
                    aria-hidden="true"
                    class={expandedActivities[activity.id] ? "rotated" : ""}
                  /></button
                >
              </div>
              <p class="activity-summary">{activity.summary}</p>
              <div class="activity-meta">
                <time title={activity.log.at}
                  >{relativeTimeLabel(activity.log.at, timelineNow)}</time
                >
                <span>{statusText(activity)}</span>
              </div>
              {#if expandedActivities[activity.id]}
                <div class="activity-details">
                  <h3>Technical Details</h3>
                  {#if activity.repeatCount > 1}<p>
                      {activity.repeatCount} related runtime updates were grouped into this activity.
                    </p>{/if}
                  {#each activity.sections as section (section.title)}
                    <section>
                      <h4>{section.title}</h4>
                      <ul>
                        {#each section.lines as line (line)}
                          <li>{line}</li>
                        {/each}
                      </ul>
                    </section>
                  {/each}
                </div>
              {/if}
            </div>
          </article>
        {/each}
      {/if}
    </div>
  </section>

  <section class="technical-drawer" class:open={technicalOpen} aria-label="Technical console">
    <button
      class="technical-toggle"
      type="button"
      aria-expanded={technicalOpen}
      onclick={() => (technicalOpen = !technicalOpen)}
      ><span>Technical console</span><ChevronDown
        size={16}
        aria-hidden="true"
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
            placeholder="Add an operator note"
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
    background: var(--surface);
    color: var(--text-bright);
  }
  .agent-hero {
    padding: 18px 18px 16px;
    border-bottom: 1px solid var(--border-subtle);
    background:
      radial-gradient(circle at 100% 0%, var(--selection-bg), transparent 38%),
      linear-gradient(145deg, var(--surface-overlay), var(--surface));
  }
  .hero-topline,
  .progress-meta,
  .activity-now,
  .hero-actions,
  .section-heading,
  .timeline-title,
  .result-summary,
  .result-item,
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
    color: var(--text-secondary);
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.13em;
    text-transform: uppercase;
  }
  .close-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    margin-left: auto;
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    background: transparent;
    color: var(--text-secondary);
  }
  .cancel-button {
    margin-left: auto;
    border: 1px solid var(--danger-border);
    border-radius: 8px;
    padding: 5px 9px;
    background: color-mix(in srgb, var(--danger-bg) 78%, transparent);
    color: var(--danger);
    font: inherit;
    font-size: 11px;
    font-weight: 800;
  }
  .cancel-button + .close-button {
    margin-left: 0;
  }
  .hero-copy {
    margin-top: 20px;
  }
  .hero-status {
    color: var(--accent);
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
    color: var(--text-secondary);
    font-size: 12px;
    line-height: 1.45;
  }
  .hero-progress {
    margin-top: 22px;
  }
  .progress-meta {
    justify-content: space-between;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 800;
  }
  .progress-meta strong {
    color: var(--text-bright);
    font-size: 15px;
  }
  .progress-track {
    height: 5px;
    margin-top: 8px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--progress-track);
  }
  .progress-track span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(
      90deg,
      var(--progress-fill),
      color-mix(in srgb, var(--progress-fill) 58%, var(--text-bright))
    );
    transform-origin: left;
    transition: transform 0.35s ease;
  }
  .activity-now {
    gap: 7px;
    margin-top: 11px;
    color: var(--text-secondary);
    font-size: 11px;
  }
  .activity-now strong {
    color: var(--text-bright);
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
    border: 1px solid var(--border-strong);
    background: var(--surface-hover);
    color: var(--text-primary);
  }
  .completion-note {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--success);
    font-size: 11px;
    font-weight: 850;
  }
  .console-section,
  .attention-card,
  .approval-card {
    margin: 12px 14px 0;
    border: 1px solid var(--border-subtle);
    border-radius: 13px;
    background: var(--surface-raised);
  }
  .approval-card {
    padding: 14px;
    border-color: color-mix(in srgb, var(--accent) 38%, var(--border-subtle));
    background: color-mix(in srgb, var(--selection-bg) 36%, var(--surface-raised));
  }
  .approval-heading,
  .approval-actions {
    display: flex;
    align-items: center;
  }
  .approval-heading {
    justify-content: space-between;
    gap: 12px;
  }
  .approval-eyebrow {
    color: var(--accent);
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .approval-heading h3 {
    margin: 4px 0 0;
    overflow-wrap: anywhere;
    font-size: 14px;
    line-height: 1.3;
  }
  .risk-badge {
    flex: 0 0 auto;
    border: 1px solid var(--border-strong);
    border-radius: 999px;
    padding: 3px 7px;
    color: var(--text-secondary);
    font-size: 9px;
    font-weight: 900;
    text-transform: uppercase;
  }
  .risk-badge.destructive,
  .risk-badge.credential_sensitive {
    border-color: var(--danger-border);
    color: var(--danger);
  }
  .approval-card > p {
    margin: 10px 0 0;
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1.5;
  }
  .approval-details {
    display: grid;
    gap: 5px;
    margin: 12px 0 0;
  }
  .approval-details div {
    display: grid;
    grid-template-columns: 68px minmax(0, 1fr);
    gap: 8px;
  }
  .approval-details dt,
  .approval-details dd {
    margin: 0;
    font-size: 10px;
  }
  .approval-details dt {
    color: var(--text-dim);
    font-weight: 800;
  }
  .approval-details dd {
    overflow-wrap: anywhere;
    color: var(--text-primary);
    font-family: var(--font-mono);
  }
  .approval-actions {
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
    padding-top: 12px;
    border-top: 1px solid var(--border-subtle);
  }
  .approval-actions button {
    min-height: 32px;
    border-radius: 8px;
    padding: 0 11px;
    font: inherit;
    font-size: 11px;
    font-weight: 900;
  }
  .decline-action {
    border: 1px solid var(--border-strong);
    background: transparent;
    color: var(--text-primary);
  }
  .approve-action {
    border: 0;
    background: var(--accent);
    color: var(--surface);
  }
  .approval-actions button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .approval-declined {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border-subtle);
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 750;
  }
  .section-heading {
    justify-content: space-between;
    min-height: 38px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .section-heading.static-heading small {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 800;
    letter-spacing: 0;
    text-transform: none;
  }
  .attention-heading {
    width: 100%;
    justify-content: space-between;
    border: 0;
    color: var(--danger);
    background: transparent;
    text-align: left;
  }
  .attention-heading span {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .attention-card {
    border-color: var(--danger-border);
    background: color-mix(in srgb, var(--danger-bg) 72%, transparent);
  }
  .attention-body {
    padding: 12px;
  }
  .attention-body p {
    margin: 0;
    color: var(--text-primary);
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
    background: var(--accent);
    color: var(--surface);
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
    border: 1px solid var(--border-strong);
    border-radius: 50%;
    background: var(--surface-raised);
    color: var(--text-dim);
  }
  .timeline-icon.done {
    border-color: var(--success-border);
    color: var(--success);
  }
  .timeline-icon.completed,
  .timeline-icon.skipped {
    border-color: var(--success-border);
    color: var(--success);
  }
  .timeline-icon.active {
    border-color: var(--focus-border);
    color: var(--accent);
  }
  .timeline-icon.running,
  .timeline-icon.ready {
    border-color: var(--focus-border);
    color: var(--accent);
  }
  .timeline-icon.running :global(svg),
  .timeline-icon.ready :global(svg) {
    animation: timeline-spin 1s linear infinite;
  }
  .timeline-icon.blocked,
  .timeline-icon.timeout,
  .timeline-icon.failed {
    border-color: var(--danger-border);
    color: var(--danger);
  }
  .timeline-line {
    position: absolute;
    top: 21px;
    bottom: -2px;
    width: 1px;
    background: var(--border-strong);
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
    color: var(--text-bright);
    font-size: 12px;
  }
  .timeline-title span {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 800;
  }
  .timeline-content p {
    margin: 4px 0 0;
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1.35;
  }
  @keyframes timeline-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .result-summary {
    gap: 10px;
    padding: 13px;
  }
  .result-summary strong {
    color: var(--text-bright);
    font-size: 12px;
  }
  .result-summary p {
    margin: 4px 0 0;
    color: var(--text-secondary);
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
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    color: var(--info);
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
    color: var(--text-primary);
    font-size: 11px;
    font-weight: 750;
    line-height: 1.35;
    text-overflow: ellipsis;
  }
  .result-item.warning {
    color: var(--warning);
  }
  .activity-feed {
    display: grid;
    gap: 9px;
    padding: 8px 12px 14px;
  }
  .activity-item {
    display: grid;
    grid-template-columns: 14px minmax(0, 1fr);
    gap: 11px;
    min-width: 0;
    padding: 12px 13px;
    border: 1px solid var(--border-subtle);
    border-radius: 13px;
    background: linear-gradient(180deg, var(--surface-raised) 0%, var(--surface) 100%);
    box-shadow: inset 0 1px 0 color-mix(in srgb, var(--text-bright) 3%, transparent);
  }
  .activity-item.minor {
    border-color: var(--border-subtle);
    background: var(--surface);
  }
  .activity-item.error {
    border-color: var(--danger-border);
    background: linear-gradient(180deg, var(--danger-bg), var(--surface));
  }
  .activity-item.success {
    border-color: var(--success-border);
  }
  .activity-marker {
    display: inline-flex;
    flex: 0 0 14px;
    width: 14px;
    height: 14px;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border-strong);
    border-radius: 50%;
    margin-top: 2px;
  }
  .activity-marker span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
  }
  .activity-item.success .activity-marker {
    border-color: var(--success-border);
  }
  .activity-item.success .activity-marker span {
    background: var(--success);
  }
  .activity-item.error .activity-marker {
    border-color: var(--danger-border);
  }
  .activity-item.error .activity-marker span {
    background: var(--danger);
  }
  .activity-item.waiting .activity-marker {
    border-color: var(--warning-border);
  }
  .activity-item.waiting .activity-marker span {
    background: var(--warning);
  }
  .activity-item.minor .activity-marker {
    border-color: var(--border-strong);
  }
  .activity-item.minor .activity-marker span {
    background: var(--text-dim);
  }
  .activity-copy {
    flex: 1;
    min-width: 0;
  }
  .activity-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  .activity-heading strong {
    min-width: 0;
  }
  .activity-copy strong {
    display: block;
    overflow: hidden;
    color: var(--text-bright);
    font-size: 12px;
    font-weight: 850;
    line-height: 1.35;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .activity-item.minor .activity-copy strong {
    color: var(--text-primary);
    font-size: 11px;
    font-weight: 780;
  }
  .activity-copy time {
    flex: 0 0 auto;
    color: var(--text-dim);
    font-size: 10px;
  }
  .activity-summary {
    margin: 3px 0 0;
    color: var(--text-secondary);
    font-size: 11px;
    line-height: 1.45;
  }
  .activity-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
    margin-top: 6px;
    color: var(--text-dim);
    font-size: 10px;
  }
  .activity-meta span:not(:first-child)::before {
    content: "";
    display: inline-block;
    width: 3px;
    height: 3px;
    margin: 0 7px 2px 0;
    border-radius: 50%;
    background: var(--border-strong);
  }
  .activity-expand {
    display: inline-flex;
    flex: 0 0 auto;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    margin: -4px -4px -4px 0;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-dim);
  }
  .activity-expand:hover {
    background: var(--surface-hover);
    color: var(--text-primary);
  }
  .activity-details {
    display: grid;
    gap: 9px;
    margin-top: 9px;
    padding: 9px 10px;
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    background: color-mix(in srgb, var(--surface-inset) 58%, transparent);
  }
  .activity-details > h3,
  .activity-details > p {
    margin: 0;
  }
  .activity-details > h3 {
    color: var(--text-primary);
    font-size: 10px;
    font-weight: 850;
  }
  .activity-details > p {
    color: var(--text-dim);
    font-size: 10px;
    line-height: 1.4;
  }
  .activity-details section {
    display: grid;
    gap: 5px;
  }
  .activity-details h4 {
    margin: 0;
    color: var(--text-secondary);
    font-size: 9px;
    font-weight: 850;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  .activity-details ul {
    display: grid;
    gap: 3px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .activity-details li {
    color: var(--text-secondary);
    font-size: 10px;
    line-height: 1.45;
    word-break: break-word;
  }
  .technical-body pre {
    margin: 6px 0 0;
    overflow: auto;
    color: var(--code-text);
    font:
      10px/1.45 ui-monospace,
      SFMono-Regular,
      monospace;
    white-space: pre-wrap;
  }
  .technical-drawer {
    margin: 12px 14px 14px;
    border: 1px solid var(--border-subtle);
    border-radius: 13px;
    background: var(--surface);
  }
  .technical-toggle {
    justify-content: space-between;
    width: 100%;
    min-height: 42px;
    border: 0;
    padding: 0 12px;
    background: transparent;
    color: var(--text-secondary);
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
    border-top: 1px solid var(--border-subtle);
  }
  .technical-body summary {
    color: var(--text-secondary);
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
    background: var(--code-block-bg);
  }
  .technical-events time,
  .technical-events span {
    color: var(--text-dim);
    font-size: 9px;
  }
  .technical-events span {
    margin-left: 8px;
  }
  .technical-events p {
    margin: 4px 0 0;
    color: var(--code-text);
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
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    padding: 8px 9px;
    background: var(--surface-inset);
    color: var(--text-bright);
    font-size: 11px;
  }
  .operator-form button {
    border: 0;
    background: var(--accent);
    color: var(--surface);
  }
  .rotated {
    transform: rotate(-180deg);
  }
</style>
