export type WorkspaceChatMessage = {
  id: string;
  role: "user" | "agent" | "system";
  text: string;
};

export type WorkspaceChatActivity = {
  id: string;
  at: number;
  kind: "message" | "tool" | "action" | "summary";
  label: string;
  text: string;
  cardId?: string | null;
};

export type WorkspaceChatSession = {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messages: WorkspaceChatMessage[];
  activities: WorkspaceChatActivity[];
  recentCardIds: string[];
  lastCardId: string | null;
  lastCreatedCardId: string | null;
};

export type WorkspaceMode = "board" | "files" | "term";

const defaultWorkspaceChatSession = createWorkspaceChatSession([]);

export type UiState = {
  workspaceMode: WorkspaceMode;
  workspaceShellWorkdir: string;
  workspaceChatMessages: WorkspaceChatMessage[];
  workspaceChatSession: WorkspaceChatSession;
  workspaceChatSessions: WorkspaceChatSession[];
  workspaceChatActiveSessionId: string | null;
  doneVisibleLimit: number | "all";
  workspaceFilesRoot: string;
  workspaceFilesDirectory: string;
  workspaceFilesActivePath: string | null;
};

export const defaultUiState: UiState = {
  workspaceMode: "board",
  workspaceShellWorkdir: "",
  workspaceChatMessages: [],
  workspaceChatSession: defaultWorkspaceChatSession,
  workspaceChatSessions: [defaultWorkspaceChatSession],
  workspaceChatActiveSessionId: null,
  doneVisibleLimit: 20,
  workspaceFilesRoot: "",
  workspaceFilesDirectory: "",
  workspaceFilesActivePath: null,
};

export function normalizeDoneVisibleLimit(value: unknown): number | "all" {
  if (value === "all") return "all";
  if (value === 10 || value === 20) return value;
  return 20;
}

export function normalizeWorkspaceMode(value: unknown): WorkspaceMode {
  if (value === "term" || value === "files") return value;
  return "board";
}

export function isWorkspaceChatMessage(value: unknown): value is WorkspaceChatMessage {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<WorkspaceChatMessage>;
  return typeof candidate.id === "string"
    && (candidate.role === "user" || candidate.role === "agent" || candidate.role === "system")
    && typeof candidate.text === "string";
}

export function isWorkspaceChatActivity(value: unknown): value is WorkspaceChatActivity {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<WorkspaceChatActivity>;
  return typeof candidate.id === "string"
    && typeof candidate.at === "number"
    && (candidate.kind === "message" || candidate.kind === "tool" || candidate.kind === "action" || candidate.kind === "summary")
    && typeof candidate.label === "string"
    && typeof candidate.text === "string";
}

export function isWorkspaceChatSession(value: unknown): value is WorkspaceChatSession {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<WorkspaceChatSession>;
  return typeof candidate.id === "string"
    && typeof candidate.title === "string"
    && typeof candidate.createdAt === "number"
    && typeof candidate.updatedAt === "number"
    && Array.isArray(candidate.messages)
    && candidate.messages.every(isWorkspaceChatMessage)
    && Array.isArray(candidate.activities)
    && candidate.activities.every(isWorkspaceChatActivity)
    && Array.isArray(candidate.recentCardIds)
    && candidate.recentCardIds.every((entry) => typeof entry === "string")
    && (candidate.lastCardId === null || typeof candidate.lastCardId === "string")
    && (candidate.lastCreatedCardId === null || typeof candidate.lastCreatedCardId === "string");
}

export function createWorkspaceChatSession(messages: WorkspaceChatMessage[], title = "Workspace Chat"): WorkspaceChatSession {
  const now = Date.now();
  return {
    id: `chat-session-${now.toString(36)}`,
    title,
    createdAt: now,
    updatedAt: now,
    messages,
    activities: [],
    recentCardIds: [],
    lastCardId: null,
    lastCreatedCardId: null,
  };
}

