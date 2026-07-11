<script lang="ts">
  import type { CardProjection } from "$lib/ipc";

  type DescriptionPart = { text: string; url?: string };

  let {
    card,
    selected,
    canStartAgent,
    actionLabel,
    executionLabel,
    ticketLabel,
    isBlocked,
    isQueued,
    showActions,
    showDelete,
    minHeight,
    onSelect,
    onQueue,
    onStartAgent,
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
    isQueued: boolean;
    showActions: boolean;
    showDelete: boolean;
    minHeight: number;
    onSelect: () => void;
    onQueue: () => void;
    onStartAgent: () => void;
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
</script>

<button
  class:selected={selected}
  class="task-card"
  style={`--card-min-height: ${minHeight}px;`}
  draggable="true"
  type="button"
  onclick={onSelect}
  ondragstart={onDragStart}
  ondragend={onDragEnd}
>
  <div class="task-status">
    <span></span>
    <strong>{executionLabel}</strong>
  </div>
  <h3>{card.title}</h3>
  <p>
    {#each description as part}
      {#if part.url}
        <a href={part.url} target="_blank" rel="noreferrer" onclick={(event) => event.stopPropagation()}>{part.text}</a>
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
      {#each card.labels.slice(0, 4) as label}
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
        }}
      >{isBlocked ? "Blocked" : isQueued ? "Queued" : "Queue"}</span>
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
        }}
      >{actionLabel}</span>
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
          }}
        >Remove</span>
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
    border: 1px solid #27252f;
    border-radius: 8px;
    padding: 16px;
    background: #1a1920;
    cursor: grab;
    color: inherit;
    font: inherit;
    text-align: left;
    user-select: none;
    transition: border-color 160ms ease, box-shadow 160ms ease, background 160ms ease, transform 160ms ease;
  }

  .task-card:hover {
    border-color: #34313d;
    background: #1d1b23;
  }

  .task-card:active {
    cursor: grabbing;
  }

  .task-card:focus-visible {
    outline: 2px solid #91aec2;
    outline-offset: 2px;
  }

  .task-card.selected {
    border-color: #91aec2;
    box-shadow: 0 0 0 2px rgba(145, 174, 194, 0.34);
  }

  .task-status {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
    color: #95b082;
    font-size: 13px;
    font-weight: 800;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .task-status span {
    width: 9px;
    height: 9px;
    border-radius: 999px;
    background: #8daa7a;
  }

  h3 {
    margin: 0;
    color: #f1edf5;
    font-size: clamp(17px, 1.35vw, 21px);
    line-height: 1.2;
    overflow-wrap: anywhere;
  }

  p {
    display: -webkit-box;
    margin: 12px 0 0;
    overflow: hidden;
    color: #aaa1c3;
    font-size: 16px;
    line-height: 1.45;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
  }

  p a {
    color: #b8d6e4;
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
    color: #b8d6e4;
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
    background: #24232d;
    color: #8f88a8;
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
    border: 1px solid #34313d;
    border-radius: 8px;
    background: #22212a;
    color: #77718a;
    font-weight: 800;
    white-space: nowrap;
    transition: border-color 140ms ease, background 140ms ease, color 140ms ease, opacity 140ms ease;
  }

  .actions .queue {
    cursor: pointer;
  }

  .actions .queue.active {
    border-color: rgba(149, 176, 130, 0.5);
    background: rgba(52, 73, 46, 0.42);
    color: #cce2bd;
    cursor: default;
  }

  .actions .queue.blocked {
    border-color: rgba(240, 176, 170, 0.32);
    background: rgba(91, 42, 42, 0.22);
    color: #f0b0aa;
    cursor: default;
  }

  .actions .start {
    cursor: pointer;
    border-color: #52606a;
    background: #252a31;
    color: #b8d6e4;
  }

  .actions .start.retry {
    border-color: rgba(240, 176, 170, 0.44);
    background: rgba(91, 42, 42, 0.34);
    color: #f0b0aa;
  }

  .actions .start[aria-disabled="true"] {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .actions .delete {
    border-color: rgba(240, 176, 170, 0.26);
    background: rgba(91, 42, 42, 0.18);
    color: #f0b0aa;
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
