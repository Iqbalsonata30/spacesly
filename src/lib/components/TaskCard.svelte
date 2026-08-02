<script lang="ts">
  import type { CardProjection } from "$lib/ipc";
  import type { AgentTaskCardProjection } from "$lib/agentRun";

  type DescriptionPart = { text: string; url?: string };

  let {
    card,
    selected,
    canStartAgent,
    actionLabel,
    executionLabel,
    ticketLabel,
    isBlocked,
    agentTask,
    isQueued,
    showActions,
    showDelete,
    minHeight,
    onSelect,
    onQueue,
    onStartAgent,
    onMarkDone,
    onDelete,
    onDragStart,
    onDragEnd,
  } = $props<{
    card: CardProjection;
    selected: boolean;
    canStartAgent: boolean;
    actionLabel: string;
    executionLabel: string;
    ticketLabel: string;
    isBlocked: boolean;
    agentTask: AgentTaskCardProjection | null;
    isQueued: boolean;
    showActions: boolean;
    showDelete: boolean;
    minHeight: number;
    onSelect: () => void;
    onQueue: () => void;
    onStartAgent: () => void;
    onMarkDone: () => void;
    onDelete: () => void;
    onDragStart: (event: DragEvent) => void;
    onDragEnd: () => void;
  }>();

  let cachedDescriptionText = "";
  let cachedDescriptionParts: DescriptionPart[] = [];
  let description = $derived(descriptionParts(card.description));

  function descriptionParts(value: string): DescriptionPart[] {
    if (cachedDescriptionText === value) return cachedDescriptionParts;

    const parts = value
      .split(/(https?:\/\/\S+)/g)
      .filter(Boolean)
      .map((part) => ({ text: part, url: part.startsWith("http") ? part : undefined }));
    cachedDescriptionText = value;
    cachedDescriptionParts = parts;
    return parts;
  }

  function startAgent(event: MouseEvent | KeyboardEvent): void {
    event.stopPropagation();
    if (canStartAgent) onStartAgent();
  }

  function queueTask(event: MouseEvent | KeyboardEvent): void {
    event.stopPropagation();
    if (!isBlocked && !isQueued) onQueue();
  }

  function deleteCard(event: MouseEvent | KeyboardEvent): void {
    event.stopPropagation();
    onDelete();
  }

  function markDone(event: MouseEvent | KeyboardEvent): void {
    event.stopPropagation();
    if (isBlocked) onMarkDone();
  }
</script>

<button
  class:selected
  class="task-card"
  style={`--card-min-height: ${minHeight}px;`}
  draggable="true"
  type="button"
  onclick={onSelect}
  ondragstart={onDragStart}
  ondragend={onDragEnd}
>
  <div class="task-status">
    <strong>{agentTask?.status ?? executionLabel}</strong>
    {#if agentTask}<em>{agentTask.progress}%</em>{/if}
  </div>
  {#if agentTask}
    <div class="task-progress" aria-label={`Agent progress ${agentTask.progress}%`}>
      <span style={`width: ${agentTask.progress}%`}></span>
    </div>
  {/if}
  <h3>{card.title}</h3>
  <p>
    {#each description as part, index (`${index}:${part.url || part.text}`)}
      {#if part.url}
        <a
          href={part.url}
          target="_blank"
          rel="noreferrer"
          onclick={(event) => event.stopPropagation()}>{part.text}</a
        >
      {:else}
        {part.text}
      {/if}
    {/each}
  </p>
  <footer>
    <span class="ticket-link-label">{ticketLabel}</span>
  </footer>
  {#if card.labels.length > 0}
    <div class="labels">
      {#each card.labels.slice(0, 4) as label, index (`${index}:${label}`)}
        <span>{label}</span>
      {/each}
    </div>
  {/if}
  {#if showActions}
    <div class="actions">
      <span
        class="queue"
        class:active={isQueued}
        class:blocked={isBlocked}
        aria-disabled={isBlocked || isQueued}
        role="button"
        tabindex="0"
        onclick={queueTask}
        onkeydown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            queueTask(event);
          }
        }}>{isBlocked ? "Blocked" : isQueued ? "Queued" : "Queue"}</span
      >
      <span
        class="start"
        class:retry={isBlocked}
        aria-disabled={!canStartAgent}
        role="button"
        tabindex="0"
        onclick={startAgent}
        onkeydown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            startAgent(event);
          }
        }}>{actionLabel}</span
      >
      {#if isBlocked}
        <span
          class="manual-done"
          role="button"
          tabindex="0"
          onclick={markDone}
          onkeydown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              markDone(event);
            }
          }}>Mark Done</span
        >
      {/if}
      {#if showDelete}
        <span
          class="delete"
          role="button"
          tabindex="0"
          aria-label={`Remove ${card.title}`}
          onclick={deleteCard}
          onkeydown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              deleteCard(event);
            }
          }}>Remove</span
        >
      {/if}
    </div>
  {/if}
