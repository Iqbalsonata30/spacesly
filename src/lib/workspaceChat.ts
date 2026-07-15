import type { CardProjection, WorkspaceProjection } from "$lib/ipc";

export type WorkspaceChatAction =
  | { type: "create_task"; title: string; description?: string }
  | {
      type: "move_card";
      card_id?: string;
      ticket?: string;
      title?: string;
      target: "todo" | "queued" | "in_progress" | "done";
    }
  | { type: "start_agent"; card_id?: string; ticket?: string; title?: string }
  | { type: "select_card"; card_id?: string; ticket?: string; title?: string }
  | { type: "delete_card"; card_id?: string; ticket?: string; title?: string }
  | { type: "sync_jira" };

export type WorkspaceChatActionContext = {
  activeCardIds?: Set<string>;
  selectedCardId?: string | null;
  lastCardId?: string | null;
  recentCardIds?: string[];
  lastCreatedCardId?: string | null;
};

export function terminalContext(): string {
  return "PTY terminal is active. Ask about visible terminal output or paste relevant output into chat when needed.";
}

export function chatTargetLabel(target: "todo" | "queued" | "in_progress" | "done"): string {
  if (target === "todo") return "Todo";
  if (target === "queued") return "Queued";
  if (target === "in_progress") return "In Progress";
  return "Done";
}

export function workspaceAgentContext(
  activeBoard: WorkspaceProjection["projects"][number]["boards"][number] | null,
): string {
  return [terminalContext(), boardContext(activeBoard)].filter(Boolean).join("\n\n");
}

export function boardContext(
  activeBoard: WorkspaceProjection["projects"][number]["boards"][number] | null,
): string {
  if (!activeBoard) return "Board context: no active board loaded.";

  const cards = activeBoard.columns
    .flatMap((column) =>
      column.cards.slice(0, 20).map((card) => ({
        id: card.id,
        ticket: ticketLabel(card),
        title: card.title,
        column: columnTitle(column.name),
        intent: column.intent,
        execution: executionLabel(card.execution),
        labels: card.labels.slice(0, 4),
      })),
    )
    .slice(0, 80);

  return [
    `Board context: ${activeBoard.name}`,
    `Columns: ${activeBoard.columns.map((column) => `${column.intent}:${column.name}`).join(" | ")}`,
    "Cards JSON:",
    JSON.stringify(cards),
    "Allowed board actions: create_task, move_card, start_agent, select_card, delete_card, sync_jira.",
    "If you want Spacesly to mutate the board, append a final line starting with SPACESLY_ACTIONS: followed by a JSON array of actions. Use card_id when possible. Use target todo/queued/in_progress/done for move_card.",
  ].join("\n");
}

export function chatSessionContext(context: WorkspaceChatActionContext | null | undefined): string {
  if (!context) return "Chat session context: none.";

  const recent = context.recentCardIds?.filter(Boolean).slice(0, 6) ?? [];
  return [
    "Chat session context:",
    `Selected card: ${context.selectedCardId ?? "none"}`,
    `Last card: ${context.lastCardId ?? "none"}`,
    `Last created card: ${context.lastCreatedCardId ?? "none"}`,
    `Recent cards: ${recent.length > 0 ? recent.join(", ") : "none"}`,
  ].join("\n");
}

export function extractWorkspaceActions(response: string): WorkspaceChatAction[] {
  const match = response.match(/SPACESLY_ACTIONS:\s*(\[[\s\S]*\])\s*$/i);
  if (!match) return [];

  try {
    const parsed = JSON.parse(match[1]) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((entry) => normalizeWorkspaceAction(entry));
  } catch {
    return [];
  }
}

export function stripWorkspaceActions(response: string): string {
  return response.replace(/\n?SPACESLY_ACTIONS:\s*\[[\s\S]*\]\s*$/i, "").trim() || response.trim();
}

export function normalizeWorkspaceAction(value: unknown): WorkspaceChatAction[] {
  if (!value || typeof value !== "object") return [];
  const action = value as Record<string, unknown>;
  const type = typeof action.type === "string" ? action.type : "";

  if (type === "create_task" && typeof action.title === "string" && action.title.trim()) {
    return [
      {
        type,
        title: action.title.trim(),
        description: typeof action.description === "string" ? action.description : undefined,
      },
    ];
  }

  if (type === "move_card" && typeof action.target === "string" && isBoardTarget(action.target)) {
    return [
      {
        type,
        card_id: stringField(action.card_id),
        ticket: stringField(action.ticket),
        title: stringField(action.title),
        target: action.target,
      },
    ];
  }

  if (type === "start_agent" || type === "select_card") {
    return [
      {
        type,
        card_id: stringField(action.card_id),
        ticket: stringField(action.ticket),
        title: stringField(action.title),
      },
    ];
  }

  if (type === "delete_card") {
    return [
      {
        type,
        card_id: stringField(action.card_id),
        ticket: stringField(action.ticket),
        title: stringField(action.title),
      },
    ];
  }

  if (type === "sync_jira") return [{ type }];
  return [];
}

