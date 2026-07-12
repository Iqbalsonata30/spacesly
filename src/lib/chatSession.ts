import type { CardProjection } from "$lib/ipc";

export function summarizeChatTitle(text: string): string {
  const cleaned = text.replace(/\s+/g, " ").trim();
  if (!cleaned) return "Workspace Chat";
  return cleaned.length > 42 ? `${cleaned.slice(0, 39).trim()}...` : cleaned;
}

export function isGenericChatTitle(title: string): boolean {
  return title === "Workspace Chat" || /^Chat \d+$/.test(title);
}

export function chatTitleForCard(card: CardProjection): string {
  const prefix = card.source === "local" ? "Task" : card.source.jira.key;
  return `${prefix}: ${summarizeChatTitle(card.title)}`;
}
