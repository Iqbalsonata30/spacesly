export type WorkspaceChatMessage = {
  id: string;
  role: "user" | "agent" | "system";
  text: string;
};

export type WorkspaceMode = "board" | "files" | "term";

export type UiState = {
  workspaceMode: WorkspaceMode;
  workspaceShellWorkdir: string;
  workspaceChatMessages: WorkspaceChatMessage[];
  doneVisibleLimit: number | "all";
  workspaceFilesRoot: string;
  workspaceFilesDirectory: string;
  workspaceFilesActivePath: string | null;
};

export const defaultUiState: UiState = {
  workspaceMode: "board",
  workspaceShellWorkdir: "",
  workspaceChatMessages: [],
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

export function loadUiState(
  storage: Storage | undefined,
  defaults: { chatMessages: WorkspaceChatMessage[] },
): UiState {
  if (!storage) {
    return {
      ...defaultUiState,
      workspaceChatMessages: defaults.chatMessages,
    };
  }

  try {
    const parsed = JSON.parse(storage.getItem("spacesly.ui.v1") ?? "{}") as Partial<UiState>;
    const messages = Array.isArray(parsed.workspaceChatMessages)
      ? parsed.workspaceChatMessages.filter(isWorkspaceChatMessage)
      : defaults.chatMessages;

    return {
      workspaceMode: normalizeWorkspaceMode(parsed.workspaceMode),
      workspaceShellWorkdir: typeof parsed.workspaceShellWorkdir === "string" ? parsed.workspaceShellWorkdir : "",
      workspaceChatMessages: messages.length > 0 ? messages : defaults.chatMessages,
      doneVisibleLimit: normalizeDoneVisibleLimit(parsed.doneVisibleLimit),
      workspaceFilesRoot: typeof parsed.workspaceFilesRoot === "string" ? parsed.workspaceFilesRoot : "",
      workspaceFilesDirectory: typeof parsed.workspaceFilesDirectory === "string" ? parsed.workspaceFilesDirectory : "",
      workspaceFilesActivePath: typeof parsed.workspaceFilesActivePath === "string" ? parsed.workspaceFilesActivePath : null,
    };
  } catch {
    return {
      ...defaultUiState,
      workspaceChatMessages: defaults.chatMessages,
    };
  }
}

export function serializeUiState(state: UiState): string {
  return JSON.stringify(state);
}
