import type {
  BoardProjection,
  CardProjection,
  ColumnIntent,
  ExecutionState,
  WorkspaceProjection,
} from "$lib/ipc";

export function withCompletionMetadata(
  card: CardProjection,
  columnIntent: ColumnIntent,
): CardProjection {
  if (columnIntent === "done")
    return { ...card, completedAt: card.completedAt ?? Date.now(), syncMissingAt: null };
  if (card.completedAt == null) return card;
  return { ...card, completedAt: null, syncMissingAt: null };
}

export function recoverInterruptedAgentRuns(
  workspace: WorkspaceProjection,
  interruptedCardIds: string[],
): WorkspaceProjection {
  if (interruptedCardIds.length === 0) return workspace;
  const interrupted = new Set(interruptedCardIds);
  const reason =
    "Agent execution was interrupted when Spacesly closed. Review the task, then retry when ready.";

  return {
    ...workspace,
    projects: workspace.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) => ({
          ...column,
          cards: column.cards.map((card) =>
            interrupted.has(card.id) && card.execution === "running"
              ? { ...card, execution: { blocked: { reason } } }
              : card,
          ),
        })),
      })),
    })),
  };
}

export function mergeSyncedWorkspace(
  projection: WorkspaceProjection,
  currentColumns: BoardProjection["columns"] | undefined,
  legacySeedCardId: string,
  retainMissingCardMs: number,
  locallyDeletedCardIds: string[] = [],
): WorkspaceProjection {
  const nowMs = Date.now();
  const deletedIds = new Set(locallyDeletedCardIds);
  const currentEntries = new Map<string, { card: CardProjection; intent: ColumnIntent }>();
  const incomingIds = new Set<string>();
  const incomingCards = new Map<string, CardProjection>();

  for (const column of currentColumns ?? []) {
    for (const card of column.cards) {
      if (deletedIds.has(card.id)) continue;
      currentEntries.set(card.id, { card, intent: column.intent });
    }
  }

  for (const column of projection.projects[0]?.boards[0]?.columns ?? []) {
    for (const card of column.cards) {
      if (deletedIds.has(card.id)) continue;
      incomingIds.add(card.id);
      incomingCards.set(card.id, card);
    }
  }

  const retainedCardsByIntent = new Map<ColumnIntent, CardProjection[]>();
  const retainedIds = new Set<string>();

  const retainCard = (card: CardProjection, intent: ColumnIntent) => {
    const cards = retainedCardsByIntent.get(intent) ?? [];
    cards.push(card);
    retainedCardsByIntent.set(intent, cards);
    retainedIds.add(card.id);
  };

  for (const { card, intent } of currentEntries.values()) {
    if (card.id === legacySeedCardId || deletedIds.has(card.id)) continue;

    if (card.source === "local") {
      retainCard({ ...card, syncMissingAt: null }, intent);
      continue;
    }

    if (intent !== "in_progress" && intent !== "done") continue;

    const missingAt = incomingIds.has(card.id) ? null : (card.syncMissingAt ?? nowMs);
    if (missingAt !== null && nowMs - missingAt > retainMissingCardMs) continue;

    retainCard(
      {
        ...card,
        jira_snapshot: incomingCards.get(card.id)?.jira_snapshot ?? card.jira_snapshot ?? null,
        syncMissingAt: missingAt,
      },
      intent,
    );
  }

  return {
    ...projection,
    projects: projection.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) => {
          const retainedForColumn = retainedCardsByIntent.get(column.intent) ?? [];
          return {
            ...column,
            cards: [
              ...column.cards
                .filter((card) => !retainedIds.has(card.id) && !deletedIds.has(card.id))
                .map((card) => {
                  const current = currentEntries.get(card.id)?.card;
                  return {
                    ...card,
                    completedAt: current?.completedAt ?? card.completedAt ?? null,
                    syncMissingAt: null,
                  };
                }),
              ...retainedForColumn,
            ],
          };
        }),
      })),
    })),
  };
}

export function descriptionParts(description: string): Array<{ text: string; url?: string }> {
  const urlPattern = /https?:\/\/[^\s<>"]+/g;
  const parts: Array<{ text: string; url?: string }> = [];
  let lastIndex = 0;

  for (const match of description.matchAll(urlPattern)) {
    const index = match.index ?? 0;
    if (index > lastIndex) {
      parts.push({ text: description.slice(lastIndex, index) });
    }

    const url = match[0].replace(/[),.;]+$/, "");
    const trailing = match[0].slice(url.length);
    parts.push({ text: url, url });
    if (trailing) parts.push({ text: trailing });
    lastIndex = index + match[0].length;
  }

  if (lastIndex < description.length) {
    parts.push({ text: description.slice(lastIndex) });
  }

  return parts.length > 0 ? parts : [{ text: description }];
}

export function executionDetail(execution: ExecutionState): string {
  if (typeof execution === "string") return execution.replace("_", " ");
  if ("blocked" in execution) return execution.blocked.reason;
  return execution.completed.summary;
}

export function isBlocked(execution: ExecutionState): boolean {
  return typeof execution === "object" && "blocked" in execution;
}

export function canStartAgent(card: CardProjection, running: boolean): boolean {
  return !running && card.execution !== "running";
}

export function agentActionLabel(
  card: CardProjection,
  running: boolean,
  hasOperatorNotes: boolean,
): string {
  if (running || card.execution === "running") return "Running";
  if (isBlocked(card.execution)) return hasOperatorNotes ? "↻ Continue Agent" : "↻ Retry Agent";
  return "▷ Start";
}