export function loadUiState(
  storage: Storage | undefined,
  defaults: { chatMessages: WorkspaceChatMessage[] },
): UiState {
  if (!storage) {
    const session = createWorkspaceChatSession(defaults.chatMessages);
    return {
      ...defaultUiState,
      workspaceChatMessages: defaults.chatMessages,
      workspaceChatSession: session,
      workspaceChatSessions: [session],
      workspaceChatActiveSessionId: session.id,
    };
  }

  try {
    const parsed = JSON.parse(storage.getItem("spacesly.ui.v1") ?? "{}");
    const messages = Array.isArray(parsed.workspaceChatMessages)
      ? parsed.workspaceChatMessages.filter(isWorkspaceChatMessage)
      : defaults.chatMessages;
    const fallbackSession = createWorkspaceChatSession(messages.length > 0 ? messages : defaults.chatMessages);
    const legacySession = isWorkspaceChatSession(parsed.workspaceChatSession) ? parsed.workspaceChatSession : null;
    const sessionEntries = Array.isArray(parsed.workspaceChatSessions)
      ? parsed.workspaceChatSessions.filter(isWorkspaceChatSession)
      : [];
    const sessions = dedupeChatSessions(
      sessionEntries.length > 0
        ? sessionEntries
        : legacySession
          ? [legacySession]
          : [fallbackSession],
      messages,
      defaults.chatMessages,
    );
    const activeSessionId = typeof parsed.workspaceChatActiveSessionId === "string"
      ? parsed.workspaceChatActiveSessionId
      : sessions[0]?.id ?? fallbackSession.id;
    const activeSession = sessions.find((session) => session.id === activeSessionId) ?? sessions[0] ?? fallbackSession;
    const sessionMessages = activeSession.messages.length > 0 ? activeSession.messages : (messages.length > 0 ? messages : defaults.chatMessages);

    return {
      workspaceMode: normalizeWorkspaceMode(parsed.workspaceMode),
      workspaceShellWorkdir: typeof parsed.workspaceShellWorkdir === "string" ? parsed.workspaceShellWorkdir : "",
      workspaceChatMessages: sessionMessages,
      workspaceChatSession: {
        ...activeSession,
        messages: sessionMessages,
      },
      workspaceChatSessions: sessions.map((session) => session.id === activeSession.id ? { ...session, messages: sessionMessages } : session),
      workspaceChatActiveSessionId: activeSession.id,
      doneVisibleLimit: normalizeDoneVisibleLimit(parsed.doneVisibleLimit),
      workspaceFilesRoot: typeof parsed.workspaceFilesRoot === "string" ? parsed.workspaceFilesRoot : "",
      workspaceFilesDirectory: typeof parsed.workspaceFilesDirectory === "string" ? parsed.workspaceFilesDirectory : "",
      workspaceFilesActivePath: typeof parsed.workspaceFilesActivePath === "string" ? parsed.workspaceFilesActivePath : null,
    };
  } catch {
    const session = createWorkspaceChatSession(defaults.chatMessages);
    return {
      ...defaultUiState,
      workspaceChatMessages: defaults.chatMessages,
      workspaceChatSession: session,
      workspaceChatSessions: [session],
      workspaceChatActiveSessionId: session.id,
    };
  }
}

function dedupeChatSessions(
  sessions: WorkspaceChatSession[],
  messages: WorkspaceChatMessage[],
  fallbackMessages: WorkspaceChatMessage[],
): WorkspaceChatSession[] {
  const merged = new Map<string, WorkspaceChatSession>();
  for (const session of sessions) {
    merged.set(session.id, {
      ...session,
      messages: session.messages.length > 0 ? session.messages : (messages.length > 0 ? messages : fallbackMessages),
    });
  }

  return Array.from(merged.values()).sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 6);
}

export function serializeUiState(state: UiState): string {
  return JSON.stringify(state);
}
