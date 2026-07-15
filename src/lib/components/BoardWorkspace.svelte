<script lang="ts">
  import TaskCard from "$lib/components/TaskCard.svelte";
  import type { BoardProjection, CardProjection } from "$lib/ipc";
  import { columnTitle } from "$lib/workspaceChat";

  type BoardDisplayColumn = BoardProjection["columns"][number] & {
    totalCardCount: number;
    hiddenLaneCardCount: number;
    hiddenDoneCardCount: number;
  };

  type Props = {
    displayColumns: BoardDisplayColumn[];
    selectedCardId: string | null;
    draggedCardId: string | null;
    runningWorkerCardIds: Record<string, true>;
    cardMinHeight: number;
    doneVisibleLimit: number | "all";
    hasAgentConsoleSession: boolean;
    agentConsoleOpen: boolean;
    agentRunStatus: string;
    agentRunProgress: number;
    onResizeLane: (event: PointerEvent) => void;
    onResizeCard: (event: PointerEvent) => void;
    onOpenAgentConsole: () => void;
    onDropCard: (cardId: string, columnId: string) => void;
    onSelectCard: (card: CardProjection) => void;
    onQueueCard: (cardId: string) => void;
    onStartAgent: (cardId: string) => void;
    onMarkDone: (cardId: string) => void;
    onDeleteCard: (cardId: string) => void;
    onDragStartCard: (cardId: string) => void;
    onDragEndCard: () => void;
    onSetDoneVisibleLimit: (limit: number | "all") => void;
    onShowMoreLaneCards: (column: BoardDisplayColumn) => void;
    onShowAllLaneCards: (column: BoardDisplayColumn) => void;
    onOpenNewTask: () => void;
    canStartAgent: (card: CardProjection, running: boolean) => boolean;
    agentActionLabel: (card: CardProjection, running: boolean, hasNote: boolean) => string;
    executionLabel: (execution: CardProjection["execution"]) => string;
    ticketLabel: (card: CardProjection) => string;
    isBlocked: (execution: CardProjection["execution"]) => boolean;
    operatorNotesForCard: (cardId: string) => string | null;
  };

  let {
    displayColumns,
    selectedCardId,
    draggedCardId,
    runningWorkerCardIds,
    cardMinHeight,
    doneVisibleLimit,
    hasAgentConsoleSession,
    agentConsoleOpen,
    agentRunStatus,
    agentRunProgress,
    onResizeLane,
    onResizeCard,
    onOpenAgentConsole,
    onDropCard,
    onSelectCard,
    onQueueCard,
    onStartAgent,
    onMarkDone,
    onDeleteCard,
    onDragStartCard,
    onDragEndCard,
    onSetDoneVisibleLimit,
    onShowMoreLaneCards,
    onShowAllLaneCards,
    onOpenNewTask,
    canStartAgent,
    agentActionLabel,
    executionLabel,
    ticketLabel,
    isBlocked,
    operatorNotesForCard,
  }: Props = $props();
</script>

<div class="board-panel">
    <div class="resize-toolbar" aria-label="Board sizing controls">
      <span>Board visibility</span>
      <div title="Drag horizontally to resize columns">
        <strong>Columns</strong>
        <span class="toolbar-resize-handle">
          <span
            class="drag-handle horizontal"
            role="separator"
            aria-orientation="horizontal"
            onpointerdown={onResizeLane}
          ></span>
        </span>
      </div>
      <div title="Drag vertically to resize cards">
        <strong>Cards</strong>
        <span class="toolbar-resize-handle">
          <span
            class="drag-handle vertical"
            role="separator"
            aria-orientation="vertical"
            onpointerdown={onResizeCard}
          ></span>
        </span>
      </div>
      {#if hasAgentConsoleSession}
        <button class="agent-console-launch" class:active={agentConsoleOpen} type="button" onclick={onOpenAgentConsole}>
          <span class={`agent-console-dot ${agentRunStatus}`}></span>
          <span>Open Agent Console</span>
          <strong>{agentRunProgress}%</strong>
        </button>
      {/if}
    </div>

    <div class="board">
      {#each displayColumns as column (column.id)}
        <section
          class="lane"
          class:drop-target={draggedCardId !== null}
          data-intent={column.intent}
          role="list"
          ondragover={(event) => event.preventDefault()}
          ondrop={(event) => {
            event.preventDefault();
            const cardId = event.dataTransfer?.getData("text/plain");
            if (cardId) onDropCard(cardId, column.id);
          }}
        >
          <header class="lane-header">
            <div>
              <span class="caret">⌄</span>
              <h2>{column.name}</h2>
              <span class="count">{column.intent === "done" && column.hiddenDoneCardCount > 0 ? `${column.cards.length}/${column.totalCardCount}` : column.totalCardCount}</span>
            </div>
            {#if column.intent === "done"}
              <div class="done-controls" aria-label="Done card visibility">
                <button class:active={doneVisibleLimit === 10} type="button" onclick={() => onSetDoneVisibleLimit(10)}>10</button>
                <button class:active={doneVisibleLimit === 20} type="button" onclick={() => onSetDoneVisibleLimit(20)}>20</button>
                <button class:active={doneVisibleLimit === "all"} type="button" onclick={() => onSetDoneVisibleLimit("all")}>All</button>
              </div>
            {/if}
          </header>

          <div class="lane-cards">
            {#each column.cards as card (card.id)}
              <TaskCard
                card={card}
                selected={selectedCardId === card.id}
                canStartAgent={canStartAgent(card, Boolean(runningWorkerCardIds[card.id]))}
                actionLabel={agentActionLabel(card, Boolean(runningWorkerCardIds[card.id]), Boolean(operatorNotesForCard(card.id)))}
                executionLabel={executionLabel(card.execution)}
                ticketLabel={ticketLabel(card)}
                isBlocked={isBlocked(card.execution)}
                isQueued={column.intent === "queued"}
                showActions={column.intent === "backlog" || column.intent === "queued" || isBlocked(card.execution)}
                showDelete={column.intent === "backlog"}
                minHeight={cardMinHeight}
                onSelect={() => onSelectCard(card)}
                onQueue={() => onQueueCard(card.id)}
                onStartAgent={() => onStartAgent(card.id)}
                onMarkDone={() => onMarkDone(card.id)}
                onDelete={() => onDeleteCard(card.id)}
                onDragStart={() => onDragStartCard(card.id)}
                onDragEnd={onDragEndCard}
              />
            {:else}
              <div class="empty-card">Drop completed work here</div>
            {/each}

            {#if column.hiddenDoneCardCount > 0}
              <div class="done-hidden-card">
                Showing latest {column.cards.length}. {column.hiddenDoneCardCount} older completed card{column.hiddenDoneCardCount === 1 ? "" : "s"} kept in history.
                <button type="button" onclick={() => onSetDoneVisibleLimit("all")}>Show all</button>
              </div>
            {/if}

            {#if column.hiddenLaneCardCount > 0}
              <div class="lane-pagination-card">
                <span>Showing {column.cards.length} of {column.totalCardCount} cards.</span>
                <div>
                  <button type="button" onclick={() => onShowMoreLaneCards(column)}>Show {Math.min(column.hiddenLaneCardCount, 40)} more</button>
                  <button type="button" onclick={() => onShowAllLaneCards(column)}>Show all</button>
                </div>
              </div>
            {/if}
          </div>

          {#if column.intent === "backlog"}
            <footer class="lane-footer">
              <button type="button" onclick={onOpenNewTask}>＋ New task</button>
            </footer>
          {/if}
        </section>
      {/each}
    </div>
</div>