</button>

<style>
  .task-card {
    display: block;
    width: 100%;
    min-height: var(--card-min-height);
    contain: layout paint;
    content-visibility: auto;
    contain-intrinsic-size: 220px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    padding: 16px;
    background: var(--surface-raised);
    cursor: grab;
    color: inherit;
    font: inherit;
    text-align: left;
    user-select: none;
    transition:
      border-color 60ms ease,
      background 60ms ease,
      transform 60ms ease;
  }

  .task-card:hover {
    border-color: var(--border-strong);
    background: var(--surface-hover);
  }

  .task-card:active {
    cursor: grabbing;
  }

  .task-card:focus-visible {
    outline: 2px solid var(--focus-border);
    outline-offset: 2px;
  }

  .task-card.selected {
    border-color: var(--selection-border);
    box-shadow: 0 0 0 2px var(--focus-ring);
  }

  .task-status {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 800;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .task-status em {
    margin-left: auto;
    color: var(--text-secondary);
    font-style: normal;
    letter-spacing: 0;
  }

  .task-progress {
    height: 3px;
    margin: -2px 0 10px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--progress-track);
  }

  .task-progress span {
    display: block;
    height: 100%;
    background: var(--progress-fill);
  }

  h3 {
    margin: 0;
    color: var(--text-bright);
    font-size: clamp(17px, 1.35vw, 21px);
    line-height: 1.2;
    overflow-wrap: anywhere;
  }

  p {
    display: -webkit-box;
    margin: 12px 0 0;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: 16px;
    line-height: 1.45;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
  }

  p a {
    color: var(--text-link);
    text-decoration: none;
  }

  p a:hover {
    text-decoration: underline;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 8px;
    margin-top: 12px;
  }

  .ticket-link-label {
    color: var(--text-link);
    font-family: var(--font-mono);
    font-size: 13px;
  }

  .labels {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
  }

  .labels span {
    border-radius: 999px;
    padding: 3px 8px;
    background: var(--surface-selected);
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 800;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 14px;
  }

  .actions span {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    height: 36px;
    padding: 0 14px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--surface-hover);
    color: var(--text-dim);
    font-weight: 800;
    white-space: nowrap;
    transition:
      border-color 60ms ease,
      background 60ms ease,
      color 60ms ease,
      opacity 60ms ease;
  }

  .actions .queue {
    cursor: pointer;
  }

  .actions .queue.active {
    border-color: var(--success-border);
    background: var(--success-bg);
    color: var(--success);
    cursor: default;
  }

  .actions .queue.blocked {
    border-color: var(--danger-border);
    background: var(--danger-bg);
    color: var(--danger);
    cursor: default;
  }

  .actions .start {
    cursor: pointer;
    border-color: var(--border-interactive);
    background: var(--selection-bg);
    color: var(--text-link);
  }

  .actions .start.retry {
    border-color: var(--danger-border);
    background: var(--danger-bg);
    color: var(--danger);
  }

  .actions .start[aria-disabled="true"] {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .actions .manual-done {
    border-color: var(--success-border);
    background: var(--success-bg);
    color: var(--success);
    cursor: pointer;
  }

  .actions .delete {
    border-color: var(--danger-border);
    background: color-mix(in srgb, var(--danger-bg) 72%, transparent);
    color: var(--danger);
    cursor: pointer;
  }

  @media (max-width: 520px) {
    .task-card {
      padding: 14px;
    }

    .actions span {
      flex: 1 1 calc(50% - 4px);
      padding: 0 10px;
      font-size: 12px;
    }

    .actions .delete {
      flex-basis: 100%;
    }
  }
</style>