export function fastWorkspaceChatActions(
  message: string,
  context?: WorkspaceChatActionContext,
): WorkspaceChatAction[] {
  const text = message.trim();
  const lower = text.toLowerCase();

  if (/^(sync|refresh)\s+jira\b/.test(lower)) return [{ type: "sync_jira" }];

  const createMatch = text.match(/^(?:create|add)(?:\s+a)?\s+task\s*:?\s+(.+)$/i);
  if (createMatch?.[1]) return [{ type: "create_task", title: createMatch[1].trim() }];

  const queueMatch = text.match(/^queue\s+(.+)$/i);
  if (queueMatch?.[1])
    return [{ type: "move_card", ...parseCardReference(queueMatch[1], context), target: "queued" }];

  const moveMatch = text.match(/^move\s+(.+?)\s+to\s+(todo|queue|queued|in[\s_-]?progress|done)$/i);
  if (moveMatch?.[1] && moveMatch[2]) {
    return [
      {
        type: "move_card",
        ...parseCardReference(moveMatch[1], context),
        target: normalizeBoardTarget(moveMatch[2]),
      },
    ];
  }

  const startMatch = text.match(
    /^(?:start|run|continue|execute)(?:\s+agent)?(?:\s+(?:on|for|with))?\s+(.+)$/i,
  );
  if (startMatch?.[1])
    return [{ type: "start_agent", ...parseCardReference(startMatch[1], context) }];

  if (/^(?:start|run|continue|execute)(?:\s+it|\s+that|\s+this|\s+again)?$/i.test(text)) {
    return [{ type: "start_agent", ...parseCardReference("it", context) }];
  }

  const openMatch = text.match(/^(?:open|show|select)\s+(.+)$/i);
  if (openMatch?.[1])
    return [{ type: "select_card", ...parseCardReference(openMatch[1], context) }];

  const deleteMatch = text.match(/^(?:delete|remove)\s+(.+)$/i);
  if (deleteMatch?.[1])
    return [{ type: "delete_card", ...parseCardReference(deleteMatch[1], context) }];

  return [];
}

export function parseCardReference(
  value: string,
  context?: WorkspaceChatActionContext,
): { card_id?: string; ticket?: string; title?: string } {
  const text = value.trim().replace(/^task\s+/i, "");
  const contextualCard = resolveContextCardId(text, context);
  if (contextualCard) return { card_id: contextualCard };
  if (/^[A-Z][A-Z0-9]+-\d+$/i.test(text)) return { ticket: text.toUpperCase() };
  return { title: text };
}

export function normalizeBoardTarget(value: string): "todo" | "queued" | "in_progress" | "done" {
  const normalized = value.toLowerCase().replace(/[\s-]+/g, "_");
  if (normalized === "in_progress") return "in_progress";
  if (normalized === "queue") return "queued";
  return normalized as "todo" | "queued" | "done";
}

function resolveContextCardId(value: string, context?: WorkspaceChatActionContext): string | null {
  if (!context) return null;

  if (context.activeCardIds?.has(value)) return value;

  const recentCardIds =
    context.recentCardIds?.filter((cardId) => context.activeCardIds?.has(cardId)) ?? [];
  if (/^(it|that|this|them|again)$/i.test(value)) {
    if (context.lastCreatedCardId && context.activeCardIds?.has(context.lastCreatedCardId))
      return context.lastCreatedCardId;
    if (context.lastCardId && context.activeCardIds?.has(context.lastCardId))
      return context.lastCardId;
    if (recentCardIds[0]) return recentCardIds[0];
    if (context.selectedCardId && context.activeCardIds?.has(context.selectedCardId))
      return context.selectedCardId;
    return null;
  }

  return null;
}

function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function isBoardTarget(value: string): value is "todo" | "queued" | "in_progress" | "done" {
  return value === "todo" || value === "queued" || value === "in_progress" || value === "done";
}

export function ticketLabel(card: CardProjection): string {
  if (card.source === "local") return "spacesly/local";
  return card.source.jira.key;
}

export function columnTitle(name: string): string {
  if (name.toLowerCase() === "backlog") return "Todo";
  return name;
}

export function executionLabel(value: CardProjection["execution"]): string {
  if (typeof value === "string") return value.replace("_", " ");
  if ("blocked" in value) return "blocked";
  return "merged";
}
