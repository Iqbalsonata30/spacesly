import type {
  BoardProjection,
  CardProjection,
  ColumnIntent,
  ExecutionState,
  WorkspaceProjection,
} from "$lib/ipc";

export type AgentRunLog = {
  id: string;
  at: string;
  tone: "info" | "success" | "error";
  label: string;
  message: string;
};

export type AgentPhaseKey = "prepare" | "jira" | "model" | "execute" | "writeback" | "done";

export type AgentPhase = {
  key: AgentPhaseKey;
  label: string;
  state: "pending" | "active" | "done" | "blocked";
};

export function withCompletionMetadata(card: CardProjection, columnIntent: ColumnIntent): CardProjection {
  if (columnIntent === "done") return { ...card, completedAt: card.completedAt ?? Date.now(), syncMissingAt: null };
  if (card.completedAt == null) return card;
  return { ...card, completedAt: null, syncMissingAt: null };
}

export function mergeSyncedWorkspace(
  projection: WorkspaceProjection,
  currentColumns: BoardProjection["columns"] | undefined,
  legacySeedCardId: string,
  retainMissingCardMs: number,
): WorkspaceProjection {
  const nowMs = Date.now();
  const currentEntries = new Map<string, { card: CardProjection; intent: ColumnIntent }>();
  const incomingIds = new Set<string>();

  for (const column of currentColumns ?? []) {
    for (const card of column.cards) {
      currentEntries.set(card.id, { card, intent: column.intent });
    }
  }

  for (const column of projection.projects[0]?.boards[0]?.columns ?? []) {
    for (const card of column.cards) {
      incomingIds.add(card.id);
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
    if (card.id === legacySeedCardId) continue;

    if (card.source === "local") {
      retainCard({ ...card, syncMissingAt: null }, intent);
      continue;
    }

    if (intent !== "in_progress" && intent !== "done") continue;

    const missingAt = incomingIds.has(card.id) ? null : card.syncMissingAt ?? nowMs;
    if (missingAt !== null && nowMs - missingAt > retainMissingCardMs) continue;

    retainCard({ ...card, syncMissingAt: missingAt }, intent);
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
                .filter((card) => !retainedIds.has(card.id))
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

export function agentActivity(
  status: "idle" | "queued" | "running" | "blocked" | "completed",
  progress: number,
  log: AgentRunLog | null,
): { title: string; detail: string; next: string } {
  if (status === "blocked") {
    return {
      title: "Blocked",
      detail: log?.message ?? "Agent stopped because it needs attention.",
      next: "Review the blocker, then retry the Agent when ready.",
    };
  }

  if (status === "completed") {
    return {
      title: "Completed and verified",
      detail: log?.message ?? "Agent finished the task and stored the result.",
      next: "Review the result or open the card for details.",
    };
  }

  if (progress < 15) {
    return {
      title: "Preparing workspace task",
      detail: log?.message ?? "Opening execution session and reading card context.",
      next: "Move the card into execution state.",
    };
  }

  if (progress < 35) {
    return {
      title: "Syncing Jira execution state",
      detail: log?.message ?? "Assigning the issue and moving Jira to In Progress when available.",
      next: "Send the task context to the Agent runtime.",
    };
  }

  if (progress < 75) {
    return {
      title: "Agent is working",
      detail: log?.message ?? "The selected runtime is processing the task and preparing output.",
      next: "Wait for completion evidence or a blocker.",
    };
  }

  if (progress < 94) {
    return {
      title: "Recording result",
      detail: log?.message ?? "Spacesly is saving the Agent result and preparing Jira write-back.",
      next: "Post summary/comment and transition Jira if configured.",
    };
  }

  return {
    title: "Finalizing",
    detail: log?.message ?? "Spacesly is finishing local and Jira state updates.",
    next: "Mark the run completed after final verification.",
  };
}

export function agentPhaseTimeline(status: "idle" | "queued" | "running" | "blocked" | "completed", progress: number): AgentPhase[] {
  const phases: Array<Omit<AgentPhase, "state"> & { threshold: number }> = [
    { key: "prepare", label: "Prepare", threshold: 5 },
    { key: "jira", label: "Jira", threshold: 25 },
    { key: "model", label: "Runtime", threshold: 35 },
    { key: "execute", label: "Execute", threshold: 55 },
    { key: "writeback", label: "Write-back", threshold: 82 },
    { key: "done", label: "Done", threshold: 100 },
  ];

  return phases.map((phase, index) => {
    const next = phases[index + 1];
    const active = progress >= phase.threshold && (!next || progress < next.threshold);
    return {
      key: phase.key,
      label: phase.label,
      state: status === "blocked" && active
        ? "blocked"
        : progress >= phase.threshold && (!next || progress >= next.threshold)
          ? "done"
          : active
            ? "active"
            : "pending",
    };
  });
}

export function isBlocked(execution: ExecutionState): boolean {
  return typeof execution === "object" && "blocked" in execution;
}

export function canStartAgent(card: CardProjection, running: boolean): boolean {
  return !running && card.execution !== "running";
}

export function agentActionLabel(card: CardProjection, running: boolean, hasOperatorNotes: boolean): string {
  if (running || card.execution === "running") return "Running";
  if (isBlocked(card.execution)) return hasOperatorNotes ? "↻ Continue Agent" : "↻ Retry Agent";
  return "▷ Start";
}
