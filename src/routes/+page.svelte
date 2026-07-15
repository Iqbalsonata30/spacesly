<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import type { Terminal as XtermTerminal } from "@xterm/xterm";
  import type { FitAddon as XtermFitAddon } from "@xterm/addon-fit";
  import BoardWorkspace from "$lib/components/BoardWorkspace.svelte";
  import NewTaskPopover from "$lib/components/NewTaskPopover.svelte";
  import NotificationStack from "$lib/components/NotificationStack.svelte";
  import SegmentedControl from "$lib/components/SegmentedControl.svelte";
  import TerminalWorkspace from "$lib/components/TerminalWorkspace.svelte";
  import { formatEditorText, validateEditorSyntax } from "$lib/editorFormatting";
  import { chatTitleForCard, isGenericChatTitle, summarizeChatTitle } from "$lib/chatSession";
  import { displayPath, fileName, normalizeAbsolutePath } from "$lib/filesFeature";
  import { collectAncestorPaths, pruneExpandedFolderTree } from "$lib/fileBrowser";
  import {
    agentActivity,
    agentActionLabel,
    agentPhaseTimeline,
    canStartAgent,
    type AgentPhase,
    descriptionParts,
    executionDetail,
    isBlocked,
    mergeSyncedWorkspace,
    withCompletionMetadata,
  } from "$lib/boardWorkflow";
  import {
    agentSessionReplay,
    appendAgentSessionEvent,
    createAgentRunSession,
    createAgentSessionEvent,
    type AgentRunGitSnapshot,
    type AgentRunLog,
    type AgentRunSession,
    type AgentRunStatus,
    type AgentSessionEvent,
    type AgentTerminalLine,
  } from "$lib/agentRun";
  import { capList, capText } from "$lib/boundedBuffers";
  import {
    createWorkspaceChatSession,
    loadUiState,
    serializeUiState,
    type UiState,
    type WorkspaceChatActivity,
    type WorkspaceChatMessage,
    type WorkspaceChatSession,
    type WorkspaceMode,
  } from "$lib/uiState";
  import {
    chatTargetLabel,
    chatSessionContext,
    executionLabel,
    extractWorkspaceActions,
    fastWorkspaceChatActions,
    ticketLabel,
    workspaceAgentContext,
    stripWorkspaceActions,
    type WorkspaceChatAction,
    type WorkspaceChatActionContext,
  } from "$lib/workspaceChat";
  import { createSourceControlStore } from "$lib/sourceControlStore.svelte";
  import "./page.css";
  import {
    addJiraComment,
    assignJiraIssue,
    chatAiWorker,
    executeAiWorkerTask,
    getJiraBoards,
    getPathGitInfo,
    getWorkspaceGitInfo,
    getWorkspace,
    listDirectory,
    loadAppSecrets,
    openPtyTerminal,
    closePtyTerminal,
    readFile,
    resizePtyTerminal,
    saveAppSecrets,
    setWorkspaceRoot,
    syncJiraWorkspace,
    testAiWorker,
    testMcpServerConnection,
    transitionJiraIssue,
    writeFile,
    writePtyTerminal,
    workspaceRootPath,
    IpcPolicyError,
    type AiWorkerConfig,
    type AiWorkerStatus,
    type BoardProjection,
    type CardProjection,
    type CardSource,
    type ColumnIntent,
    type ExecutionState,
    type FileEntry,
    type GitStatus,
    type GitWorkspaceInfo,
    type AiWorkerTaskResult,
    type JiraMcpConfig,
    type JiraBoard,
    type WorkspaceProjection,
    testJiraMcpConnection,
  } from "$lib/ipc";
  import {
    createMcpServer,
    loadLegacySettingsSecrets,
    loadSettings,
    saveSettings,
    hasAnySecret,
    mergeAppSecrets,
    secretsFromSettings,
    settingsWithoutSecrets,
    type AppSettings,
    type AppSecrets,
  } from "$lib/settings";
  import { aiProviders, defaultModelForProvider, modelById, providerById } from "$lib/aiModels";
  import { opencodeModelOptions } from "$lib/opencodeModels";
  import { cachedWorkspaceSizeBytes, loadCachedWorkspace, saveCachedWorkspace } from "$lib/workspaceCache";

  type CodeEditorHandle = {
    getValue: () => string;
    setValue: (value: string) => void;
    markSaved: (value?: string) => void;
    focus: () => void;
  };

  const initialSettings = loadSettings();
  const initialAppSecrets = loadLegacySettingsSecrets();
  const AGENT_STATUS_KEY = "spacesly.agent.status.v1";
  const MAX_AGENT_LOGS = 120;
  const MAX_AGENT_TERMINAL_LINES = 80;
  const MAX_AGENT_SESSION_EVENTS = 120;
  const MAX_AGENT_SESSION_REPLAY_CHARS = 12_000;
  const MAX_WORKSPACE_CHAT_MESSAGES = 80;
  const MAX_CHAT_SESSIONS = 6;
  const MAX_WORKSPACE_CHAT_ACTIVITIES = 120;
  const MAX_WORKSPACE_CHAT_RECENT_CARDS = 12;
  const MAX_AGENT_OUTPUT_CHARS = 32_000;
  const DEFAULT_DONE_VISIBLE_LIMIT = 20;
  const DEFAULT_LANE_VISIBLE_LIMIT = 40;
  const LANE_VISIBLE_INCREMENT = 40;
  const SYNC_RETAIN_MISSING_CARD_MS = 3 * 24 * 60 * 60 * 1_000;
  const LEGACY_SEED_CARD_ID = "local-list-current-directory";
  const UI_STATE_WRITE_DELAY_MS = 200;
  const NOTICE_AUTO_DISMISS_MS = 3_000;
  const ERROR_NOTICE_AUTO_DISMISS_MS = 5_000;
  const LAYOUT_PREFS_KEY = "spacesly.layout.v1";
  const UI_STATE_KEY = "spacesly.ui.v1";

  type LayoutPrefs = {
    laneWidth: number;
    cardMinHeight: number;
    fileSidebarWidth: number;
    terminalWidth: number;
    agentConsoleWidth: number;
    agentLogHeight: number;
    agentOutputHeight: number;
    agentApprovalHeight: number;
  };

  type LayoutResizeDrag = {
    key: keyof LayoutPrefs;
    min: number;
    max: number;
    invert: boolean;
    lastPosition: number;
    axis: "x" | "y";
    pointerId: number;
  };

  type OpenEditorFile = {
    path: string;
    name: string;
    content: string;
    savedContent: string;
    dirty: boolean;
    editor: CodeEditorHandle | null;
  };

  const defaultLayoutPrefs: LayoutPrefs = {
    laneWidth: 300,
    cardMinHeight: 220,
    fileSidebarWidth: 340,
    terminalWidth: 680,
    agentConsoleWidth: 420,
    agentLogHeight: 170,
    agentOutputHeight: 220,
    agentApprovalHeight: 180,
  };

  const defaultWorkspaceChatMessages: WorkspaceChatMessage[] = [
    {
      id: "chat-welcome",
      role: "system",
      text: "Agent Chat is command-first. Press Enter to send, Shift+Enter for multiline. Try: queue ABC-123, start agent on ABC-123, sync Jira, or ask what changed.",
    },
  ];
  const initialUiState = (() => {
    const state = loadUiState(typeof localStorage === "undefined" ? undefined : localStorage, {
      chatMessages: defaultWorkspaceChatMessages,
    });

    const activeSession = state.workspaceChatSession;
    return {
      ...state,
      workspaceChatMessages: state.workspaceChatMessages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
      workspaceChatSessions: state.workspaceChatSessions.map((session) => ({
        ...session,
        messages: session.messages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
        activities: session.activities.slice(-MAX_WORKSPACE_CHAT_ACTIVITIES),
        recentCardIds: session.recentCardIds.slice(0, MAX_WORKSPACE_CHAT_RECENT_CARDS),
      })),
      workspaceChatActiveSessionId: state.workspaceChatActiveSessionId ?? activeSession.id,
      workspaceChatSession: {
        ...activeSession,
        messages: activeSession.messages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
        activities: activeSession.activities.slice(-MAX_WORKSPACE_CHAT_ACTIVITIES),
        recentCardIds: activeSession.recentCardIds.slice(0, MAX_WORKSPACE_CHAT_RECENT_CARDS),
      },
    };
  })();

  type AgentConnectionState = {
    connected: boolean;
    testedAt: number;
    message: string;
  };

  type McpConnectionState = {
    status: "connected" | "disconnected";
    testedAt: number;
    message: string;
    toolCount: number;
  };

  type ChatSessionState = WorkspaceChatSession;

  type BoardDisplayColumn = BoardProjection["columns"][number] & {
    totalCardCount: number;
    hiddenLaneCardCount: number;
    hiddenDoneCardCount: number;
  };

  type BoardIndex = {
    cards: CardProjection[];
    cardById: Map<string, CardProjection>;
    columnById: Map<string, BoardProjection["columns"][number]>;
    columnByIntent: Map<ColumnIntent, BoardProjection["columns"][number]>;
    cardColumnIntentById: Map<string, ColumnIntent>;
  };

  let workspace = $state<WorkspaceProjection | null>(null);
  let cacheSavedAt = $state<number | null>(null);
  let error = $state<string | null>(null);
  let syncError = $state<string | null>(null);
  let syncing = $state(false);
  let testingConnection = $state(false);
  let loadingBoards = $state(false);
  let connectingJira = $state(false);
  let testingWorker = $state(false);
  let runningWorkerCardIds = $state<Record<string, true>>({});
  let connectionMessage = $state<string | null>(null);
  let workerStatus = $state<AiWorkerStatus | null>(null);
  let agentConnectionStates = $state<Record<string, AgentConnectionState>>(loadAgentConnectionStates());
  let mcpConnectionStates = $state<Record<string, McpConnectionState>>({});
  let appNotice = $state<{ tone: "info" | "success" | "error"; message: string } | null>(null);
  let mcpToolsByServer = $state<Record<string, string[]>>({});
  let settingsOpen = $state(false);
  let settingsTab = $state<"agent" | "rules" | "skills" | "mcp" | "jira" | "theme">("agent");
  let settingsError = $state<string | null>(null);
  let settings = $state<AppSettings>(initialSettings);
  let appSecrets = $state<AppSecrets>(initialAppSecrets);
  let secretsHydrated = $state(false);
  let workspaceCacheHydrated = $state(false);
  let filesStateHydrated = $state(false);
  let selectedServerId = $state(initialSettings.jira.serverId);
  let workspaceMode = $state<WorkspaceMode>(initialUiState.workspaceMode);
  let doneVisibleLimit = $state<number | "all">(initialUiState.doneVisibleLimit);
  let laneVisibleLimits = $state<Record<string, number>>({});
  let now = $state(new Date());
  let selectedCardId = $state<string | null>(null);
  let draggedCardId = $state<string | null>(null);
  let newTaskOpen = $state(false);
  let newTaskTitle = $state("");
  let newTaskDescription = $state("");
  let agentConsoleOpen = $state(false);
  let agentConsoleCardId = $state<string | null>(null);
  let agentRunCardId = $state<string | null>(null);
  let agentRunTitle = $state("No active run");
  let agentRunStatus = $state<AgentRunStatus>("idle");
  let agentRunProgress = $state(0);
  let agentRunOutput = $state("");
  let agentRunResult = $state<AiWorkerTaskResult | null>(null);
  let agentRunLogs = $state<AgentRunLog[]>([]);
  let agentTerminalLines = $state<AgentTerminalLine[]>([]);
  let agentRunGitSnapshot = $state<AgentRunGitSnapshot | null>(null);
  let agentRunTranscript = $state<AgentSessionEvent[]>([]);
  let agentTerminalInput = $state("");
  let agentRunSessions = $state<Record<string, AgentRunSession>>({});
  let workspaceShellWorkdir = $state(initialUiState.workspaceShellWorkdir);
  let workspaceTerminalContainer: HTMLDivElement | null = $state(null);
  let workspaceTerminal: XtermTerminal | null = null;
  let workspaceFitAddon: XtermFitAddon | null = null;
  let workspaceTerminalResizeObserver: ResizeObserver | null = null;
  let workspaceTerminalOpened = $state(false);
  const workspaceTerminalId = "main-workspace-terminal";
  let workspaceTerminalRuntime: Promise<{
    Terminal: typeof import("@xterm/xterm").Terminal;
    FitAddon: typeof import("@xterm/addon-fit").FitAddon;
  }> | null = null;
  let editorWorkspaceModule = $state<typeof import("$lib/components/EditorWorkspace.svelte") | null>(null);
  let editorWorkspaceRuntime: Promise<typeof import("$lib/components/EditorWorkspace.svelte")> | null = null;
  let fileBrowserModule = $state<typeof import("$lib/components/FileBrowserPane.svelte") | null>(null);
  let fileBrowserRuntime: Promise<typeof import("$lib/components/FileBrowserPane.svelte")> | null = null;
  let gitActionsModule = $state<typeof import("$lib/components/GitActionsPane.svelte") | null>(null);
  let gitActionsRuntime: Promise<typeof import("$lib/components/GitActionsPane.svelte")> | null = null;
  let workspaceChatModule = $state<typeof import("$lib/components/WorkspaceChatPane.svelte") | null>(null);
  let workspaceChatRuntime: Promise<typeof import("$lib/components/WorkspaceChatPane.svelte")> | null = null;
  let mcpConnectionModule = $state<typeof import("$lib/components/McpConnectionSettings.svelte") | null>(null);
  let mcpConnectionRuntime: Promise<typeof import("$lib/components/McpConnectionSettings.svelte")> | null = null;
  let agentConsoleModule = $state<typeof import("$lib/components/AgentConsolePanel.svelte") | null>(null);
  let agentConsoleRuntime: Promise<typeof import("$lib/components/AgentConsolePanel.svelte")> | null = null;
  let workspaceChatTextarea: HTMLTextAreaElement | null = $state(null);
  let workspaceChatEnd: HTMLDivElement | null = $state(null);
  let agentRulesTextarea: HTMLTextAreaElement | null = $state(null);
  let agentSkillsTextarea: HTMLTextAreaElement | null = $state(null);
  let workspaceChatRunning = $state(false);
  let workspaceChatSession = $state<ChatSessionState>(initialUiState.workspaceChatSession);
  let workspaceChatSessions = $state<ChatSessionState[]>(initialUiState.workspaceChatSessions);
  let workspaceChatActiveSessionId = $state<string>(initialUiState.workspaceChatActiveSessionId ?? initialUiState.workspaceChatSession.id);
  let workspaceChatMessages = $state<WorkspaceChatMessage[]>(initialUiState.workspaceChatMessages);
  let layoutPrefs = $state<LayoutPrefs>(loadLayoutPrefs());
  let layoutResizeDrag: LayoutResizeDrag | null = null;
  let uiStateSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let appNoticeTimer: ReturnType<typeof setTimeout> | null = null;
  let terminalFrameId: number | null = null;
  let fileEntries = $state<FileEntry[]>([]);
  let fileDirectory = $state("");
  let fileRootLabel = $state("~");
  let fileLoading = $state(false);
  let fileError = $state<string | null>(null);
  let fileFilter = $state("");
  let expandedFileEntries = $state<Record<string, FileEntry[]>>({});
  let expandingFilePaths = $state<Record<string, true>>({});
  let fileTreeRevision = 0;
  let fileSidebarCollapsed = $state(false);
  let workspaceSidebarTab = $state<"explorer" | "source-control">("explorer");
  let workspaceFilesRoot = $state(initialUiState.workspaceFilesRoot);
  let workspaceFilesDirectory = $state(initialUiState.workspaceFilesDirectory);
  let openEditorFiles = $state<OpenEditorFile[]>([]);
  let activeEditorPath = $state<string | null>(null);
  let savingFilePath = $state<string | null>(null);
  let formattingFilePath = $state<string | null>(null);
  let editorDiagnostic = $state<string | null>(null);
  let workspaceRoot = $state<string | null>(null);
  let workspaceGitInfo = $state<GitWorkspaceInfo | null>(null);
  let workspaceGitLoading = $state(false);
  let workspaceGitError = $state<string | null>(null);
  let workspaceGitStatus = $state<GitStatus>({ staged: [], unstaged: [] });
  let selectedWorkspaceBranch = $state("");
  let switchingWorkspaceBranch = $state(false);
  let editorDiagnosticTimer: ReturnType<typeof setTimeout> | null = null;
  let workspaceGitInfoRequestId = 0;
  let workspaceGitStatusRequestId = 0;
  let backlogStartConfirmation = $state<{ cardId: string; title: string } | null>(null);
  let backlogStartConfirmationResolve: ((confirmed: boolean) => void) | null = null;
  let manualDoneConfirmation = $state<{ cardId: string; title: string } | null>(null);
  const sourceControl = createSourceControlStore({
    onRepositoryChanged: async (refreshFiles, refreshEditors) => {
      if (refreshFiles) await refreshFileDirectory(fileDirectory);
      if (refreshEditors) await refreshOpenEditorFilesFromDisk();
      syncSourceControlState();
    },
    onNotice: (tone, message) => {
      appNotice = { tone, message };
    },
  });

  onMount(() => {
    void hydrateCachedWorkspace();

    const timer = window.setInterval(() => {
      now = new Date();
    }, 60_000);

    return () => window.clearInterval(timer);
  });

  $effect(() => {
    if (workspace || !workspaceCacheHydrated) return;

    getWorkspace()
      .then((projection) => {
        workspace = projection;
      })
      .catch((reason: unknown) => {
        error = reason instanceof Error ? reason.message : String(reason);
      });
  });

  $effect(() => {
    if (workspaceMode === "term" && workspaceTerminalContainer) {
      if (workspaceTerminalOpened) {
        scheduleWorkspaceTerminalActivation();
      } else {
        scheduleWorkspaceTerminalInit();
      }
    }
  });

  $effect(() => {
    if (workspaceMode === "files") {
      void loadFileBrowserRuntime();
      void loadEditorWorkspaceRuntime();
      void loadGitActionsRuntime();
    } else if (workspaceMode === "term") {
      void loadWorkspaceChatRuntime();
    }
  });

  $effect(() => {
    if (settingsOpen && settingsTab === "mcp") {
      void loadMcpConnectionRuntime();
    }
  });

  $effect(() => {
    if (agentConsoleOpen && hasAgentConsoleSession) {
      void loadAgentConsoleRuntime();
    }
  });

  $effect(() => {
    if (!agentConsoleCardId || agentRunSessions[agentConsoleCardId]) return;
    const fallback = latestAgentSession;
    agentConsoleCardId = fallback?.cardId ?? null;
    if (!fallback) agentConsoleOpen = false;
  });

  $effect(() => {
    if (workspaceMode === "files" && workspace && fileEntries.length === 0 && !fileLoading) {
      void refreshFileDirectory(fileDirectory);
    }
  });

  $effect(() => {
    if (!workspace || workspaceRoot) return;
    workspaceRootPath(workspace.id)
      .then((path) => {
        if (workspaceRoot) return;
        workspaceRoot = normalizeAbsolutePath(path);
        fileRootLabel = displayPath(path);
      })
      .catch(() => {
        workspaceRoot = null;
      });
  });

  $effect(() => {
    if (!workspaceRoot) {
      workspaceGitInfo = null;
      workspaceGitError = null;
      workspaceGitLoading = false;
      workspaceGitStatus = { staged: [], unstaged: [] };
      selectedWorkspaceBranch = "";
      return;
    }

    void refreshWorkspaceGitState();
  });

  async function refreshWorkspaceGitInfo() {
    if (!workspaceRoot) {
      workspaceGitInfo = null;
      workspaceGitError = null;
      workspaceGitLoading = false;
      selectedWorkspaceBranch = "";
      return;
    }

    const requestId = ++workspaceGitInfoRequestId;
    await sourceControl.refresh("info");
    if (requestId === workspaceGitInfoRequestId) syncSourceControlState();
  }

  async function refreshWorkspaceGitStatus() {
    if (!workspaceRoot) {
      workspaceGitStatus = { staged: [], unstaged: [] };
      return;
    }

    const requestId = ++workspaceGitStatusRequestId;
    await sourceControl.refresh("status");
    if (requestId === workspaceGitStatusRequestId) syncSourceControlState();
  }

  async function refreshWorkspaceGitState() {
    await sourceControl.refresh("state");
    syncSourceControlState();
  }

  function syncSourceControlState() {
    workspaceGitInfo = sourceControl.info;
    workspaceGitStatus = sourceControl.status;
    workspaceGitError = sourceControl.error;
    workspaceGitLoading = sourceControl.loading;
    selectedWorkspaceBranch = sourceControl.info?.current_branch ?? "";
  }

  $effect(() => {
    if (!workspace || filesStateHydrated) return;
    filesStateHydrated = true;
    void restoreFilesState();
  });

  $effect(() => {
    if (secretsHydrated) return;
    secretsHydrated = true;
    void hydrateSecrets();
  });

  $effect(() => {
    if (appNoticeTimer) {
      clearTimeout(appNoticeTimer);
      appNoticeTimer = null;
    }

    if (!appNotice) return;

    const notice = appNotice;
    appNoticeTimer = setTimeout(() => {
      if (appNotice === notice) appNotice = null;
      appNoticeTimer = null;
    }, notice.tone === "error" ? ERROR_NOTICE_AUTO_DISMISS_MS : NOTICE_AUTO_DISMISS_MS);
  });

  onDestroy(() => {
    if (appNoticeTimer) clearTimeout(appNoticeTimer);
    if (terminalFrameId !== null) window.cancelAnimationFrame(terminalFrameId);
    if (editorDiagnosticTimer) clearTimeout(editorDiagnosticTimer);
    resolveBacklogStartConfirmation(false);
    flushUiState();
    workspaceTerminalResizeObserver?.disconnect();
    void closePtyTerminal(workspaceTerminalId).catch(() => {});
    workspaceTerminal?.dispose();
  });

  let activeBoard = $derived<BoardProjection | null>(
    workspace?.projects[0]?.boards[0] ?? null,
  );
  let displayColumns = $derived<BoardDisplayColumn[]>(
    activeBoard?.columns.map((column) => {
      const cards = visibleCardsForColumn(column);
      return {
        ...column,
        cards,
        totalCardCount: column.cards.length,
        hiddenLaneCardCount: column.intent === "done" ? 0 : Math.max(0, column.cards.length - cards.length),
        hiddenDoneCardCount: column.intent === "done" ? Math.max(0, column.cards.length - cards.length) : 0,
      };
    }) ?? [],
  );
  let boardIndex = $derived.by((): BoardIndex => {
    const cards: CardProjection[] = [];
    const cardById = new Map<string, CardProjection>();
    const columnById = new Map<string, BoardProjection["columns"][number]>();
    const columnByIntent = new Map<ColumnIntent, BoardProjection["columns"][number]>();
    const cardColumnIntentById = new Map<string, ColumnIntent>();

    for (const column of activeBoard?.columns ?? []) {
      columnById.set(column.id, column);
      columnByIntent.set(column.intent, column);

      for (const card of column.cards) {
        cards.push(card);
        cardById.set(card.id, card);
        cardColumnIntentById.set(card.id, column.intent);
      }
    }

    return { cards, cardById, columnById, columnByIntent, cardColumnIntentById };
  });
  let activeCards = $derived(boardIndex.cards);
  let renderedCardCount = $derived(
    displayColumns.reduce((count, column) => count + column.cards.length, 0),
  );
  let activeCardById = $derived(boardIndex.cardById);
  let activeCardIds = $derived(new Set(activeCardById.keys()));
  let activeColumnById = $derived(boardIndex.columnById);
  let activeColumnByIntent = $derived(boardIndex.columnByIntent);
  let cardColumnIntentById = $derived(boardIndex.cardColumnIntentById);
  let selectedCard = $derived<CardProjection | null>(
    selectedCardId ? activeCardById.get(selectedCardId) ?? null : null,
  );
  let selectedCardAgentSession = $derived<AgentRunSession | null>(
    selectedCardId ? agentRunSessions[selectedCardId] ?? null : null,
  );
  let activeEditorFile = $derived<OpenEditorFile | null>(
    activeEditorPath ? openEditorFiles.find((file) => file.path === activeEditorPath) ?? null : null,
  );
  let activeEditorReady = $derived(Boolean(activeEditorFile?.editor));
  let activeEditorDirty = $derived(Boolean(activeEditorFile?.dirty));
  let hasDirtyEditorFiles = $derived(openEditorFiles.some((file) => file.dirty));

  let fileStatusLabel = $derived(
    fileLoading
      ? "Loading files"
      : fileError
        ? "File error"
        : activeEditorFile && !activeEditorReady
          ? "Loading editor"
          : activeEditorFile
            ? `${activeEditorFile.path}${activeEditorFile.dirty ? " • unsaved" : ""}`
            : `${fileEntries.length} item${fileEntries.length === 1 ? "" : "s"}`,
  );
  let workspaceChangedFiles = $derived([...workspaceGitStatus.staged, ...workspaceGitStatus.unstaged]);
  let sourceControlChangedCount = $derived(workspaceChangedFiles.length);
  let selectedServer = $derived(
    settings.mcpServers.find((server) => server.id === selectedServerId) ??
      settings.mcpServers[0],
  );
  let selectedMcpTools = $derived(selectedServer ? mcpToolsByServer[selectedServer.id] ?? [] : []);
  let currentDate = $derived(
    now.toLocaleDateString(undefined, {
      weekday: "short",
      year: "numeric",
      month: "short",
      day: "numeric",
    }),
  );
  let currentTime = $derived(
    now.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    }),
  );
  let cacheStatusLabel = $derived(cacheSavedAt ? `Cached ${relativeTime(cacheSavedAt)}` : "No cached board");
  let boardResourceLabel = $derived(`${renderedCardCount}/${activeCards.length} cards rendered`);
  let cacheSizeLabel = $derived(cacheSavedAt ? `Cache ${formatBytes(cachedWorkspaceSizeBytes())}` : "Cache empty");
  let syncBudgetLabel = $derived(
    `Fast sync: up to ${settings.jira.pageSize * settings.jira.maxPages} Jira cards (${settings.jira.pageSize}/page × ${settings.jira.maxPages} page${settings.jira.maxPages === 1 ? "" : "s"}).`,
  );
  let selectedAiProvider = $derived(providerById(settings.aiWorker.providerId));
  let selectedAiModel = $derived(modelById(selectedAiProvider, settings.aiWorker.modelId));
  let selectedAiApiKey = $derived(appSecrets.ai_api_keys[selectedAiProvider.id] ?? "");
  let selectedAiEndpoint = $derived(
    selectedAiProvider.apiStyle === "anthropic_messages"
      ? `${selectedAiProvider.baseUrl}/messages`
      : selectedAiProvider.apiStyle === "openai_responses"
        ? `${selectedAiProvider.baseUrl}/responses`
      : `${selectedAiProvider.baseUrl}/chat/completions`,
  );
  let selectedAgentLabel = $derived(
    settings.aiWorker.runtime === "opencode"
      ? `OpenCode · ${settings.aiWorker.opencodeModel}`
      : `${selectedAiProvider.label} · ${selectedAiModel.label}`,
  );
  let selectedAgentStatusKey = $derived(
    settings.aiWorker.runtime === "opencode"
      ? `opencode:${settings.aiWorker.opencodeCommand}:${settings.aiWorker.opencodeModel}`
      : `api:${selectedAiProvider.id}:${selectedAiModel.id}`,
  );
  let selectedAgentConnection = $derived(agentConnectionStates[selectedAgentStatusKey] ?? null);
  let workerConnected = $derived(selectedAgentConnection?.connected === true);
  let workerStatusLabel = $derived(
    workerConnected
      ? `${selectedAgentLabel} connected · ${relativeTime(selectedAgentConnection?.testedAt ?? Date.now())}`
      : `${selectedAgentLabel} not tested`,
  );
  let visibleAgentSession = $derived<AgentRunSession | null>(
    agentConsoleCardId ? agentRunSessions[agentConsoleCardId] ?? null : null,
  );
  let visibleAgentRunTitle = $derived(visibleAgentSession?.title ?? "No active run");
  let visibleAgentRunStatus = $derived<AgentRunStatus>(visibleAgentSession?.status ?? "idle");
  let visibleAgentRunProgress = $derived(visibleAgentSession?.progress ?? 0);
  let visibleAgentRunOutput = $derived(visibleAgentSession?.output ?? "");
  let visibleAgentRunResult = $derived(visibleAgentSession?.result ?? null);
  let visibleAgentRunLogs = $derived(visibleAgentSession?.logs ?? []);
  let visibleAgentTerminalLines = $derived(visibleAgentSession?.terminalLines ?? []);
  let visibleAgentRunTranscript = $derived(visibleAgentSession?.transcript ?? []);
  let visibleAgentLog = $derived(visibleAgentRunLogs.at(-1) ?? null);
  let agentActivityView = $derived(agentActivity(visibleAgentRunStatus, visibleAgentRunProgress, visibleAgentLog));
  let agentActivityTitle = $derived(agentActivityView.title);
  let agentActivityDetail = $derived(agentActivityView.detail);
  let agentNextStep = $derived(agentActivityView.next);
  let agentPhases = $derived(agentPhaseTimeline(visibleAgentRunStatus, visibleAgentRunProgress));
  let hasAgentConsoleSession = $derived(Boolean(visibleAgentSession));
  let latestAgentSession = $derived<AgentRunSession | null>(
    Object.values(agentRunSessions).sort((left, right) => {
      const leftLog = left.logs.at(-1)?.id ?? "";
      const rightLog = right.logs.at(-1)?.id ?? "";
      return rightLog.localeCompare(leftLog);
    })[0] ?? null,
  );
  let settingsTitle = $derived(
    {
      agent: "Agent",
      rules: "Agent Rules",
      skills: "Agent Skills",
      mcp: "MCP Connections",
      jira: "Jira Sync",
      theme: "Theme",
    }[settingsTab],
  );

  function relativeTime(timestamp: number): string {
    const elapsedMs = Math.max(0, now.getTime() - timestamp);
    const elapsedMinutes = Math.floor(elapsedMs / 60_000);
    if (elapsedMinutes < 1) return "just now";
    if (elapsedMinutes < 60) return `${elapsedMinutes}m ago`;

    const elapsedHours = Math.floor(elapsedMinutes / 60);
    if (elapsedHours < 24) return `${elapsedHours}h ago`;

    const elapsedDays = Math.floor(elapsedHours / 24);
    return `${elapsedDays}d ago`;
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1_024) return `${bytes} B`;
    if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
    return `${(bytes / 1_048_576).toFixed(1)} MB`;
  }

  function loadLayoutPrefs(): LayoutPrefs {
    if (typeof localStorage === "undefined") return { ...defaultLayoutPrefs };
    try {
      const parsed = JSON.parse(localStorage.getItem(LAYOUT_PREFS_KEY) ?? "{}");
      return normalizeLayoutPrefs(parsed);
    } catch {
      return { ...defaultLayoutPrefs };
    }
  }

  function saveUiState() {
    if (uiStateSaveTimer) clearTimeout(uiStateSaveTimer);
    uiStateSaveTimer = setTimeout(() => {
      uiStateSaveTimer = null;
      flushUiState();
    }, UI_STATE_WRITE_DELAY_MS);
  }

  async function hydrateSecrets() {
    const localSecrets = appSecrets;
    try {
      const storedSecrets = await loadAppSecrets();
      const mergedSecrets = hasAnySecret(localSecrets)
        ? mergeAppSecrets(localSecrets, storedSecrets)
        : storedSecrets;

      if (hasAnySecret(localSecrets)) {
        await saveAppSecrets(mergedSecrets);
        saveSettings(settingsWithoutSecrets(settings));
      }

      appSecrets = mergedSecrets;
      settings = settingsWithoutSecrets(settings);
      secretsHydrated = true;
    } catch (reason: unknown) {
      appNotice = {
        tone: "error",
        message: `Could not load secure settings: ${reason instanceof Error ? reason.message : String(reason)}`,
      };
    }
  }

  async function hydrateCachedWorkspace() {
    try {
      const cached = await loadCachedWorkspace();
      if (cached && !workspace) {
        workspace = cached.workspace;
        cacheSavedAt = cached.savedAt;
        appNotice = { tone: "info", message: "Loaded saved cards. Sync Jira only when you need fresh updates." };
      }
    } catch (reason: unknown) {
      appNotice = {
        tone: "error",
        message: `Could not load workspace cache: ${reason instanceof Error ? reason.message : String(reason)}`,
      };
    } finally {
      workspaceCacheHydrated = true;
    }
  }

  async function persistSettingsAndSecrets(value: AppSettings) {
    const storedSecrets = await loadAppSecrets();
    const mergedSecrets = hasAnySecret(appSecrets) ? mergeAppSecrets(appSecrets, storedSecrets) : storedSecrets;
    await saveAppSecrets(mergedSecrets);
    saveSettings(value);
  }

  function flushUiState() {
    if (typeof localStorage === "undefined") return;
    if (uiStateSaveTimer) {
      clearTimeout(uiStateSaveTimer);
      uiStateSaveTimer = null;
    }

    localStorage.setItem(
      UI_STATE_KEY,
      serializeUiState({
        workspaceMode,
        workspaceShellWorkdir,
        workspaceChatMessages: workspaceChatMessages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
        workspaceChatSession: {
          ...workspaceChatSession,
          messages: workspaceChatMessages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
          activities: workspaceChatSession.activities.slice(-MAX_WORKSPACE_CHAT_ACTIVITIES),
          recentCardIds: workspaceChatSession.recentCardIds.slice(0, MAX_WORKSPACE_CHAT_RECENT_CARDS),
        },
        workspaceChatSessions: workspaceChatSessions.map((session) => ({
          ...session,
          messages: session.id === workspaceChatSession.id
            ? workspaceChatMessages.slice(-MAX_WORKSPACE_CHAT_MESSAGES)
            : session.messages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
          activities: session.activities.slice(-MAX_WORKSPACE_CHAT_ACTIVITIES),
          recentCardIds: session.recentCardIds.slice(0, MAX_WORKSPACE_CHAT_RECENT_CARDS),
        })),
        workspaceChatActiveSessionId,
        doneVisibleLimit,
        workspaceFilesRoot,
        workspaceFilesDirectory,
        workspaceFilesActivePath: activeEditorPath,
      } satisfies UiState),
    );
  }

  function setDoneVisibleLimit(limit: number | "all") {
    doneVisibleLimit = limit;
    saveUiState();
  }

  function setWorkspaceMode(mode: WorkspaceMode) {
    if (workspaceMode === mode) return;
    workspaceMode = mode;
    saveUiState();
    if (mode === "term") {
      if (workspaceTerminalOpened) {
        scheduleWorkspaceTerminalActivation();
      } else {
        scheduleWorkspaceTerminalInit();
      }
    } else if (mode === "files") {
      void refreshFileDirectory(fileDirectory);
    }
  }

  async function refreshFileDirectory(relativePath = fileDirectory) {
    if (!workspace || fileLoading) return;
    const revision = fileTreeRevision + 1;
    fileTreeRevision = revision;
    fileLoading = true;
    fileError = null;
    fileDirectory = relativePath;
    workspaceFilesDirectory = relativePath;
    saveUiState();
    try {
      const entries = await listDirectory(workspace.id, relativePath);
      if (revision !== fileTreeRevision) return;
      fileEntries = entries;
      expandedFileEntries = {};
      expandingFilePaths = {};
    } catch (reason: unknown) {
      if (revision !== fileTreeRevision) return;
      fileError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (revision === fileTreeRevision) fileLoading = false;
    }
  }

  async function refreshWorkspaceRootLabel() {
    if (!workspace) return;
    const root = await workspaceRootPath(workspace.id);
    workspaceRoot = normalizeAbsolutePath(root);
    workspaceFilesRoot = workspaceRoot;
    fileRootLabel = displayPath(root);
  }

  async function refreshOpenEditorFilesFromDisk() {
    if (!workspace || openEditorFiles.length === 0) return;

    const activePath = activeEditorPath;
    const refreshedFiles: OpenEditorFile[] = [];
    const missingFiles: string[] = [];

    for (const file of openEditorFiles) {
      try {
        const content = await readFile(workspace.id, file.path);
        file.editor?.setValue(content);
        file.editor?.markSaved(content);
        refreshedFiles.push({
          ...file,
          content,
          savedContent: content,
          dirty: false,
        });
      } catch {
        missingFiles.push(file.path);
      }
    }

    openEditorFiles = refreshedFiles;
    activeEditorPath =
      (activePath && refreshedFiles.some((file) => file.path === activePath) ? activePath : null) ??
      refreshedFiles[0]?.path ??
      null;
    saveUiState();

    if (missingFiles.length > 0) {
      appNotice = {
        tone: "error",
        message: `${missingFiles.length} open file${missingFiles.length === 1 ? " was" : "s were"} missing on this branch and closed.`,
      };
    }

    await tick();
    activeEditorFile?.editor?.focus();
    await validateActiveEditorSyntax();
  }

  async function switchWorkspaceBranch(branch: string) {
    if (!workspaceGitInfo?.is_git_repo || switchingWorkspaceBranch) return;
    if (openEditorFiles.some((file) => file.dirty)) {
      appNotice = {
        tone: "error",
        message: "Save or discard open files before switching branches.",
      };
      selectedWorkspaceBranch = workspaceGitInfo.current_branch ?? branch;
      return;
    }

    switchingWorkspaceBranch = true;
    workspaceGitError = null;
    fileError = null;
    try {
      await sourceControl.checkoutBranch(branch);
      syncSourceControlState();
      if (sourceControl.error) return;
      appNotice = { tone: "success", message: `Switched to ${selectedWorkspaceBranch || branch}` };
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      selectedWorkspaceBranch = workspaceGitInfo?.current_branch ?? branch;
    } finally {
      switchingWorkspaceBranch = false;
    }
  }

  async function pullWorkspaceGitChanges() {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.pull();
      syncSourceControlState();
      if (sourceControl.error) return;
      appNotice = { tone: "success", message: "Pulled latest changes." };
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
    }
  }

  async function commitWorkspaceGitChanges(message: string) {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.commit(message);
      syncSourceControlState();
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
    }
  }

  async function stageWorkspaceGitPath(path: string) {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.stageFile(path);
      syncSourceControlState();
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      await refreshWorkspaceGitState();
    }
  }

  async function stageAllWorkspaceGitPaths() {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.stageAll();
      syncSourceControlState();
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      await refreshWorkspaceGitState();
    }
  }

  async function unstageWorkspaceGitPath(path: string) {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.unstageFile(path);
      syncSourceControlState();
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      await refreshWorkspaceGitState();
    }
  }

  async function unstageAllWorkspaceGitPaths() {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.unstageAll();
      syncSourceControlState();
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      await refreshWorkspaceGitState();
    }
  }

  async function pushWorkspaceGitChanges() {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.push();
      syncSourceControlState();
      if (sourceControl.error) return;
      appNotice = { tone: "success", message: "Pushed changes." };
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
    }
  }

  async function mergeWorkspaceGitBranch(branch: string) {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.merge(branch);
      syncSourceControlState();
      if (sourceControl.error) return;
      appNotice = { tone: "success", message: `Merged ${branch}` };
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
    }
  }

  async function rebaseWorkspaceGitBranch(branch: string) {
    if (!workspaceGitInfo?.is_git_repo) return;
    try {
      await sourceControl.rebase(branch);
      syncSourceControlState();
      if (sourceControl.error) return;
      appNotice = { tone: "success", message: `Rebased onto ${branch}` };
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
    }
  }

  async function restoreFilesState() {
    if (!workspace) return;

    const savedRoot = normalizeAbsolutePath(initialUiState.workspaceFilesRoot);
    if (savedRoot && savedRoot !== workspaceRoot) {
      await setWorkspaceRoot(savedRoot);
      workspaceRoot = savedRoot;
      workspaceFilesRoot = savedRoot;
      fileRootLabel = displayPath(savedRoot);
    }

    const savedActivePath = initialUiState.workspaceFilesActivePath;
    const targetDirectory = "";

    if (fileDirectory !== targetDirectory) {
      await refreshFileDirectory(targetDirectory);
    }

    if (savedActivePath) {
      const existingFile = openEditorFiles.find((file) => file.path === savedActivePath);
      if (existingFile) {
        activeEditorPath = savedActivePath;
        await expandFileAncestors(savedActivePath);
        saveUiState();
        return;
      }

      await openFileEntry({ name: fileName(savedActivePath), path: savedActivePath, is_dir: false, size: 0 });
    }

    saveUiState();
  }

  async function openFolderFromDialog() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const selected = await openDialogIfAvailable({ directory: true, multiple: false, defaultPath: workspaceRoot ?? undefined });
    if (typeof selected !== "string") return;

    await setWorkspaceRoot(selected);
    fileDirectory = "";
    workspaceFilesDirectory = "";
    openEditorFiles = [];
    activeEditorPath = null;
    expandedFileEntries = {};
    expandingFilePaths = {};
    fileTreeRevision += 1;
    fileFilter = "";
    await refreshWorkspaceRootLabel();
    await refreshFileDirectory("");
    await refreshWorkspaceGitState();
    saveUiState();
    appNotice = { tone: "success", message: `Opened folder ${fileRootLabel}` };
  }

  async function openFileFromDialog() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const selected = await openDialogIfAvailable({ directory: false, multiple: false, defaultPath: workspaceRoot ?? undefined });
    if (typeof selected !== "string") return;

    const normalized = normalizeAbsolutePath(selected);
    const separator = normalized.lastIndexOf("/");
    const parent = separator > 0 ? normalized.slice(0, separator) : normalized;
    const name = separator > 0 ? normalized.slice(separator + 1) : normalized;
    await setWorkspaceRoot(parent);
    fileDirectory = "";
    workspaceFilesDirectory = "";
    expandedFileEntries = {};
    expandingFilePaths = {};
    fileTreeRevision += 1;
    fileFilter = "";
    await refreshWorkspaceRootLabel();
    await refreshFileDirectory("");
    await expandFileAncestors(name);
    await openFileEntry({ name, path: name, is_dir: false, size: 0 });
    await refreshWorkspaceGitState();
    saveUiState();
    appNotice = { tone: "success", message: `Opened ${name}` };
  }

  async function createNewFile() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const target = window.prompt("New file name", fileDirectory ? `${fileDirectory}/untitled.txt` : "untitled.txt");
    if (!target) return;

    const normalized = target.replace(/^\/+/, "").trim();
    if (!normalized) return;

    await writeFile(workspace.id, normalized, "");
    await refreshWorkspaceGitState();
    await refreshFileDirectory(fileDirectory);
    await openFileEntry({ name: fileName(normalized), path: normalized, is_dir: false, size: 0 });
    appNotice = { tone: "success", message: `Created ${normalized}` };
  }

  async function openFileEntry(entry: FileEntry) {
    if (entry.is_dir) {
      await toggleFileFolder(entry);
      return;
    }

    if (!workspace) return;
    activeEditorPath = entry.path;
    saveUiState();
    if (openEditorFiles.some((file) => file.path === entry.path)) {
      await tick();
      activeEditorFile?.editor?.focus();
      return;
    }

    fileLoading = true;
    fileError = null;
    try {
      const content = await readFile(workspace.id, entry.path);
      openEditorFiles = [
        ...openEditorFiles,
        { path: entry.path, name: entry.name, content, savedContent: content, dirty: false, editor: null },
      ];
      await expandFileAncestors(entry.path);
      await tick();
      activeEditorFile?.editor?.focus();
      scheduleEditorDiagnostics();
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      activeEditorPath = openEditorFiles.at(-1)?.path ?? null;
    } finally {
      fileLoading = false;
    }
  }

  async function toggleFileFolder(entry: FileEntry) {
    if (!workspace || !entry.is_dir || expandingFilePaths[entry.path]) return;

    if (expandedFileEntries[entry.path]) {
      expandedFileEntries = pruneExpandedFolderTree(expandedFileEntries, entry.path);
      return;
    }

    const revision = fileTreeRevision;
    expandingFilePaths = { ...expandingFilePaths, [entry.path]: true };
    fileError = null;
    try {
      const children = await listDirectory(workspace.id, entry.path);
      if (revision !== fileTreeRevision) return;
      expandedFileEntries = { ...expandedFileEntries, [entry.path]: children };
    } catch (reason: unknown) {
      if (revision !== fileTreeRevision) return;
      fileError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (revision !== fileTreeRevision) return;
      const { [entry.path]: _finished, ...remaining } = expandingFilePaths;
      expandingFilePaths = remaining;
    }
  }

  function clearFileFilter() {
    fileFilter = "";
  }

  function collapseAllFileFolders() {
    expandedFileEntries = {};
    expandingFilePaths = {};
  }

  async function expandFileAncestors(path: string) {
    if (!workspace) return;

    for (const current of collectAncestorPaths(path)) {
      if (expandedFileEntries[current] || expandingFilePaths[current]) continue;
      const folderEntry: FileEntry = { name: current.split("/").at(-1) ?? current, path: current, is_dir: true, size: 0 };
      await toggleFileFolder(folderEntry);
    }
  }

  function setEditorDirty(path: string, dirty: boolean) {
    openEditorFiles = openEditorFiles.map((file) =>
      file.path === path ? { ...file, dirty } : file,
    );
    if (path === activeEditorPath) {
      scheduleEditorDiagnostics();
    }
  }

  function selectEditorTab(path: string) {
    if (activeEditorPath === path) return;
    activeEditorPath = path;
    saveUiState();
    void tick().then(() => {
      activeEditorFile?.editor?.focus();
      scheduleEditorDiagnostics();
    });
  }

  function closeEditorTab(path: string) {
    const index = openEditorFiles.findIndex((file) => file.path === path);
    openEditorFiles = openEditorFiles.filter((file) => file.path !== path);
    if (activeEditorPath === path) {
      activeEditorPath = openEditorFiles[Math.max(0, index - 1)]?.path ?? openEditorFiles[0]?.path ?? null;
    }
    saveUiState();
  }

  async function saveActiveFile() {
    const editor = activeEditorFile?.editor;
    if (!workspace || !activeEditorPath || !editor) return;
    const path = activeEditorPath;
    const content = editor.getValue();
    savingFilePath = path;
    fileError = null;
    try {
      await writeFile(workspace.id, path, content);
      openEditorFiles = openEditorFiles.map((file) =>
        file.path === path ? { ...file, content, savedContent: content, dirty: false } : file,
      );
      editor.markSaved(content);
      appNotice = { tone: "success", message: `Saved ${path}` };
      await validateActiveEditorSyntax();
      await refreshWorkspaceGitState();
      void refreshFileDirectory(fileDirectory);
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      savingFilePath = null;
    }
  }

  async function formatActiveFile() {
    const editor = activeEditorFile?.editor;
    if (!activeEditorPath || !editor) return;

    formattingFilePath = activeEditorPath;
    fileError = null;
    try {
      const formatted = await formatEditorText(activeEditorPath, editor.getValue());
      editor.setValue(formatted);
      await validateActiveEditorSyntax();
      appNotice = { tone: "success", message: `Formatted ${activeEditorPath}` };
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      formattingFilePath = null;
    }
  }

  function scheduleEditorDiagnostics() {
    if (editorDiagnosticTimer) clearTimeout(editorDiagnosticTimer);
    editorDiagnosticTimer = setTimeout(() => {
      editorDiagnosticTimer = null;
      void validateActiveEditorSyntax();
    }, 650);
  }

  async function validateActiveEditorSyntax() {
    const editor = activeEditorFile?.editor;
    if (!activeEditorPath || !editor) {
      editorDiagnostic = null;
      return;
    }
    editorDiagnostic = await validateEditorSyntax(activeEditorPath, editor.getValue());
  }

  function scheduleWorkspaceTerminalInit() {
    if (terminalFrameId !== null || workspaceTerminalOpened) return;

    terminalFrameId = window.requestAnimationFrame(() => {
      terminalFrameId = null;
      void initWorkspaceTerminal();
    });
  }

  function scheduleWorkspaceTerminalActivation() {
    if (terminalFrameId !== null) return;

    terminalFrameId = window.requestAnimationFrame(() => {
      terminalFrameId = null;
      if (!workspaceTerminal || !workspaceFitAddon || !workspaceTerminalContainer) return;
      workspaceFitAddon.fit();
      resizePtyTerminal(workspaceTerminalId, workspaceTerminal.rows, workspaceTerminal.cols).catch(() => {});
      workspaceTerminal.focus();
    });
  }

  function openSettings(tab?: typeof settingsTab) {
    if (tab) settingsTab = tab;
    settingsOpen = true;
  }

  function closeSettings() {
    settingsOpen = false;
  }

  function switchSettingsTab(tab: typeof settingsTab) {
    if (settingsTab === tab) return;
    settingsTab = tab;
  }

  function normalizeLayoutPrefs(value: Partial<LayoutPrefs>): LayoutPrefs {
    return {
      laneWidth: clampNumber(value.laneWidth, 260, 460, defaultLayoutPrefs.laneWidth),
      cardMinHeight: clampNumber(value.cardMinHeight, 170, 360, defaultLayoutPrefs.cardMinHeight),
      fileSidebarWidth: clampNumber(value.fileSidebarWidth, 240, 560, defaultLayoutPrefs.fileSidebarWidth),
      terminalWidth: clampNumber(value.terminalWidth, 420, 1100, defaultLayoutPrefs.terminalWidth),
      agentConsoleWidth: clampNumber(value.agentConsoleWidth, 360, 720, defaultLayoutPrefs.agentConsoleWidth),
      agentLogHeight: clampNumber(value.agentLogHeight, 110, 320, defaultLayoutPrefs.agentLogHeight),
      agentOutputHeight: clampNumber(value.agentOutputHeight, 140, 420, defaultLayoutPrefs.agentOutputHeight),
      agentApprovalHeight: clampNumber(value.agentApprovalHeight, 120, 360, defaultLayoutPrefs.agentApprovalHeight),
    };
  }

  function clampNumber(value: unknown, min: number, max: number, fallback: number): number {
    const numeric = typeof value === "number" && Number.isFinite(value) ? value : fallback;
    return Math.max(min, Math.min(max, numeric));
  }

  function saveLayoutPrefs() {
    localStorage.setItem(LAYOUT_PREFS_KEY, JSON.stringify(layoutPrefs));
  }

  function toggleFileSidebar() {
    fileSidebarCollapsed = !fileSidebarCollapsed;
    if (!fileSidebarCollapsed && layoutPrefs.fileSidebarWidth < 240) {
      layoutPrefs = { ...layoutPrefs, fileSidebarWidth: defaultLayoutPrefs.fileSidebarWidth };
    }
  }

  function resizeLayout(key: keyof LayoutPrefs, delta: number, min: number, max: number, invert = false) {
    layoutPrefs = {
      ...layoutPrefs,
      [key]: clampNumber(layoutPrefs[key] + (invert ? -delta : delta), min, max, defaultLayoutPrefs[key]),
    };
  }

  function beginLayoutResize(
    event: PointerEvent,
    key: keyof LayoutPrefs,
    min: number,
    max: number,
    axis: "x" | "y",
    invert = false,
  ) {
    event.preventDefault();
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    layoutResizeDrag = {
      key,
      min,
      max,
      invert,
      axis,
      pointerId: event.pointerId,
      lastPosition: axis === "x" ? event.clientX : event.clientY,
    };
  }

  function moveLayoutResize(event: PointerEvent) {
    if (!layoutResizeDrag || event.pointerId !== layoutResizeDrag.pointerId) return;
    const position = layoutResizeDrag.axis === "x" ? event.clientX : event.clientY;
    const delta = position - layoutResizeDrag.lastPosition;
    layoutResizeDrag.lastPosition = position;
    resizeLayout(layoutResizeDrag.key, delta, layoutResizeDrag.min, layoutResizeDrag.max, layoutResizeDrag.invert);
  }

  function endLayoutResize(event: PointerEvent) {
    if (!layoutResizeDrag || event.pointerId !== layoutResizeDrag.pointerId) return;
    const target = event.currentTarget as HTMLElement;
    try {
      target.releasePointerCapture(event.pointerId);
    } catch {
      // Pointer capture may already be released by the browser.
    }
    layoutResizeDrag = null;
    saveLayoutPrefs();
  }

  function loadAgentConnectionStates(): Record<string, AgentConnectionState> {
    if (typeof localStorage === "undefined") return {};

    try {
      const parsed = JSON.parse(localStorage.getItem(AGENT_STATUS_KEY) ?? "{}") as unknown;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};

      return Object.fromEntries(
        Object.entries(parsed).flatMap(([key, value]) => {
          if (!value || typeof value !== "object" || Array.isArray(value)) return [];
          const entry = value as Partial<AgentConnectionState>;
          return [[key, {
            connected: entry.connected === true,
            testedAt: Number(entry.testedAt) || 0,
            message: String(entry.message ?? ""),
          }]];
        }),
      );
    } catch {
      return {};
    }
  }

  function saveAgentConnectionStates(states: Record<string, AgentConnectionState>) {
    localStorage.setItem(AGENT_STATUS_KEY, JSON.stringify(states));
  }

  function rememberAgentConnection(key: string, state: AgentConnectionState) {
    const next = { ...agentConnectionStates, [key]: state };
    agentConnectionStates = next;
    saveAgentConnectionStates(next);
  }

  function rememberMcpConnection(serverId: string, state: McpConnectionState) {
    mcpConnectionStates = { ...mcpConnectionStates, [serverId]: state };
  }

  function rememberMcpTools(serverId: string, tools: string[]) {
    mcpToolsByServer = { ...mcpToolsByServer, [serverId]: tools };
  }

  function mcpConnectionState(serverId: string): McpConnectionState | null {
    return mcpConnectionStates[serverId] ?? null;
  }

  function mcpConnectionLabel(serverId: string): string {
    const state = mcpConnectionState(serverId);
    if (!state) return "Not tested";
    return state.status === "connected" ? "Connected" : "Disconnected";
  }

  function mcpConnectionDetail(serverId: string): string {
    const state = mcpConnectionState(serverId);
    if (!state) return "Test connection to verify this MCP.";
    if (state.status === "connected") {
      return `${state.toolCount} tool${state.toolCount === 1 ? "" : "s"} · ${relativeTime(state.testedAt)}`;
    }
    return state.message;
  }

  function dismissAppNotice() {
    if (appNoticeTimer) {
      clearTimeout(appNoticeTimer);
      appNoticeTimer = null;
    }
    appNotice = null;
  }

  function completedSortValue(card: CardProjection): number {
    return typeof card.completedAt === "number" ? card.completedAt : 0;
  }

  function visibleCardsForColumn(column: BoardProjection["columns"][number]): CardProjection[] {
    if (column.intent !== "done") {
      const limit = laneVisibleLimits[column.id] ?? DEFAULT_LANE_VISIBLE_LIMIT;
      return column.cards.slice(0, limit);
    }

    const cards = [...column.cards].sort((left, right) => completedSortValue(right) - completedSortValue(left));
    return doneVisibleLimit === "all" ? cards : cards.slice(0, doneVisibleLimit);
  }

  function showMoreLaneCards(column: BoardDisplayColumn) {
    laneVisibleLimits = {
      ...laneVisibleLimits,
      [column.id]: Math.min(column.totalCardCount, (laneVisibleLimits[column.id] ?? DEFAULT_LANE_VISIBLE_LIMIT) + LANE_VISIBLE_INCREMENT),
    };
  }

  function showAllLaneCards(column: BoardDisplayColumn) {
    laneVisibleLimits = {
      ...laneVisibleLimits,
      [column.id]: column.totalCardCount,
    };
  }

  function buildJiraConfig(): JiraMcpConfig | null {
    const server = settings.mcpServers.find((entry) => entry.id === settings.jira.serverId);

    const credential = credentialValue();

    if (!settings.jira.baseUrl.trim() || !credential) {
      syncError = "Open Settings and fill Jira URL plus the selected credential before syncing.";
      openSettings("jira");
      return null;
    }

    if (settings.jira.authMode !== "pat" && !settings.jira.username.trim()) {
      syncError = "Open Settings and fill the Jira email/username required by the selected auth method.";
      openSettings("jira");
      return null;
    }

    return {
      server: {
        command: server?.command ?? "",
        args: server?.args ?? [],
        env: {
          ...(server?.env ?? {}),
          JIRA_URL: settings.jira.baseUrl,
          JIRA_BASE_URL: settings.jira.baseUrl,
          ATLASSIAN_SITE_URL: settings.jira.baseUrl,
          JIRA_USERNAME: settings.jira.username,
          JIRA_EMAIL: settings.jira.username,
          ATLASSIAN_EMAIL: settings.jira.username,
          ...authEnv(),
        },
      },
      auth: {
        base_url: settings.jira.baseUrl,
        auth_mode: settings.jira.authMode,
        username: settings.jira.username,
        api_token: appSecrets.jira_api_token,
        personal_access_token: appSecrets.jira_personal_access_token,
        password: appSecrets.jira_password,
      },
      tool_name: settings.jira.toolName,
      board_tool_name: settings.jira.boardToolName,
      board_issues_tool_name: settings.jira.boardIssuesToolName,
      jql: settings.jira.jql,
      board_id: settings.jira.boardId || null,
      project_key: settings.jira.projectKey || null,
      board_name: settings.jira.boardNameFilter || null,
      page_size: settings.jira.pageSize,
      max_pages: settings.jira.maxPages,
    };
  }

  function buildAiWorkerConfig(): AiWorkerConfig | null {
    const effectiveSettings = settingsWithInstructionDrafts();
    const effectiveProvider = providerById(effectiveSettings.aiWorker.providerId);
    const effectiveModel = modelById(effectiveProvider, effectiveSettings.aiWorker.modelId);
    const effectiveApiKey = appSecrets.ai_api_keys[effectiveProvider.id] ?? "";

    if (effectiveSettings.aiWorker.runtime === "api" && !effectiveApiKey.trim()) {
      openSettings("agent");
      appNotice = { tone: "error", message: `Open Settings and fill ${effectiveProvider.apiKeyLabel}.` };
      return null;
    }

    if (effectiveSettings.aiWorker.runtime === "opencode" && !effectiveSettings.aiWorker.opencodeCommand.trim()) {
      openSettings("agent");
      appNotice = { tone: "error", message: "Open Settings and fill the OpenCode command." };
      return null;
    }

    return {
      runtime: effectiveSettings.aiWorker.runtime,
      provider_name: effectiveProvider.label,
      base_url: effectiveProvider.baseUrl,
      api_style: effectiveProvider.apiStyle,
      api_key: effectiveApiKey,
      model: effectiveSettings.aiWorker.runtime === "opencode" ? effectiveSettings.aiWorker.opencodeModel : effectiveModel.id,
      opencode_command: effectiveSettings.aiWorker.opencodeCommand,
      opencode_model: effectiveSettings.aiWorker.opencodeModel,
      opencode_workdir: effectiveSettings.aiWorker.opencodeWorkdir.trim() || null,
      opencode_auto_approve: effectiveSettings.aiWorker.opencodeAutoApprove,
      agent_rules: effectiveSettings.aiWorker.agentRules,
      agent_skills: effectiveSettings.aiWorker.agentSkills,
      temperature: effectiveSettings.aiWorker.temperature,
    };
  }

  function appendWorkspaceChat(message: Omit<WorkspaceChatMessage, "id">) {
    const entry: WorkspaceChatMessage = {
      ...message,
      id: `chat-${Date.now().toString(36)}-${workspaceChatMessages.length}`,
    };
    workspaceChatMessages = capList([...workspaceChatMessages, entry], MAX_WORKSPACE_CHAT_MESSAGES);
    workspaceChatSession = {
      ...workspaceChatSession,
      updatedAt: Date.now(),
      messages: workspaceChatMessages,
      title: isGenericChatTitle(workspaceChatSession.title) && message.role === "user"
        ? summarizeChatTitle(message.text)
        : workspaceChatSession.title,
    };
    syncWorkspaceChatSession();
    appendWorkspaceChatActivity({
      kind: "message",
      label: message.role,
      text: message.text,
    });
    saveUiState();
    scrollWorkspaceChatToLatest();
  }

  function appendWorkspaceChatActivity(activity: Omit<WorkspaceChatActivity, "id" | "at">) {
    const entry: WorkspaceChatActivity = {
      ...activity,
      id: `chat-activity-${Date.now().toString(36)}-${workspaceChatSession.activities.length}`,
      at: Date.now(),
    };
    workspaceChatSession = {
      ...workspaceChatSession,
      updatedAt: Date.now(),
      activities: capList([...workspaceChatSession.activities, entry], MAX_WORKSPACE_CHAT_ACTIVITIES),
    };
    syncWorkspaceChatSession();
    saveUiState();
  }

  function syncWorkspaceChatSession() {
    const normalizedSession = {
      ...workspaceChatSession,
      messages: workspaceChatMessages,
      activities: workspaceChatSession.activities,
    };
    workspaceChatSession = normalizedSession;
    workspaceChatActiveSessionId = normalizedSession.id;
    workspaceChatSessions = capList(
      [normalizedSession, ...workspaceChatSessions.filter((session) => session.id !== normalizedSession.id)],
      MAX_CHAT_SESSIONS,
    );
  }

  function activateWorkspaceChatSession(sessionId: string) {
    const session = workspaceChatSessions.find((entry) => entry.id === sessionId);
    if (!session || session.id === workspaceChatActiveSessionId) return;

    workspaceChatActiveSessionId = session.id;
    workspaceChatSession = {
      ...session,
      messages: session.messages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
      activities: session.activities.slice(-MAX_WORKSPACE_CHAT_ACTIVITIES),
      recentCardIds: session.recentCardIds.slice(0, MAX_WORKSPACE_CHAT_RECENT_CARDS),
    };
    workspaceChatMessages = workspaceChatSession.messages;
    saveUiState();
    scrollWorkspaceChatToLatest();
  }

  function startWorkspaceChatSession() {
    const sessionNumber = workspaceChatSessions.length + 1;
    const session = createWorkspaceChatSession([], `Chat ${sessionNumber}`);
    workspaceChatSessions = capList([session, ...workspaceChatSessions], MAX_CHAT_SESSIONS);
    workspaceChatActiveSessionId = session.id;
    workspaceChatSession = session;
    workspaceChatMessages = session.messages;
    saveUiState();
    scrollWorkspaceChatToLatest();
  }

  function setWorkspaceChatSessionCard(cardId: string | null, options: { created?: boolean } = {}) {
    const card = cardId ? activeCardById.get(cardId) ?? null : null;
    const recentCardIds = cardId
      ? capList([cardId, ...workspaceChatSession.recentCardIds.filter((entry) => entry !== cardId)], MAX_WORKSPACE_CHAT_RECENT_CARDS)
      : workspaceChatSession.recentCardIds;
    workspaceChatSession = {
      ...workspaceChatSession,
      updatedAt: Date.now(),
      title: card && (options.created || isGenericChatTitle(workspaceChatSession.title))
        ? chatTitleForCard(card)
        : workspaceChatSession.title,
      lastCardId: cardId ?? workspaceChatSession.lastCardId,
      lastCreatedCardId: options.created && cardId ? cardId : workspaceChatSession.lastCreatedCardId,
      recentCardIds,
    };
    syncWorkspaceChatSession();
    saveUiState();
  }

  function workspaceChatActionContext(): WorkspaceChatActionContext {
    return {
      activeCardIds,
      selectedCardId,
      lastCardId: workspaceChatSession.lastCardId,
      lastCreatedCardId: workspaceChatSession.lastCreatedCardId,
      recentCardIds: workspaceChatSession.recentCardIds,
    };
  }

  function workspaceChatSessionPromptContext(): string {
    const recentMessages = workspaceChatMessages
      .filter((message) => message.id !== "chat-welcome")
      .slice(-10)
      .map((message) => `${message.role}: ${singleLine(message.text, 500)}`);
    const recentActivities = workspaceChatSession.activities
      .slice(-8)
      .map((activity) => `${activity.kind}/${activity.label}: ${singleLine(activity.text, 300)}`);

    return [
      chatSessionContext(workspaceChatActionContext()),
      recentMessages.length > 0 ? ["Recent chat turns:", ...recentMessages].join("\n") : "Recent chat turns: none.",
      recentActivities.length > 0 ? ["Recent tool activity:", ...recentActivities].join("\n") : "Recent tool activity: none.",
    ].join("\n\n");
  }

  function scrollWorkspaceChatToLatest() {
    void tick().then(() => {
      workspaceChatEnd?.scrollIntoView({ block: "end" });
    });
  }

  function focusWorkspaceChatInput() {
    void tick().then(() => workspaceChatTextarea?.focus());
  }

  function handleWorkspaceChatKeydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;

    event.preventDefault();
    void sendWorkspaceChat();
  }

  async function initWorkspaceTerminal() {
    await tick();
    if (!workspaceTerminalContainer || workspaceTerminalOpened) {
      workspaceTerminal?.focus();
      return;
    }

    const { Terminal, FitAddon } = await loadWorkspaceTerminalRuntime();
    if (!workspaceTerminalContainer || workspaceTerminalOpened) {
      workspaceTerminal?.focus();
      return;
    }

    workspaceTerminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      scrollback: 3_000,
      fontFamily: "'SF Mono', 'Fira Code', 'Menlo', monospace",
      fontSize: 13,
      theme: {
        background: "#09090d",
        foreground: "#d7d0e2",
        cursor: "#b8d6e4",
        black: "#111016",
        red: "#f0b0aa",
        green: "#b9d6aa",
        yellow: "#e7d38f",
        blue: "#b8d6e4",
        magenta: "#d0b8e8",
        cyan: "#a8dce8",
        white: "#f1edf5",
      },
    });
    workspaceFitAddon = new FitAddon();
    workspaceTerminal.loadAddon(workspaceFitAddon);
    workspaceTerminal.open(workspaceTerminalContainer);
    workspaceFitAddon.fit();
    workspaceTerminal.focus();
    workspaceTerminalOpened = true;

    workspaceTerminal.onData((data) => {
      const bytes = Array.from(new TextEncoder().encode(data));
      writePtyTerminal(workspaceTerminalId, bytes).catch(() => {});
    });

    workspaceTerminalResizeObserver = new ResizeObserver(() => {
      if (!workspaceTerminal || !workspaceFitAddon || !workspaceTerminalContainer?.offsetHeight) return;
      workspaceFitAddon.fit();
      resizePtyTerminal(workspaceTerminalId, workspaceTerminal.rows, workspaceTerminal.cols).catch(() => {});
    });
    workspaceTerminalResizeObserver.observe(workspaceTerminalContainer);

    try {
      await openPtyTerminal(
        workspaceTerminalId,
        workspaceShellWorkdir.trim() || null,
        (data) => workspaceTerminal?.write(new Uint8Array(data)),
      );
      await resizePtyTerminal(workspaceTerminalId, workspaceTerminal.rows, workspaceTerminal.cols);
    } catch (reason) {
      workspaceTerminal.writeln(`\r\n\x1b[31mFailed to open terminal: ${reason}\x1b[0m`);
    }
  }

  function openTermWorkspace() {
    void loadWorkspaceTerminalRuntime();
    setWorkspaceMode("term");
  }

  function loadWorkspaceTerminalRuntime() {
    workspaceTerminalRuntime ??= (async () => {
      await import("@xterm/xterm/css/xterm.css");
      const [terminal, fit] = await Promise.all([
      import("@xterm/xterm"),
      import("@xterm/addon-fit"),
      ]);

      return {
        Terminal: terminal.Terminal,
        FitAddon: fit.FitAddon,
      };
    })();

    return workspaceTerminalRuntime;
  }

  async function openDialogIfAvailable(options: Parameters<typeof import("@tauri-apps/plugin-dialog").open>[0]) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    return open(options);
  }

  function loadEditorWorkspaceRuntime() {
    editorWorkspaceRuntime ??= import("$lib/components/EditorWorkspace.svelte").then((module) => {
      editorWorkspaceModule = module;
      return module;
    });

    return editorWorkspaceRuntime;
  }

  function loadFileBrowserRuntime() {
    fileBrowserRuntime ??= import("$lib/components/FileBrowserPane.svelte").then((module) => {
      fileBrowserModule = module;
      return module;
    });

    return fileBrowserRuntime;
  }

  function loadGitActionsRuntime() {
    gitActionsRuntime ??= import("$lib/components/GitActionsPane.svelte").then((module) => {
      gitActionsModule = module;
      return module;
    });

    return gitActionsRuntime;
  }

  function loadWorkspaceChatRuntime() {
    workspaceChatRuntime ??= import("$lib/components/WorkspaceChatPane.svelte").then((module) => {
      workspaceChatModule = module;
      return module;
    });

    return workspaceChatRuntime;
  }

  function loadMcpConnectionRuntime() {
    mcpConnectionRuntime ??= import("$lib/components/McpConnectionSettings.svelte").then((module) => {
      mcpConnectionModule = module;
      return module;
    });

    return mcpConnectionRuntime;
  }

  function loadAgentConsoleRuntime() {
    agentConsoleRuntime ??= import("$lib/components/AgentConsolePanel.svelte").then((module) => {
      agentConsoleModule = module;
      return module;
    });

    return agentConsoleRuntime;
  }

  async function sendWorkspaceChat() {
    const message = workspaceChatTextarea?.value.trim() ?? "";
    if (!message || workspaceChatRunning) return;

    if (workspaceChatTextarea) workspaceChatTextarea.value = "";
    appendWorkspaceChat({ role: "user", text: message });
    focusWorkspaceChatInput();

    const localActions = fastWorkspaceChatActions(message, workspaceChatActionContext());
    if (localActions.length > 0) {
      const actionSummary = await applyWorkspaceChatActions(localActions);
      appendWorkspaceChat({ role: "system", text: actionSummary });
      focusWorkspaceChatInput();
      return;
    }

    const config = buildAiWorkerConfig();
    if (!config) return;

    workspaceChatRunning = true;

    try {
      const result = await chatAiWorker(config, {
        message,
        terminal_context: workspaceAgentContext(activeBoard),
        session_context: workspaceChatSessionPromptContext(),
      });
      const response = result.message;
      const actions = extractWorkspaceActions(response);
      appendWorkspaceChat({ role: "agent", text: stripWorkspaceActions(response) });
      if (actions.length > 0) {
        const actionSummary = await applyWorkspaceChatActions(actions);
        appendWorkspaceChat({ role: "system", text: actionSummary });
      }
    } catch (reason) {
      appendWorkspaceChat({ role: "system", text: reason instanceof Error ? reason.message : String(reason) });
    } finally {
      workspaceChatRunning = false;
      focusWorkspaceChatInput();
    }
  }

  async function applyWorkspaceChatActions(actions: WorkspaceChatAction[]): Promise<string> {
    const results: string[] = [];

    for (const action of actions.slice(0, 5)) {
      if (action.type === "create_task") {
        const card = createBoardTask(action.title, action.description ?? "Created by Spacesly Agent chat.");
        if (card) {
          setWorkspaceChatSessionCard(card.id, { created: true });
          appendWorkspaceChatActivity({
            kind: "tool",
            label: "create_task",
            text: `Created ${ticketLabel(card)}: ${card.title}`,
            cardId: card.id,
          });
          results.push(`Created local task "${card.title}" in Todo. Queue it or start the Agent when ready.`);
        } else {
          results.push(`Could not create task "${action.title}".`);
        }
        continue;
      }

      if (action.type === "sync_jira") {
        await syncJira();
        appendWorkspaceChatActivity({ kind: "tool", label: "sync_jira", text: "Requested Jira sync from chat." });
        results.push("Jira sync requested. Watch the board notice for fetched card count or errors.");
        continue;
      }

      const card = resolveActionCard(action);
      if (!card) {
        results.push(`Could not find the requested card for ${action.type}.`);
        continue;
      }

      if (action.type === "select_card") {
        selectCard(card);
        setWorkspaceChatSessionCard(card.id);
        setWorkspaceMode("board");
        appendWorkspaceChatActivity({ kind: "tool", label: "select_card", text: `Opened ${ticketLabel(card)}: ${card.title}`, cardId: card.id });
        results.push(`Opened ${ticketLabel(card)}: "${card.title}". Current state: ${executionDetail(card.execution)}.`);
        continue;
      }

      if (action.type === "delete_card") {
        const removed = removeCard(card.id);
        appendWorkspaceChatActivity({ kind: "tool", label: "delete_card", text: `${removed ? "Removed" : "Failed to remove"} ${ticketLabel(card)}: ${card.title}`, cardId: card.id });
        results.push(removed ? `Removed ${ticketLabel(card)} from Spacesly.` : `Could not remove ${ticketLabel(card)}.`);
        continue;
      }

      if (action.type === "start_agent") {
        setWorkspaceChatSessionCard(card.id);
        appendWorkspaceChatActivity({ kind: "tool", label: "start_agent", text: `Requested Agent start for ${ticketLabel(card)}: ${card.title}`, cardId: card.id });
        await startWorkerForCard(card.id);
        results.push(`Agent start requested for ${ticketLabel(card)}: "${card.title}". Open Agent Console from the board toolbar when you need run details.`);
        continue;
      }

      if (action.type === "move_card") {
        const columnId = columnIdForChatTarget(action.target);
        if (!columnId) {
          results.push(`Could not find ${action.target} column.`);
          continue;
        }
        await moveCardAndSync(card.id, columnId);
        setWorkspaceChatSessionCard(card.id);
        appendWorkspaceChatActivity({ kind: "tool", label: "move_card", text: `Moved ${ticketLabel(card)} to ${chatTargetLabel(action.target)}.`, cardId: card.id });
        results.push(`Moved ${ticketLabel(card)} to ${chatTargetLabel(action.target)}. Jira write-back is attempted only for In Progress and Done.`);
      }
    }

    return results.length > 0 ? `Command result: ${results.join(" ")}` : "Command result: no board actions were applied.";
  }

  function columnIdForChatTarget(target: "todo" | "queued" | "in_progress" | "done"): string | null {
    const intent: ColumnIntent = target === "todo" ? "backlog" : target;
    return activeColumnByIntent.get(intent)?.id ?? null;
  }

  function credentialValue(): string {
    if (settings.jira.authMode === "pat") return appSecrets.jira_personal_access_token.trim();
    if (settings.jira.authMode === "password") return appSecrets.jira_password.trim();
    return appSecrets.jira_api_token.trim();
  }

  function authEnv(): Record<string, string> {
    if (settings.jira.authMode === "pat") {
      return {
        JIRA_PAT: appSecrets.jira_personal_access_token,
        JIRA_PERSONAL_ACCESS_TOKEN: appSecrets.jira_personal_access_token,
        ATLASSIAN_PAT: appSecrets.jira_personal_access_token,
        ATLASSIAN_PERSONAL_ACCESS_TOKEN: appSecrets.jira_personal_access_token,
      };
    }

    if (settings.jira.authMode === "password") {
      return {
        JIRA_PASSWORD: appSecrets.jira_password,
        ATLASSIAN_PASSWORD: appSecrets.jira_password,
      };
    }

    return {
      JIRA_API_TOKEN: appSecrets.jira_api_token,
      ATLASSIAN_API_TOKEN: appSecrets.jira_api_token,
    };
  }

  function applyConfiguredBoardName(projection: WorkspaceProjection): WorkspaceProjection {
    const name = settings.jira.boardName.trim();
    if (!name) return projection;

    return {
      ...projection,
      projects: projection.projects.map((project, projectIndex) =>
        projectIndex === 0
          ? {
              ...project,
              boards: project.boards.map((board, boardIndex) =>
                boardIndex === 0 ? { ...board, name } : board,
              ),
            }
          : project,
      ),
    };
  }

  async function syncJira() {
    const config = buildJiraConfig();
    if (!config) return;

    if (!config.board_id && settings.jira.boards[0]) {
      config.board_id = settings.jira.boards[0].id;
      settings = {
        ...settings,
        jira: {
          ...settings.jira,
          boardId: settings.jira.boards[0].id,
          boardName: settings.jira.boards[0].name,
        },
      };
    }

    if (!config.board_id) {
      appNotice = { tone: "error", message: "Choose a Jira board first. Open Settings and click Connect Jira." };
      openSettings("jira");
      return;
    }

    syncing = true;
    syncError = null;
      appNotice = { tone: "info", message: "Refreshing from Jira. Saved cards stay visible if Jira is slow." };
    await tick();

    try {
      const projection = mergeSyncedWorkspace(
        applyConfiguredBoardName(await syncJiraWorkspace(config)),
        activeBoard?.columns,
        LEGACY_SEED_CARD_ID,
        SYNC_RETAIN_MISSING_CARD_MS,
      );
      workspace = projection;
      cacheSavedAt = Date.now();
      saveCachedWorkspace(projection);
      const syncedCards = projection.projects[0]?.boards[0]?.columns.reduce(
        (count, column) => count + column.cards.reduce((total, card) => total + (card.source !== "local" ? 1 : 0), 0),
        0,
      ) ?? 0;
      appNotice = {
        tone: syncedCards > 0 ? "success" : "info",
        message: syncedCards > 0
          ? `Synced ${syncedCards} Jira card${syncedCards === 1 ? "" : "s"} from up to ${settings.jira.maxPages} page${settings.jira.maxPages === 1 ? "" : "s"}.`
          : "Sync finished, but Jira returned no cards for this board/query.",
      };
    } catch (reason) {
      syncError = reason instanceof Error ? reason.message : String(reason);
      appNotice = {
        tone: "error",
        message: cacheSavedAt ? `${syncError} Saved cards are still available.` : syncError,
      };
    } finally {
      syncing = false;
    }
  }

  async function testJiraConnection() {
    if (selectedServer?.kind !== "jira") {
      await testSelectedMcpConnection();
      return;
    }

    const config = buildJiraConfig();
    if (!config) return;
    const serverId = selectedServer.id;

    testingConnection = true;
    connectionMessage = null;
    settingsError = null;

    try {
      const status = await testJiraMcpConnection(config);
      rememberMcpTools(serverId, status.tools);
      connectionMessage = status.board_count > 0 || status.issue_count > 0
        ? `Jira connected. Found ${status.board_count} board${status.board_count === 1 ? "" : "s"} and ${status.issue_count} sample ticket${status.issue_count === 1 ? "" : "s"}.`
        : `MCP connected with ${status.tool_count} tools, but Jira returned no boards or tickets yet. Try Connect Jira, a project key, or a board name.`;
      rememberMcpConnection(serverId, {
        status: "connected",
        testedAt: Date.now(),
        message: connectionMessage,
        toolCount: status.tool_count,
      });
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      settingsError = message;
      rememberMcpTools(serverId, []);
      rememberMcpConnection(serverId, {
        status: "disconnected",
        testedAt: Date.now(),
        message,
        toolCount: 0,
      });
    } finally {
      testingConnection = false;
    }
  }

  async function testSelectedMcpConnection() {
    if (!selectedServer) return;
    const serverId = selectedServer.id;

    const serverConfig = selectedServer.kind === "jira"
      ? buildJiraConfig()?.server
      : {
          command: selectedServer.command,
          args: selectedServer.args,
          env: selectedServer.env,
        };
    if (!serverConfig) return;

    testingConnection = true;
    connectionMessage = null;
    settingsError = null;

    try {
      const status = await testMcpServerConnection(serverConfig);
      rememberMcpTools(serverId, status.tools);
      connectionMessage = `Connected. ${status.tool_count} MCP tool${status.tool_count === 1 ? "" : "s"} available.`;
      rememberMcpConnection(serverId, {
        status: "connected",
        testedAt: Date.now(),
        message: connectionMessage,
        toolCount: status.tool_count,
      });
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      settingsError = message;
      rememberMcpTools(serverId, []);
      rememberMcpConnection(serverId, {
        status: "disconnected",
        testedAt: Date.now(),
        message,
        toolCount: 0,
      });
    } finally {
      testingConnection = false;
    }
  }

  async function loadJiraBoards() {
    const config = buildJiraConfig();
    if (!config) return;

    loadingBoards = true;
    connectionMessage = null;
    settingsError = null;
    appNotice = { tone: "info", message: "Loading Jira boards..." };

    try {
      const boards: JiraBoard[] = await getJiraBoards(config);
      const selectedBoard = boards.find((board) => board.id === settings.jira.boardId) ?? boards[0];
      settings = {
        ...settings,
        jira: {
          ...settings.jira,
          boards,
          boardId: selectedBoard?.id ?? "",
          boardName: selectedBoard?.name ?? settings.jira.boardName,
        },
      };
      if (boards.length === 0) {
        connectionMessage = "Connected, but Jira returned no boards for this account/filter.";
        appNotice = { tone: "error", message: connectionMessage };
      } else {
        connectionMessage = `Loaded ${boards.length} Jira board${boards.length === 1 ? "" : "s"}.`;
        appNotice = { tone: "success", message: connectionMessage };
      }
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: settingsError };
    } finally {
      loadingBoards = false;
    }
  }

  async function connectJira() {
    const config = buildJiraConfig();
    if (!config) return;

    connectingJira = true;
    settingsError = null;
    connectionMessage = null;
    appNotice = { tone: "info", message: "Connecting to Jira and loading boards..." };

    try {
      const boards = await getJiraBoards(config);
      const selectedBoard = boards.find((board) => board.id === settings.jira.boardId) ?? boards[0];
      if (!selectedBoard) {
        connectionMessage = "Connected to Jira, but no boards were returned for this account/filter.";
        appNotice = { tone: "error", message: connectionMessage };
        return;
      }
      const nextSettings = {
        ...settings,
        jira: {
          ...settings.jira,
          boards,
          boardId: selectedBoard?.id ?? "",
          boardName: selectedBoard?.name ?? settings.jira.boardName,
        },
      };
      settings = nextSettings;
      await persistSettingsAndSecrets(nextSettings);
      connectionMessage = `Jira connected. Selected ${selectedBoard.name}. Click Sync Jira board.`;
      appNotice = { tone: "success", message: connectionMessage };
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: settingsError };
    } finally {
      connectingJira = false;
    }
  }

  async function testWorkerConnection() {
    const config = buildAiWorkerConfig();
    if (!config) return;
    const statusKey = selectedAgentStatusKey;

    testingWorker = true;
    settingsError = null;
    connectionMessage = null;
    appNotice = { tone: "info", message: "Testing Agent model..." };

    try {
      const status = await testAiWorker(config);
      workerStatus = status;
      connectionMessage = `${selectedAgentLabel} connected.`;
      rememberAgentConnection(statusKey, {
        connected: true,
        testedAt: Date.now(),
        message: status.message,
      });
      appNotice = { tone: "success", message: connectionMessage };
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      workerStatus = null;
      rememberAgentConnection(statusKey, {
        connected: false,
        testedAt: Date.now(),
        message,
      });
      settingsError = message;
      appNotice = { tone: "error", message };
    } finally {
      testingWorker = false;
    }
  }

  function addMcpServer() {
    const server = createMcpServer();
    settings = {
      ...settings,
      mcpServers: [...settings.mcpServers, server],
      jira: { ...settings.jira, serverId: server.id },
    };
    selectedServerId = server.id;
  }

  function removeSelectedServer() {
    if (settings.mcpServers.length <= 1 || !selectedServer) return;

    const remaining = settings.mcpServers.filter((server) => server.id !== selectedServer.id);
    settings = {
      ...settings,
      mcpServers: remaining,
      jira: { ...settings.jira, serverId: remaining[0].id },
    };
    selectedServerId = remaining[0].id;
  }

  function updateSelectedServer(values: Partial<{ name: string; kind: AppSettings["mcpServers"][number]["kind"]; command: string; args: string[]; env: Record<string, string> }>) {
    if (!selectedServer) return;

    settings = {
      ...settings,
      mcpServers: settings.mcpServers.map((server) =>
        server.id === selectedServer.id ? { ...server, ...values } : server,
      ),
    };
  }

  async function persistSettings() {
    const nextSettings = settingsWithInstructionDrafts();
    settings = nextSettings;

    if (selectedServer?.kind === "jira" && !nextSettings.jira.toolName.trim()) {
      settingsError = "Jira tool name is required.";
      return;
    }

    try {
      await persistSettingsAndSecrets(nextSettings);
    } catch (reason: unknown) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
      return;
    }
    closeSettings();
    settingsError = null;
    syncError = null;
  }

  function applyJqlPreset(preset: "assigned" | "unassigned_todo" | "unresolved") {
    const projectClause = settings.jira.projectKey.trim()
      ? `project = ${settings.jira.projectKey.trim()} AND `
      : "";
    const jql = {
      assigned: `${projectClause}assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC`,
      unassigned_todo: `${projectClause}assignee is EMPTY AND statusCategory = "To Do" ORDER BY updated DESC`,
      unresolved: `${projectClause}resolution = Unresolved ORDER BY updated DESC`,
    }[preset];

    settings = {
      ...settings,
      jira: { ...settings.jira, jql },
    };
  }

  function settingsWithInstructionDrafts(): AppSettings {
    const agentRules = agentRulesTextarea?.value;
    const agentSkills = agentSkillsTextarea?.value;

    if (agentRules === undefined && agentSkills === undefined) return settings;

    return {
      ...settings,
      aiWorker: {
        ...settings.aiWorker,
        agentRules: agentRules ?? settings.aiWorker.agentRules,
        agentSkills: agentSkills ?? settings.aiWorker.agentSkills,
      },
    };
  }

  function commitInstructionDrafts() {
    settings = settingsWithInstructionDrafts();
  }

  function commitAgentRulesDraft() {
    if (!agentRulesTextarea) return;
    settings = {
      ...settings,
      aiWorker: { ...settings.aiWorker, agentRules: agentRulesTextarea.value },
    };
  }

  function commitAgentSkillsDraft() {
    if (!agentSkillsTextarea) return;
    settings = {
      ...settings,
      aiWorker: { ...settings.aiWorker, agentSkills: agentSkillsTextarea.value },
    };
  }

  function updateActiveBoard(
    transform: (board: BoardProjection) => BoardProjection,
  ): boolean {
    if (!workspace) return false;

    const [project, ...otherProjects] = workspace.projects;
    const [board, ...otherBoards] = project?.boards ?? [];
    if (!project || !board) return false;

    const nextBoard = transform(board);
    workspace = {
      ...workspace,
      projects: [
        {
          ...project,
          boards: [nextBoard, ...otherBoards],
        },
        ...otherProjects,
      ],
    };
    return true;
  }

  function moveCard(cardId: string, targetColumnId: string, execution?: ExecutionState) {
    const movedCard = activeCardById.get(cardId);
    if (!movedCard) return;

    const targetColumn = activeColumnById.get(targetColumnId);
    if (!targetColumn) return;

    const targetIntent = targetColumn.intent;
    const cardForTarget = {
      ...(targetIntent ? withCompletionMetadata(movedCard, targetIntent) : movedCard),
      ...(execution !== undefined ? { execution } : {}),
    };

    if (!updateActiveBoard((board) => ({
      ...board,
      columns: board.columns.map((column) => {
        const cards = column.cards.filter((card) => card.id !== cardId);
        return column.id === targetColumnId
          ? { ...column, cards: [...cards, cardForTarget] }
          : { ...column, cards };
      }),
    }))) return;

    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
    appNotice = { tone: "info", message: "Card moved in Spacesly." };
  }

  function executionForColumn(columnId: string): ExecutionState | null {
    const intent = activeColumnById.get(columnId)?.intent;
    if (intent === "backlog") return "idle";
    if (intent === "queued") return "queued";
    if (intent === "in_progress") return "running";
    return null;
  }

  function removeCard(cardId: string): boolean {
    const card = activeCardById.get(cardId);
    if (!card || card.execution === "running") {
      appNotice = { tone: "error", message: "Running cards cannot be removed." };
      return false;
    }

    if (!updateActiveBoard((board) => ({
      ...board,
      columns: board.columns.map((column) => ({
        ...column,
        cards: column.cards.filter((entry) => entry.id !== cardId),
      })),
    }))) return false;

    if (selectedCardId === cardId) selectedCardId = null;
    if (agentRunCardId === cardId) agentRunCardId = null;
    const { [cardId]: _removed, ...remainingSessions } = agentRunSessions;
    agentRunSessions = remainingSessions;
    if (agentConsoleCardId === cardId) {
      const fallback = Object.values(remainingSessions)[0] ?? null;
      agentConsoleCardId = fallback?.cardId ?? null;
      if (!fallback) agentConsoleOpen = false;
    }
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
    appNotice = {
      tone: "success",
      message: card.source === "local"
        ? `Removed ${card.title}.`
        : `Removed ${ticketLabel(card)} from Spacesly. Jira issue was not deleted.`,
    };
    return true;
  }

  function createBoardTask(title: string, description: string): CardProjection | null {
    if (!workspace || !title) {
      appNotice = { tone: "error", message: "Add a task title before creating a task." };
      return null;
    }

    const card: CardProjection = {
      id: `local-${Date.now().toString(36)}`,
      title,
      source: "local",
      url: null,
      labels: ["local"],
      description: description.trim() || "Local Spacesly task for Agent execution.",
      assignee: null,
      priority: "medium",
      execution: "idle",
    };

    if (!updateActiveBoard((board) => ({
      ...board,
      columns: board.columns.map((column) =>
        column.intent === "backlog" ? { ...column, cards: [...column.cards, card] } : column,
      ),
    }))) return null;
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
    return card;
  }

  function createLocalTask() {
    const title = newTaskTitle.trim();
    const description = newTaskDescription.trim();
    const card = createBoardTask(title, description);
    if (!card) return;

    newTaskTitle = "";
    newTaskDescription = "";
    newTaskOpen = false;
    appNotice = { tone: "success", message: "Task created. Queue it or click Start when you want the Agent to run it." };
  }

  function beginAgentRun(card: CardProjection, continuation = false, gitSnapshot: AgentRunGitSnapshot | null = gitSnapshotFromInfo(workspaceGitInfo)) {
    const previousSession = agentRunSessions[card.id];
    agentRunCardId = card.id;
    agentRunTitle = card.title;
    agentRunStatus = "running";
    agentRunProgress = continuation ? Math.max(previousSession?.progress ?? 0, 20) : 5;
    agentRunOutput = continuation && previousSession?.output ? previousSession.output : "Waiting for Agent output...";
    agentRunResult = continuation && previousSession?.result ? previousSession.result : null;
    agentRunLogs = continuation && previousSession ? previousSession.logs : [];
    agentRunGitSnapshot = continuation && previousSession ? previousSession.gitSnapshot : gitSnapshot;
    agentRunTranscript = continuation && previousSession
      ? (previousSession.transcript ?? [])
      : [createAgentSessionEvent("system", "Agent execution session opened. Use the input below for approvals, constraints, or operator notes.")];
    agentTerminalLines = continuation && previousSession
      ? previousSession.terminalLines
      : [
          {
            id: `term-${Date.now().toString(36)}`,
            prompt: "system",
            text: "Agent execution session opened. Use the input below for approvals, constraints, or operator notes.",
          },
        ];
    appendStructuredAgentLog(
      "info",
      continuation ? "continue" : "start",
      continuation ? `Agent continuation started for ${ticketLabel(card)}.` : `Agent run started for ${ticketLabel(card)}.`,
      [
        `Work item: ${ticketLabel(card)} · ${card.title}`,
        `Continuation: ${continuation ? "yes" : "no"}`,
      ],
      [
        `Execution state: ${executionDetail(card.execution)}`,
        `Terminal session opened for approvals, constraints, and operator notes.`,
      ],
      ["Review the context export, then continue with the run."],
    );
  }

  function gitSnapshotFromInfo(info: GitWorkspaceInfo | null): AgentRunGitSnapshot | null {
    if (!info?.is_git_repo) return null;

    return {
      repo_root: info.repo_root,
      current_branch: info.current_branch,
      head_commit: info.head_commit,
    };
  }

  function activeAgentSession(): AgentRunSession | null {
    if (!agentRunCardId) return null;

    return createAgentRunSession(
      agentRunCardId,
      agentRunTitle,
      agentRunStatus,
      agentRunProgress,
      agentRunOutput,
      agentRunResult,
      agentRunLogs,
      agentTerminalLines,
      agentRunGitSnapshot,
      agentRunTranscript,
    );
  }

  function applyAgentSessionToConsole(session: AgentRunSession) {
    agentRunCardId = session.cardId;
    agentRunTitle = session.title;
    agentRunStatus = session.status;
    agentRunProgress = session.progress;
    agentRunOutput = session.output;
    agentRunResult = session.result;
    agentRunLogs = session.logs;
    agentTerminalLines = session.terminalLines;
    agentRunGitSnapshot = session.gitSnapshot;
    agentRunTranscript = session.transcript ?? [];
  }

  function agentSessionForCard(cardId: string): AgentRunSession | null {
    return agentRunCardId === cardId
      ? activeAgentSession() ?? agentRunSessions[cardId] ?? null
      : agentRunSessions[cardId] ?? null;
  }

  function updateAgentSessionForCard(cardId: string, transform: (session: AgentRunSession) => AgentRunSession) {
    const session = agentSessionForCard(cardId);
    if (!session) return;

    const nextSession = transform(session);
    agentRunSessions = {
      ...agentRunSessions,
      [cardId]: nextSession,
    };
    if (agentRunCardId === cardId) applyAgentSessionToConsole(nextSession);
  }

  function appendAgentSessionTranscript(type: AgentSessionEvent["type"], text: string) {
    agentRunTranscript = appendAgentSessionEvent(
      agentRunTranscript,
      createAgentSessionEvent(type, text),
      MAX_AGENT_SESSION_EVENTS,
    );
    persistActiveAgentRun();
  }

  function appendAgentSessionTranscriptForCard(cardId: string, type: AgentSessionEvent["type"], text: string) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      transcript: appendAgentSessionEvent(
        session.transcript ?? [],
        createAgentSessionEvent(type, text),
        MAX_AGENT_SESSION_EVENTS,
      ),
    }));
  }

  function persistActiveAgentRun() {
    const session = activeAgentSession();
    if (!session) return;

    agentRunSessions = {
      ...agentRunSessions,
      [session.cardId]: session,
    };
    agentConsoleCardId ??= session.cardId;
  }

  function openAgentRunForCard(card: CardProjection) {
    const session = agentRunSessions[card.id];
    if (!session) {
      appNotice = { tone: "info", message: "This card does not have an Agent terminal session yet." };
      return;
    }

    agentConsoleOpen = true;
    agentConsoleCardId = session.cardId;
    applyAgentSessionToConsole(session);
    agentTerminalInput = "";
  }

  function openAgentConsole(card?: CardProjection | null) {
    if (card && agentRunSessions[card.id]) {
      openAgentRunForCard(card);
      return;
    }

    const session = visibleAgentSession ?? latestAgentSession;
    if (session) {
      agentConsoleOpen = true;
      agentConsoleCardId = session.cardId;
      applyAgentSessionToConsole(session);
      return;
    }

    appNotice = { tone: "info", message: "No Agent console session is available yet." };
  }

  function selectCard(card: CardProjection) {
    selectedCardId = card.id;
  }

  function setAgentRunStatus(status: AgentRunStatus) {
    agentRunStatus = status;
    persistActiveAgentRun();
  }

  function setAgentRunStatusForCard(cardId: string, status: AgentRunStatus) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, status }));
  }

  function setAgentRunOutput(output: string) {
    agentRunOutput = capText(output, MAX_AGENT_OUTPUT_CHARS);
    persistActiveAgentRun();
  }

  function setAgentRunOutputForCard(cardId: string, output: string) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, output: capText(output, MAX_AGENT_OUTPUT_CHARS) }));
  }

  function setAgentRunResultForCard(cardId: string, result: AiWorkerTaskResult | null) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, result }));
  }

  function setAgentRunGitSnapshot(snapshot: AgentRunGitSnapshot | null) {
    agentRunGitSnapshot = snapshot;
    persistActiveAgentRun();
  }

  function setAgentRunGitSnapshotForCard(cardId: string, snapshot: AgentRunGitSnapshot | null) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, gitSnapshot: snapshot }));
  }

  function setAgentProgress(value: number) {
    agentRunProgress = Math.max(agentRunProgress, Math.min(100, value));
    persistActiveAgentRun();
  }

  function setAgentProgressForCard(cardId: string, value: number) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      progress: Math.max(session.progress, Math.min(100, value)),
    }));
  }

  function appendAgentLog(tone: AgentRunLog["tone"], label: string, message: string) {
    agentRunLogs = capList([
      ...agentRunLogs,
      {
        id: `run-${Date.now().toString(36)}-${agentRunLogs.length}`,
        at: new Date().toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }),
        tone,
        label,
        message,
      },
    ], MAX_AGENT_LOGS);
    persistActiveAgentRun();
  }

  function appendAgentLogForCard(cardId: string, tone: AgentRunLog["tone"], label: string, message: string) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      logs: capList([
        ...session.logs,
        {
          id: `run-${Date.now().toString(36)}-${session.logs.length}`,
          at: new Date().toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }),
          tone,
          label,
          message,
        },
      ], MAX_AGENT_LOGS),
    }));
  }

  function appendStructuredAgentLog(
    tone: AgentRunLog["tone"],
    label: string,
    summary: string,
    evidence: string[],
    details: string[],
    next: string[],
  ) {
    appendAgentLog(
      tone,
      label,
      [
        `STATUS: ${tone === "success" ? "Complete" : tone === "error" ? "Blocked" : "Running"}`,
        `SUMMARY: ${summary}`,
        "EVIDENCE:",
        ...evidence.map((line) => `- ${line}`),
        "DETAILS:",
        ...details.map((line) => `- ${line}`),
        ...(next.length > 0 ? ["NEXT:", ...next.map((line) => `- ${line}`)] : []),
      ].join("\n"),
    );
  }

  function appendStructuredAgentLogForCard(
    cardId: string,
    tone: AgentRunLog["tone"],
    label: string,
    summary: string,
    evidence: string[],
    details: string[],
    next: string[],
  ) {
    appendAgentLogForCard(
      cardId,
      tone,
      label,
      [
        `STATUS: ${tone === "success" ? "Complete" : tone === "error" ? "Blocked" : "Running"}`,
        `SUMMARY: ${summary}`,
        "EVIDENCE:",
        ...evidence.map((line) => `- ${line}`),
        "DETAILS:",
        ...details.map((line) => `- ${line}`),
        ...(next.length > 0 ? ["NEXT:", ...next.map((line) => `- ${line}`)] : []),
      ].join("\n"),
    );
  }

  function buildAgentContextExport(
    card: CardProjection,
    config: AiWorkerConfig,
    issueKey: string | null,
    operatorNotes: string | null,
    previousOutput: string | null,
  ): string {
    const runtimeLabel = config.runtime === "opencode"
      ? `OpenCode ${config.opencode_model}`
      : `${config.provider_name} ${config.model}`;
    const description = card.description.trim();
    const clippedDescription = description.length > 320 ? `${description.slice(0, 320)}…` : description;
    const transcript = previousOutput?.trim() ? previousOutput : null;
    const clippedPreviousOutput = transcript
      ? (transcript.length > 1200 ? `${transcript.slice(-1200)}…` : transcript)
      : "None";

    return [
      "STATUS: Running",
      `SUMMARY: ${ticketLabel(card)} is prepared for Agent execution.`,
      "EVIDENCE:",
      `- Work item: ${ticketLabel(card)} · ${card.title}`,
      `- Runtime: ${runtimeLabel}`,
      `- Jira link: ${issueKey ? `Issue ${issueKey}` : "Not linked"}`,
      `- Labels: ${card.labels.length > 0 ? card.labels.join(", ") : "None"}`,
      `- Task description: ${clippedDescription || "None"}`,
      "DETAILS:",
      `- Current board state: ${executionDetail(card.execution)}`,
      `- Operator notes: ${operatorNotes ? operatorNotes : "None"}`,
      `- Session transcript replay: ${clippedPreviousOutput}`,
      "- Verification target: return evidence before marking complete.",
      "NEXT:",
      "- Pass the exported context to the runtime and wait for evidence.",
    ].join("\n");
  }

  function appendTerminalLine(prompt: string, text: string) {
    agentTerminalLines = capList([
      ...agentTerminalLines,
      {
        id: `term-${Date.now().toString(36)}-${agentTerminalLines.length}`,
        prompt,
        text,
      },
    ], MAX_AGENT_TERMINAL_LINES);
    persistActiveAgentRun();
  }

  function appendTerminalLineForCard(cardId: string, prompt: string, text: string) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      terminalLines: capList([
        ...session.terminalLines,
        {
          id: `term-${Date.now().toString(36)}-${session.terminalLines.length}`,
          prompt,
          text,
        },
      ], MAX_AGENT_TERMINAL_LINES),
    }));
  }

  function submitAgentTerminalInput() {
    const input = agentTerminalInput.trim();
    if (!input) return;
    const cardId = agentConsoleCardId;
    if (!cardId || !agentRunSessions[cardId]) {
      appNotice = { tone: "error", message: "No Agent console session is selected." };
      return;
    }

    appendTerminalLineForCard(cardId, "operator", input);
    appendAgentSessionTranscriptForCard(cardId, isApprovalText(input) ? "approval" : "operator_note", input);
    appendStructuredAgentLogForCard(
      cardId,
      "info",
      "operator",
      "Operator note recorded for the running session.",
      [`Input: ${input}`],
      ["Operator note saved to the running session."],
      ["Continue the Agent with the updated guidance."],
    );
    if (agentSessionForCard(cardId)?.status === "blocked" && isApprovalText(input)) {
      appendStructuredAgentLogForCard(
        cardId,
        "success",
        "approval",
        "Operator approval recorded for this card session.",
        ["Approval text detected in operator input."],
        ["The blocked session can continue."],
        ["Continue the Agent to finish remaining work."],
      );
      appNotice = { tone: "info", message: "Approval recorded. Continue the Agent on this card when ready." };
    }
    agentTerminalInput = "";
  }

  function isApprovalText(value: string): boolean {
    const text = value.toLowerCase();
    return text.includes("approve") || text.includes("approved") || text.includes("approval granted");
  }

  function operatorNotesForCard(cardId: string): string | null {
    const session = agentRunCardId === cardId ? activeAgentSession() : agentRunSessions[cardId];
    const notes = session?.terminalLines
      .filter((line) => line.prompt === "operator")
      .map((line) => line.text.trim())
      .filter(Boolean)
      .join("\n")
      .trim();

    return notes || null;
  }

  function previousOutputForCard(cardId: string): string | null {
    const session = agentRunCardId === cardId ? activeAgentSession() : agentRunSessions[cardId];
    const transcript = agentSessionReplay(session?.transcript ?? [], MAX_AGENT_SESSION_REPLAY_CHARS);
    if (transcript) return transcript;

    const output = session?.output?.trim();
    return output && output !== "Waiting for Agent output..." && output !== "Agent is processing the task context..."
      ? output
      : null;
  }

  function resolveSessionCardId(): string | null {
    const candidates = [
      workspaceChatSession.lastCreatedCardId,
      workspaceChatSession.lastCardId,
      ...workspaceChatSession.recentCardIds,
      selectedCardId,
    ].filter((value): value is string => typeof value === "string" && value.length > 0);

    for (const cardId of candidates) {
      if (activeCardById.has(cardId)) return cardId;
    }

    return null;
  }

  function agentResultText(result: AiWorkerTaskResult): string {
    const sections = [
      `STATUS: ${result.completion_status === "completed" ? "COMPLETE" : "BLOCKED"}`,
      `SUMMARY: ${result.summary}`,
      "EVIDENCE:",
      ...(result.evidence.length > 0 ? result.evidence.map((line) => `- ${line}`) : ["- none"]),
      "DETAILS:",
      ...(result.details.length > 0 ? result.details.map((line) => `- ${line}`) : ["- none"]),
    ];

    if (result.next.length > 0) {
      sections.push("NEXT:", ...result.next.map((line) => `- ${line}`));
    }

    return sections.join("\n");
  }

  function agentJiraComment(result: AiWorkerTaskResult, config: AiWorkerConfig, gitInfo: GitWorkspaceInfo | null = null): string {
    const runtime = config.runtime === "opencode" ? "OpenCode" : config.provider_name;
    const model = config.runtime === "opencode" ? config.opencode_model : config.model;
    const evidence = result.evidence.join("\n") || result.details.join("\n") || result.summary;
    const gitEvidence = gitInfo?.head_commit
      ? [
          "",
          `Commit: ${gitInfo.head_commit}`,
          `Branch: ${gitInfo.current_branch ?? "unknown"}`,
          `Upstream: ${gitInfo.upstream_branch ?? "none"}`,
        ]
      : [];

    return [
      "Spacesly marked this task done.",
      "",
      `Agent: ${runtime} / ${model}`,
      "",
      `Summary: ${singleLine(result.summary, 500)}`,
      "",
      `Verification: ${singleLine(evidence, 900)}`,
      ...gitEvidence,
    ].join("\n");
  }

  function agentWritebackRequiresPushedCommit(card: CardProjection): boolean {
    const text = [
      card.title,
      card.description,
      card.labels.join(" "),
    ].join("\n").toLowerCase();

    const hasUpdateVerb = ["update", "change", "modify", "edit", "add", "remove", "set"].some((needle) => text.includes(needle));
    const hasEnvTarget = [
      ".env",
      "env variable",
      "environment variable",
      "environment config",
      "env config",
      "values.yaml",
      "values yml",
      "helm values",
      "deployment template",
      "deployment-config",
      "deployment config",
      "configmap",
      "secret.yaml",
      "secret yml",
    ].some((needle) => text.includes(needle));

    return hasUpdateVerb && hasEnvTarget;
  }

  async function verifyAgentJiraDoneGate(card: CardProjection, config: AiWorkerConfig): Promise<GitWorkspaceInfo | null> {
    if (!agentWritebackRequiresPushedCommit(card)) return null;

    const gitPath = config.opencode_workdir?.trim() || workspaceRoot || workspaceGitInfo?.repo_root || null;
    if (!gitPath) {
      throw new Error("Jira Done blocked: Spacesly cannot determine the git workdir for this Helm/env/template change.");
    }

    const info = config.opencode_workdir?.trim() ? await getPathGitInfo(gitPath) : await getWorkspaceGitInfo();
    workspaceGitInfo = info.is_git_repo ? info : null;
    selectedWorkspaceBranch = info.current_branch ?? "";

    if (!info.is_git_repo) {
      throw new Error("Jira Done blocked: Spacesly cannot verify a git repository for this Helm/env/template change.");
    }

    const runSnapshot = agentSessionForCard(card.id)?.gitSnapshot ?? null;
    if (!runSnapshot?.head_commit) {
      throw new Error("Jira Done blocked: Spacesly did not capture the starting commit for this Agent run.");
    }

    if (!info.head_commit) {
      throw new Error("Jira Done blocked: Spacesly cannot read the current git HEAD commit.");
    }

    if (info.head_commit === runSnapshot.head_commit) {
      throw new Error("Jira Done blocked: no new commit was created for this Helm/env/template change.");
    }

    if (info.dirty_worktree) {
      throw new Error("Jira Done blocked: the worktree still has uncommitted changes.");
    }

    if (!info.upstream_branch) {
      throw new Error("Jira Done blocked: the current branch has no upstream remote branch.");
    }

    if (info.ahead_count > 0) {
      throw new Error("Jira Done blocked: the latest commit has not been pushed to upstream.");
    }

    return info;
  }

  function singleLine(value: string, maxChars: number): string {
    const cleaned = value.replace(/```[\s\S]*?```/g, "").replace(/\s+/g, " ").trim();
    if (cleaned.length <= maxChars) return cleaned;
    return `${cleaned.slice(0, maxChars - 3).trim()}...`;
  }

  function updateCardExecution(cardId: string, execution: ExecutionState) {
    if (!workspace) return;
    const completedAt = typeof execution === "object" && "completed" in execution ? Date.now() : null;

    if (!updateActiveBoard((board) => ({
      ...board,
      columns: board.columns.map((column) => ({
        ...column,
        cards: column.cards.map((card) => card.id === cardId ? { ...card, execution, completedAt } : card),
      })),
    }))) return;
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
  }

  function resolveActionCard(action: { card_id?: string; ticket?: string; title?: string }): CardProjection | null {
    if (action.card_id && activeCardById.has(action.card_id)) return activeCardById.get(action.card_id) ?? null;

    const ticket = action.ticket?.toLowerCase();
    if (ticket) {
      const byTicket = activeCards.find((card) => ticketLabel(card).toLowerCase() === ticket);
      if (byTicket) return byTicket;
    }

    const title = action.title?.toLowerCase();
    if (title) {
      return activeCards.find((card) => card.title.toLowerCase().includes(title)) ?? null;
    }

    const sessionCardId = resolveSessionCardId();
    return sessionCardId ? activeCardById.get(sessionCardId) ?? null : null;
  }

  function columnIdByIntent(intent: "queued" | "in_progress" | "done"): string | null {
    return activeColumnByIntent.get(intent)?.id ?? null;
  }

  function cardColumnIntent(cardId: string): string | null {
    return cardColumnIntentById.get(cardId) ?? null;
  }

  function jiraKey(card: CardProjection): string | null {
    return card.source === "local" ? null : card.source.jira.key;
  }

  function jiraTargetStatus(columnId: string): string | null {
    const column = activeColumnById.get(columnId);
    if (!column) return null;

    if (column.intent === "in_progress") return "In Progress";
    if (column.intent === "done") return "Done";
    return null;
  }

  function shouldStartWorkerForColumn(columnId: string): boolean {
    const column = activeColumnById.get(columnId);
    return column?.intent === "in_progress";
  }

  function queueCard(cardId: string) {
    const queuedColumnId = columnIdByIntent("queued");
    if (!queuedColumnId || cardColumnIntent(cardId) === "queued") return;
    void moveCardAndSync(cardId, queuedColumnId);
  }

  function requestBacklogStartConfirmation(card: CardProjection): Promise<boolean> {
    if (cardColumnIntent(card.id) !== "backlog") return Promise.resolve(true);
    if (backlogStartConfirmation) return Promise.resolve(false);

    backlogStartConfirmation = { cardId: card.id, title: card.title };
    return new Promise<boolean>((resolve) => {
      backlogStartConfirmationResolve = resolve;
    });
  }

  function resolveBacklogStartConfirmation(confirmed: boolean) {
    backlogStartConfirmation = null;
    backlogStartConfirmationResolve?.(confirmed);
    backlogStartConfirmationResolve = null;
  }

  function requestManualDoneConfirmation(cardId: string) {
    const card = activeCardById.get(cardId);
    if (!card) return;
    if (agentSessionForCard(cardId)?.status !== "blocked") {
      appNotice = { tone: "error", message: "Manual Done is only available for blocked Agent sessions." };
      return;
    }

    manualDoneConfirmation = { cardId, title: card.title };
  }

  async function confirmManualDone(confirmed: boolean) {
    const target = manualDoneConfirmation;
    manualDoneConfirmation = null;
    if (!confirmed || !target) return;

    await markBlockedAgentDoneManually(target.cardId);
  }

  async function markBlockedAgentDoneManually(cardId: string) {
    const card = activeCardById.get(cardId);
    const doneColumnId = columnIdByIntent("done");
    if (!card || !doneColumnId) return;
    if (agentSessionForCard(cardId)?.status !== "blocked") {
      appNotice = { tone: "error", message: "Manual Done is only available for blocked Agent sessions." };
      return;
    }

    const summary = "Marked Done manually by operator after resolving the blocked Agent task.";
    moveCard(cardId, doneColumnId, { completed: { summary } });
    updateCardExecution(cardId, { completed: { summary } });
    setAgentRunStatusForCard(cardId, "completed");
    setAgentProgressForCard(cardId, 100);
    appendAgentSessionTranscriptForCard(cardId, "approval", summary);
    appendStructuredAgentLogForCard(
      cardId,
      "success",
      "manual-done",
      summary,
      ["Operator confirmed the task was resolved outside the Agent."],
      ["Spacesly moved the task to Done without rerunning the Agent."],
      ["Review Jira writeback status if the task is linked to Jira."],
    );

    const issueKey = jiraKey(card);
    const jiraConfig = issueKey ? buildJiraConfig() : null;
    if (issueKey && jiraConfig) {
      try {
        await addJiraComment(
          jiraConfig,
          issueKey,
          [
            "Spacesly marked this task Done manually.",
            "",
            "Reason: Operator confirmed the blocked Agent task was resolved outside the Agent.",
          ].join("\n"),
        );
        await transitionJiraIssue(jiraConfig, issueKey, "Done");
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : String(reason);
        appNotice = { tone: "error", message };
        return;
      }
    }

    appNotice = { tone: "success", message: `${ticketLabel(card)} marked Done manually.` };
  }

  async function moveCardAndSync(cardId: string, targetColumnId: string) {
    if (shouldStartWorkerForColumn(targetColumnId)) {
      await startWorkerForCard(cardId);
      return;
    }

    const card = activeCardById.get(cardId);
    const issueKey = card ? jiraKey(card) : null;
    const targetStatus = jiraTargetStatus(targetColumnId);

    const localExecution = executionForColumn(targetColumnId);
    moveCard(cardId, targetColumnId, localExecution ?? undefined);

    if (!issueKey || !targetStatus) return;

    const config = buildJiraConfig();
    if (!config) return;

    try {
      if (targetStatus === "In Progress") {
        await assignJiraIssue(config, issueKey);
      }
      await transitionJiraIssue(config, issueKey, targetStatus);
      appNotice = {
        tone: "success",
        message: targetStatus === "In Progress"
          ? `${issueKey} assigned to you and moved to ${targetStatus} in Jira.`
          : `${issueKey} moved to ${targetStatus} in Jira.`,
      };
    } catch (reason) {
      appNotice = {
        tone: "error",
        message: reason instanceof Error ? reason.message : String(reason),
      };
    }
  }

  async function startWorkerForCard(cardId: string) {
    if (runningWorkerCardIds[cardId]) {
      appNotice = { tone: "info", message: "Agent is already running this card." };
      return;
    }

    const card = activeCardById.get(cardId);
    if (!card || card.execution === "running") return;
    if (!(await requestBacklogStartConfirmation(card))) return;

    const config = buildAiWorkerConfig();
    if (!config) return;

    const issueKey = jiraKey(card);
    const inProgressColumnId = columnIdByIntent("in_progress");
    const doneColumnId = columnIdByIntent("done");
    if (!inProgressColumnId || !doneColumnId) return;
    const existingSession = agentRunSessions[cardId];
    const isContinuation = existingSession?.status === "blocked";
    const operatorNotes = operatorNotesForCard(cardId);
    const previousOutput = previousOutputForCard(cardId);
    runningWorkerCardIds = { ...runningWorkerCardIds, [cardId]: true };
    beginAgentRun(card, isContinuation, null);
    const runtimeLabel = config.runtime === "opencode" ? `OpenCode ${config.opencode_model}` : `${config.provider_name} ${config.model}`;
    appNotice = { tone: "info", message: `${isContinuation ? "Agent continuing" : "Agent started"} ${ticketLabel(card)} with ${runtimeLabel}.` };

    try {
      if (config.runtime === "opencode") {
        const runGitInfo = config.opencode_workdir?.trim()
          ? await getPathGitInfo(config.opencode_workdir.trim())
          : await getWorkspaceGitInfo();
        setAgentRunGitSnapshotForCard(cardId, gitSnapshotFromInfo(runGitInfo));
      }

      if (cardColumnIntent(cardId) !== "in_progress") {
        appendStructuredAgentLogForCard(
          cardId,
          "info",
          "board",
          "Moved card to In Progress locally.",
          [`Card: ${ticketLabel(card)}`],
          [`Local board projection moved to In Progress.`],
          ["Continue with Jira and runtime setup."],
        );
        moveCard(cardId, inProgressColumnId, "running");
      } else {
        updateCardExecution(cardId, "running");
      }
      setAgentProgressForCard(cardId, 15);
      appendStructuredAgentLogForCard(
        cardId,
        "info",
        "model",
        config.runtime === "opencode" ? `Using OpenCode / ${config.opencode_model}.` : `Using ${config.provider_name} / ${config.model}.`,
        [`Runtime selected: ${config.runtime === "opencode" ? `OpenCode ${config.opencode_model}` : `${config.provider_name} ${config.model}`}`],
        [`Model configuration validated for this run.`],
        ["Wait for the execution result before marking progress."],
      );

      if (issueKey && !isContinuation) {
        const jiraConfig = buildJiraConfig();
        if (jiraConfig) {
          appendStructuredAgentLogForCard(
            cardId,
            "info",
            "jira",
            `Assigning ${issueKey} and moving Jira to In Progress.`,
            [`Issue: ${issueKey}`],
            [`Jira transition target: In Progress`],
            ["Wait for the Jira transition confirmation before execution."],
          );
          setAgentProgressForCard(cardId, 25);
          await assignJiraIssue(jiraConfig, issueKey);
          await transitionJiraIssue(jiraConfig, issueKey, "In Progress");
          appendStructuredAgentLogForCard(
            cardId,
            "success",
            "jira",
            `${issueKey} is In Progress in Jira.`,
            [`Issue transitioned to In Progress successfully.`],
            [`Jira state now mirrors the local run state.`],
            ["Proceed with the exported task context."],
          );
          setAgentProgressForCard(cardId, 35);
        }
      } else {
        appendStructuredAgentLogForCard(
          cardId,
          "info",
          "local",
          "Local Spacesly task. Jira sync is not required.",
          [`Issue linked: none`],
          [`The run will stay local and update only the board state.`],
          ["Proceed with the exported task context."],
        );
        setAgentProgressForCard(cardId, 35);
      }
      if (issueKey && isContinuation) {
        appendStructuredAgentLogForCard(
          cardId,
          "info",
          "jira",
          `${issueKey} is already in execution. Skipping duplicate Jira In Progress transition.`,
          [`Issue: ${issueKey}`],
          [`Continuation run detected.`],
          ["Proceed with the exported task context."],
        );
        setAgentProgressForCard(cardId, 35);
      }

      appendStructuredAgentLogForCard(
        cardId,
        "info",
        "context",
        `Exported structured context for ${ticketLabel(card)}.`,
        [
          `Sections: SUMMARY, EVIDENCE, DETAILS`,
          `Runtime: ${config.runtime === "opencode" ? `OpenCode ${config.opencode_model}` : `${config.provider_name} ${config.model}`}`,
          `Jira: ${issueKey ?? "none"}`,
        ],
        [
          `Operator notes: ${operatorNotes ? "included" : "none"}`,
          `Previous output: ${previousOutput ? "included" : "none"}`,
          `Task description clipped to keep logs readable.`,
        ],
        ["Pass the exported context to the runtime and wait for evidence."],
      );
      setAgentRunOutputForCard(cardId, buildAgentContextExport(card, config, issueKey, operatorNotes, previousOutput));
      setAgentProgressForCard(cardId, 55);
      let result: AiWorkerTaskResult;
      try {
        result = await executeAiWorkerTask(config, {
          key: issueKey,
          title: card.title,
          description: card.description,
          labels: card.labels,
          url: card.url,
          operator_notes: operatorNotes,
          previous_output: previousOutput,
        });
      } catch (reason) {
        if (reason instanceof IpcPolicyError && reason.category === "timeout") {
          setAgentRunStatusForCard(cardId, "timeout");
          appendStructuredAgentLogForCard(
            cardId,
            "error",
            "timeout",
            "Spacesly stopped waiting for the Agent response before a structured result arrived.",
            [reason.message],
            ["The card is not marked blocked.", "Check the Agent runtime output if it is still running."],
            ["Continue or retry from the current card once the runtime finishes or becomes available."],
          );
          appNotice = { tone: "info", message: `${ticketLabel(card)} timed out waiting for the Agent response.` };
          return;
        }

        throw reason;
      }
      appendStructuredAgentLogForCard(
        cardId,
        result.completion_status === "completed" ? "success" : "error",
        "agent",
        result.summary,
        [
          `Completion status: ${result.completion_status}`,
          `Blocked reason: ${result.blocked_reason ?? "none"}`,
        ],
        [
          `Evidence lines: ${result.evidence.length}`,
          `Detail lines: ${result.details.length}`,
        ],
        result.completion_status === "completed"
          ? ["Review the result, then write back to board and Jira."]
          : ["Inspect the blocker, add notes if needed, and continue."],
      );
      setAgentRunResultForCard(cardId, result);
      setAgentRunOutputForCard(cardId, agentResultText(result));
      appendTerminalLineForCard(cardId, "agent", agentResultText(result));
      appendAgentSessionTranscriptForCard(cardId, result.completion_status === "completed" ? "agent_output" : "blocker", agentResultText(result));
      setAgentProgressForCard(cardId, 75);

      if (result.completion_status !== "completed") {
        const reason = result.blocked_reason ?? result.summary;
        updateCardExecution(cardId, { blocked: { reason } });
        setAgentRunStatusForCard(cardId, "blocked");
        appendAgentSessionTranscriptForCard(cardId, "blocker", reason);
        appendStructuredAgentLogForCard(
          cardId,
          "error",
          "blocked",
          "Agent did not complete and verify the requested work. Card will not move to Done.",
          [
            `Completion status: ${result.completion_status}`,
            `Blocked reason: ${reason}`,
          ],
          [
            `Card execution remains blocked until the issue is resolved.`,
            `No Done transition will occur for this run.`,
          ],
          ["Add operator notes or fix the blocker, then continue the Agent."],
        );
        appNotice = { tone: "error", message: `${ticketLabel(card)} blocked: ${reason}` };
        return;
      }

      let gitWritebackInfo: GitWorkspaceInfo | null = null;
      try {
        gitWritebackInfo = await verifyAgentJiraDoneGate(card, config);
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : String(reason);
        updateCardExecution(cardId, { blocked: { reason: message } });
        setAgentRunStatusForCard(cardId, "blocked");
        appendAgentSessionTranscriptForCard(cardId, "blocker", message);
        appendStructuredAgentLogForCard(
          cardId,
          "error",
          "blocked",
          message,
          ["The Agent finished, but Jira writeback was blocked before Done."],
          ["Resolve the git evidence issue, then retry the writeback."],
          ["Commit and push the repository change, then run the writeback again."],
        );
        appNotice = { tone: "error", message };
        return;
      }

      appendStructuredAgentLogForCard(
        cardId,
        "success",
        "board",
        "Stored Agent summary on card and moved card to Done locally.",
        [
          `Card: ${ticketLabel(card)}`,
          `Board target: Done`,
        ],
        [
          `Local workspace projection updated with the verified summary.`,
          `Completed timestamp stored for the card.`,
        ],
        issueKey ? ["Write Jira completion state and add the completion comment."] : ["No Jira issue linked; board write-back is complete."],
      );
      moveCard(cardId, doneColumnId, { completed: { summary: result.summary } });
      setAgentProgressForCard(cardId, 82);

      if (issueKey) {
        const jiraConfig = buildJiraConfig();
        if (jiraConfig) {
          appendStructuredAgentLogForCard(
            cardId,
            "info",
            "jira",
            `Moving ${issueKey} to Done.`,
            [
              `Issue: ${issueKey}`,
              `Transition target: Done`,
            ],
            ["Posting board completion back to Jira."],
            ["Wait for the Jira transition result before finalizing the run."],
          );
          setAgentProgressForCard(cardId, 88);
          await transitionJiraIssue(jiraConfig, issueKey, "Done");
          appendStructuredAgentLogForCard(
            cardId,
            "success",
            "jira",
            `${issueKey} is Done in Jira.`,
            [`Issue transitioned to Done successfully.`],
            [`Jira state now matches the local board state.`],
            [`Post the completion comment with evidence.`],
          );
          appendStructuredAgentLogForCard(
            cardId,
            "info",
            "jira",
            `Posting Spacesly completion comment to ${issueKey}.`,
            [`Comment target: ${issueKey}`],
            [`Comment includes summary and verification evidence.`],
            [`Wait for comment confirmation before final completion.`],
          );
          setAgentProgressForCard(cardId, 94);
          await addJiraComment(jiraConfig, issueKey, agentJiraComment(result, config, gitWritebackInfo));
          appendStructuredAgentLogForCard(
            cardId,
            "success",
            "jira",
            `Spacesly completion comment posted to ${issueKey}.`,
            [`Completion comment posted successfully.`],
            [`Evidence is now persisted in Jira.`],
            [`Finalize the run as completed.`],
          );
        }
      }

      setAgentRunStatusForCard(cardId, "completed");
      setAgentProgressForCard(cardId, 100);
      appNotice = { tone: "success", message: `${ticketLabel(card)} completed by Agent and moved to Done.` };
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      updateCardExecution(cardId, { blocked: { reason: message } });
      setAgentRunStatusForCard(cardId, "blocked");
      setAgentRunResultForCard(cardId, null);
      setAgentRunOutputForCard(cardId, message);
      appendTerminalLineForCard(cardId, "error", message);
      appendAgentSessionTranscriptForCard(cardId, "error", message);
      appendStructuredAgentLogForCard(
        cardId,
        "error",
        "blocked",
        message,
        [`Failure: ${message}`],
        [`The run was interrupted before completion.`],
        [`Resolve the failure, then retry the Agent.`],
      );
      appNotice = { tone: "error", message };
    } finally {
      const { [cardId]: _finished, ...remainingRuns } = runningWorkerCardIds;
      runningWorkerCardIds = remainingRuns;
    }
  }
</script>

<svelte:head>
  <title>Spacesly</title>
</svelte:head>

<main class="stage">
  <section class="window" aria-label="Spacesly workspace">
    <header class="titlebar">
      <div class="workspace-picker">
        <strong>{workspace?.projects[0]?.name.toLowerCase() ?? "spacesly"}</strong>
        <span>⌄</span>
      </div>

      <button class="icon-button" type="button" aria-label="Settings" onclick={() => openSettings()}>Settings</button>

      <nav class="mode-switch" aria-label="Workspace mode">
        <button class:active={workspaceMode === "board"} type="button" aria-label="Board view" onclick={() => setWorkspaceMode("board")}>Board</button>
        <button class:active={workspaceMode === "files"} type="button" aria-label="Files view" onclick={() => setWorkspaceMode("files")}>Files</button>
        <button class:active={workspaceMode === "term"} type="button" aria-label="Terminal view" onclick={openTermWorkspace}>Term</button>
      </nav>

      <button
        class:connected={workerConnected}
        class="worker-pill"
        type="button"
        title={workerStatusLabel}
        onclick={() => openSettings("agent")}
      >
        <span></span>
        {selectedAgentLabel}
      </button>

      <button class="sync-button" type="button" disabled={syncing} onclick={syncJira}>
        {syncing ? "Syncing Jira" : settings.jira.boardId ? "Sync Jira board" : "Sync Jira"}
      </button>

    </header>

    {#if settingsOpen}
      <section class="settings-backdrop" aria-label="Settings dialog">
        <div class="settings-panel">
          <header>
            <div>
              <p>Settings</p>
              <h2>{settingsTitle}</h2>
            </div>
            <button type="button" onclick={closeSettings}>×</button>
          </header>

          <div class:mcp={settingsTab === "mcp"} class="settings-grid">
            <aside class="settings-nav" aria-label="Settings navigation">
              <button class:active={settingsTab === "agent"} type="button" onclick={() => switchSettingsTab("agent")}>
                <strong>Agent</strong>
                <span>Model and worker runtime</span>
              </button>
              <button class:active={settingsTab === "rules"} type="button" onclick={() => switchSettingsTab("rules")}>
                <strong>Rules</strong>
                <span>Operating guardrails</span>
              </button>
              <button class:active={settingsTab === "skills"} type="button" onclick={() => switchSettingsTab("skills")}>
                <strong>Skills</strong>
                <span>Reusable playbooks</span>
              </button>
              <button class:active={settingsTab === "mcp"} type="button" onclick={() => switchSettingsTab("mcp")}>
                <strong>MCP</strong>
                <span>Tools and server connections</span>
              </button>
              <button class:active={settingsTab === "jira"} type="button" onclick={() => switchSettingsTab("jira")}>
                <strong>Jira</strong>
                <span>Board sync and credentials</span>
              </button>
              <button class:active={settingsTab === "theme"} type="button" onclick={() => switchSettingsTab("theme")}>
                <strong>Theme</strong>
                <span>Appearance preferences</span>
              </button>
            </aside>

            {#if settingsTab === "mcp"}
            <aside class="server-list">
              {#each settings.mcpServers as server (server.id)}
                <button
                  class:active={server.id === selectedServerId}
                  type="button"
                  onclick={() => {
                    selectedServerId = server.id;
                    if (server.kind === "jira") {
                      settings = { ...settings, jira: { ...settings.jira, serverId: server.id } };
                    }
                  }}
                >
                  <strong>{server.name || "Unnamed MCP"}</strong>
                  <span>{server.kind.toUpperCase()} · {server.command || "No command configured"}</span>
                  <small class={`mcp-status ${mcpConnectionState(server.id)?.status ?? "unknown"}`}>
                    <i></i>
                    {mcpConnectionLabel(server.id)}
                  </small>
                  <em>{mcpConnectionDetail(server.id)}</em>
                </button>
              {/each}
              <button class="add-server" type="button" onclick={addMcpServer}>＋ Add MCP</button>
            </aside>
            {/if}

            {#if selectedServer}
              <form class="settings-form" onsubmit={(event) => event.preventDefault()}>
                {#if settingsTab === "mcp"}
                  {#if mcpConnectionModule}
                    {@const McpConnectionSettings = mcpConnectionModule.default}
                    <McpConnectionSettings
                      server={selectedServer}
                      jiraBaseUrl={settings.jira.baseUrl}
                      jiraPrincipal={settings.jira.username}
                      jiraAuthMode={settings.jira.authMode}
                      onUpdate={updateSelectedServer}
                      onError={(message) => (settingsError = message)}
                    />
                  {:else}
                    <section class="settings-section">
                      <div>
                        <p class="section-kicker">MCP Connection</p>
                        <h3>Loading connection settings</h3>
                      </div>
                      <p class="field-help">Preparing MCP controls only when this settings tab is opened.</p>
                    </section>
                  {/if}
                {/if}

                {#if settingsTab === "agent"}
                <div class="settings-section worker-section">
                  <div>
                    <p class="section-kicker">Agent</p>
                    <h3>Model runtime</h3>
                  </div>
                  <p class="field-help">Choose a supported provider and model. Spacesly configures the endpoint automatically and only asks for the credential that provider requires.</p>

                  <div class="runtime-options" aria-label="Agent runtime">
                    <button
                      class:active={settings.aiWorker.runtime === "api"}
                      type="button"
                      onclick={() => {
                        settings = { ...settings, aiWorker: { ...settings.aiWorker, runtime: "api" } };
                        workerStatus = null;
                      }}
                    >
                      <strong>Direct API</strong>
                      <span>Use provider API keys stored per provider.</span>
                    </button>
                    <button
                      class:active={settings.aiWorker.runtime === "opencode"}
                      type="button"
                      onclick={() => {
                        settings = { ...settings, aiWorker: { ...settings.aiWorker, runtime: "opencode" } };
                        workerStatus = null;
                      }}
                    >
                      <strong>OpenCode OAuth</strong>
                      <span>Use local opencode auth, including OpenAI OAuth.</span>
                    </button>
                  </div>

                  {#if settings.aiWorker.runtime === "api"}
                    <div class="field-row">
                    <label>
                      <span>Provider</span>
                      <select
                        value={settings.aiWorker.providerId}
                        oninput={(event) => {
                          const providerId = event.currentTarget.value;
                          const modelId = settings.aiWorker.modelIds[providerId] ?? defaultModelForProvider(providerId);
                          settings = {
                            ...settings,
                            aiWorker: {
                              ...settings.aiWorker,
                              providerId,
                              modelId,
                            },
                          };
                          workerStatus = null;
                        }}
                      >
                        {#each aiProviders as provider (provider.id)}
                          <option value={provider.id}>{provider.label}</option>
                        {/each}
                      </select>
                    </label>
                    <label>
                      <span>Model</span>
                      <select
                        value={settings.aiWorker.modelId}
                        oninput={(event) => {
                          const modelId = event.currentTarget.value;
                          (settings = {
                            ...settings,
                            aiWorker: {
                              ...settings.aiWorker,
                              modelId,
                              modelIds: {
                                ...settings.aiWorker.modelIds,
                                [selectedAiProvider.id]: modelId,
                              },
                            },
                          });
                          workerStatus = null;
                        }}
                      >
                        {#each selectedAiProvider.models as model (model.id)}
                          <option value={model.id}>{model.label} · {model.description}</option>
                        {/each}
                      </select>
                    </label>
                    </div>

                    <div class="endpoint-card">
                      <span>Endpoint</span>
                      <code>{selectedAiEndpoint}</code>
                    </div>

                    <div class="field-row">
                    <label>
                      <span>{selectedAiProvider.apiKeyLabel}</span>
                      <input
                        type="password"
                        placeholder={selectedAiProvider.apiKeyPlaceholder}
                        value={selectedAiApiKey}
                        oninput={(event) =>
                          (appSecrets = {
                            ...appSecrets,
                            ai_api_keys: {
                              ...appSecrets.ai_api_keys,
                              [selectedAiProvider.id]: event.currentTarget.value,
                            },
                          })}
                      />
                    </label>
                    <label>
                      <span>Temperature</span>
                      <input
                        type="number"
                        min="0"
                        max="2"
                        step="0.1"
                        value={settings.aiWorker.temperature}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, temperature: Number(event.currentTarget.value) },
                          })}
                      />
                    </label>
                    </div>
                  {:else}
                    <div class="endpoint-card">
                      <span>Runtime</span>
                      <code>{settings.aiWorker.opencodeCommand} run --model {settings.aiWorker.opencodeModel}</code>
                    </div>
                    <div class="field-row">
                      <label>
                        <span>OpenCode command</span>
                        <input
                          placeholder="opencode"
                          value={settings.aiWorker.opencodeCommand}
                          oninput={(event) => {
                            settings = {
                              ...settings,
                              aiWorker: { ...settings.aiWorker, opencodeCommand: event.currentTarget.value },
                            };
                            workerStatus = null;
                          }}
                        />
                      </label>
                    </div>
                    <div class="opencode-model-picker" aria-label="OpenCode model">
                      <div class="opencode-model-header">
                        <span>OpenCode model</span>
                        <strong>{settings.aiWorker.opencodeModel}</strong>
                      </div>
                      <div class="opencode-model-grid">
                        {#each opencodeModelOptions as model (model.id)}
                          <button
                            class:active={settings.aiWorker.opencodeModel === model.id}
                            type="button"
                            onclick={() => {
                              settings = {
                                ...settings,
                                aiWorker: { ...settings.aiWorker, opencodeModel: model.id },
                              };
                              workerStatus = null;
                            }}
                          >
                            <span class={`model-badge ${model.badge.toLowerCase()}`}>{model.badge}</span>
                            <strong>{model.label}</strong>
                            <small>{model.provider}</small>
                            <em>{model.description}</em>
                            <code>{model.id}</code>
                          </button>
                        {/each}
                      </div>
                    </div>
                    <label>
                      <span>Working directory</span>
                      <input
                        placeholder="Optional. Defaults to Spacesly current process directory."
                        value={settings.aiWorker.opencodeWorkdir}
                        oninput={(event) => {
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, opencodeWorkdir: event.currentTarget.value },
                          };
                        }}
                      />
                    </label>
                    <label class="check-row">
                      <input
                        type="checkbox"
                        checked={settings.aiWorker.opencodeAutoApprove}
                        oninput={(event) => {
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, opencodeAutoApprove: event.currentTarget.checked },
                          };
                        }}
                      />
                      <span>Allow OpenCode to auto-approve execution requests for file and shell changes.</span>
                    </label>
                    <p class="field-help">Run <code>opencode auth login</code> in your terminal first. Spacesly uses the same local OpenCode credential store and does not need your OpenAI API key for this runtime.</p>
                  {/if}

                  <div class="worker-status">
                    <span class:connected={workerConnected}></span>
                    <strong>{workerStatusLabel}</strong>
                  </div>
                </div>
                {/if}

                {#if settingsTab === "rules"}
                  <div class="settings-section guidance-section rules-section">
                    <div class="guidance-hero">
                      <div>
                        <p class="section-kicker">Agent Governance</p>
                        <h3>Rules that every run must follow</h3>
                        <span>Keep these short, explicit, and enforceable. They are injected into Agent tasks and chat before user instructions.</span>
                      </div>
                      <strong>Always on</strong>
                    </div>

                    <div class="guidance-metrics" aria-label="Rules behavior">
                      <div>
                        <strong>Priority</strong>
                        <span>Higher than task text</span>
                      </div>
                      <div>
                        <strong>Scope</strong>
                        <span>All Agent actions</span>
                      </div>
                      <div>
                        <strong>Format</strong>
                        <span>One rule per line</span>
                      </div>
                    </div>

                    <label class="guidance-editor">
                      <span>Operating rules</span>
                      <textarea
                        bind:this={agentRulesTextarea}
                        class="agent-instruction-field"
                        rows="10"
                        spellcheck="false"
                        placeholder="Never mark a task done unless it was actually executed and verified.&#10;Do not touch secrets unless explicitly requested.&#10;Block instead of guessing when tools or access are missing."
                        value={settings.aiWorker.agentRules}
                        onblur={commitAgentRulesDraft}
                      ></textarea>
                    </label>

                    <div class="guidance-examples">
                      <strong>Good rules</strong>
                      <span>Use direct verbs: verify, block, ask, avoid, require.</span>
                      <span>Avoid vague preferences like “be smart” or “do your best”.</span>
                    </div>
                  </div>
                {/if}

                {#if settingsTab === "skills"}
                  <div class="settings-section guidance-section skills-section">
                    <div class="guidance-hero">
                      <div>
                        <p class="section-kicker">Agent Playbooks</p>
                        <h3>Skills the Agent can reuse</h3>
                        <span>Describe repeatable work patterns for your domain. The Agent applies them only when relevant to a task.</span>
                      </div>
                      <strong>Contextual</strong>
                    </div>

                    <div class="skill-template-grid" aria-label="Skill examples">
                      <div>
                        <strong>Bamboo diagnostics</strong>
                        <span>Check latest build, fetch logs, identify failing job, summarize evidence.</span>
                      </div>
                      <div>
                        <strong>OCP troubleshooting</strong>
                        <span>Inspect pods, events, logs, and resource usage before proposing fixes.</span>
                      </div>
                    </div>

                    <label class="guidance-editor">
                      <span>Reusable skills</span>
                      <textarea
                        bind:this={agentSkillsTextarea}
                        class="agent-instruction-field"
                        rows="12"
                        spellcheck="false"
                        placeholder="Skill: Bamboo diagnostics&#10;Check latest build status, fetch logs, identify failing job, summarize evidence.&#10;&#10;Skill: OCP troubleshooting&#10;Check pod status, recent events, logs, and resource usage before guessing."
                        value={settings.aiWorker.agentSkills}
                        onblur={commitAgentSkillsDraft}
                      ></textarea>
                    </label>
                  </div>
                {/if}

                {#if settingsTab === "jira"}
                <div class="jira-section settings-section">
                  <div>
                    <p class="section-kicker">Jira Integration</p>
                    <h3>Jira account</h3>
                  </div>
                  <p class="field-help">Configure Jira once. These credentials power board sync, card transitions, Jira comments, and the selected Jira MCP connection.</p>

                  <label>
                    <span>Jira MCP Runtime</span>
                    <select
                      value={settings.jira.serverId}
                      oninput={(event) => {
                        selectedServerId = event.currentTarget.value;
                        settings = { ...settings, jira: { ...settings.jira, serverId: event.currentTarget.value } };
                      }}
                    >
                      {#each settings.mcpServers as server (server.id)}
                        <option value={server.id}>{server.name || "Unnamed MCP"} ({server.kind})</option>
                      {/each}
                    </select>
                  </label>
                  <p class="field-help">Only the MCP command lives in the MCP tab. Its Jira identity is inherited from this page so credentials do not drift.</p>

                  <h3>Credentials</h3>
                  <label>
                    <span>Authentication Method</span>
                    <select
                      value={settings.jira.authMode}
                      oninput={(event) =>
                        (settings = {
                          ...settings,
                          jira: {
                            ...settings.jira,
                            authMode: event.currentTarget.value as AppSettings["jira"]["authMode"],
                          },
                        })}
                    >
                      <option value="api_token">Email + API token</option>
                      <option value="pat">Personal access token</option>
                      <option value="password">Username + password</option>
                    </select>
                  </label>

                  <div class="field-row">
                    <label>
                      <span>Jira URL</span>
                      <input
                        placeholder="https://company.atlassian.net"
                        value={settings.jira.baseUrl}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, baseUrl: event.currentTarget.value },
                          })}
                      />
                    </label>
                    {#if settings.jira.authMode !== "pat"}
                      <label>
                        <span>{settings.jira.authMode === "password" ? "Username" : "Email"}</span>
                        <input
                          placeholder={settings.jira.authMode === "password" ? "jira-user" : "you@company.com"}
                          value={settings.jira.username}
                          oninput={(event) =>
                            (settings = {
                              ...settings,
                              jira: { ...settings.jira, username: event.currentTarget.value },
                            })}
                        />
                      </label>
                    {/if}
                  </div>

                  {#if settings.jira.authMode === "api_token"}
                    <label>
                      <span>Jira API Token</span>
                      <input
                        type="password"
                        placeholder="Paste API token here"
                        value={appSecrets.jira_api_token}
                        oninput={(event) =>
                          (appSecrets = {
                            ...appSecrets,
                            jira_api_token: event.currentTarget.value,
                          })}
                      />
                    </label>
                  {:else if settings.jira.authMode === "pat"}
                    <label>
                      <span>Personal Access Token</span>
                      <input
                        type="password"
                        placeholder="Paste PAT here"
                        value={appSecrets.jira_personal_access_token}
                        oninput={(event) =>
                          (appSecrets = {
                            ...appSecrets,
                            jira_personal_access_token: event.currentTarget.value,
                          })}
                      />
                    </label>
                  {:else}
                    <label>
                      <span>Password</span>
                      <input
                        type="password"
                        placeholder="Jira password"
                        value={appSecrets.jira_password}
                        oninput={(event) =>
                          (appSecrets = {
                            ...appSecrets,
                            jira_password: event.currentTarget.value,
                          })}
                      />
                    </label>
                  {/if}

                  <h3>Board sync</h3>
                  <div class="field-row board-picker-row">
                    <label>
                      <span>Jira Board</span>
                      <select
                        value={settings.jira.boardId}
                        oninput={(event) => {
                          const board = settings.jira.boards.find(
                            (entry) => entry.id === event.currentTarget.value,
                          );
                          settings = {
                            ...settings,
                            jira: {
                              ...settings.jira,
                              boardId: event.currentTarget.value,
                              boardName: board?.name ?? settings.jira.boardName,
                            },
                          };
                        }}
                      >
                        <option value="">Use JQL only</option>
                        {#each settings.jira.boards as board (board.id)}
                          <option value={board.id}>{board.name} ({board.board_type})</option>
                        {/each}
                      </select>
                    </label>
                    <button type="button" onclick={loadJiraBoards} disabled={loadingBoards}>
                      {loadingBoards ? "Loading..." : "Load Jira boards"}
                    </button>
                  </div>

                  <div class="field-row">
                    <label>
                      <span>Project Key Filter</span>
                      <input
                        placeholder="PROJ"
                        value={settings.jira.projectKey}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, projectKey: event.currentTarget.value },
                          })}
                      />
                    </label>
                    <label>
                      <span>Board Name Filter</span>
                      <input
                        placeholder="Team Kanban"
                        value={settings.jira.boardNameFilter}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, boardNameFilter: event.currentTarget.value },
                          })}
                      />
                    </label>
                  </div>

                  <label>
                    <span>Manual Jira Board ID</span>
                    <input
                      placeholder="Only needed if board loading fails"
                      value={settings.jira.boardId}
                      oninput={(event) =>
                        (settings = {
                          ...settings,
                          jira: { ...settings.jira, boardId: event.currentTarget.value },
                        })}
                    />
                  </label>

                  <label>
                    <span>Workspace / Board Name</span>
                    <input
                      placeholder="My Jira work"
                      value={settings.jira.boardName}
                      oninput={(event) =>
                        (settings = {
                          ...settings,
                          jira: { ...settings.jira, boardName: event.currentTarget.value },
                        })}
                    />
                  </label>

                  <div class="field-row">
                    <label>
                      <span>Cards Per Sync Page</span>
                      <input
                        type="number"
                        min="1"
                        max="100"
                        value={settings.jira.pageSize}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, pageSize: Number(event.currentTarget.value) },
                          })}
                      />
                    </label>
                    <label>
                      <span>Max Pages Per Sync</span>
                      <input
                        type="number"
                        min="1"
                        max="20"
                        value={settings.jira.maxPages}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, maxPages: Number(event.currentTarget.value) },
                          })}
                      />
                    </label>
                  </div>
                  <p class="field-help">{syncBudgetLabel} Keep this small for daily use; use a narrower JQL instead of fetching many pages.</p>
                  <label>
                    <span>MCP Tool Name</span>
                    <input
                      value={settings.jira.toolName}
                      oninput={(event) =>
                        (settings = {
                          ...settings,
                          jira: { ...settings.jira, toolName: event.currentTarget.value },
                        })}
                    />
                  </label>
                  <div class="field-row advanced-tools">
                    <label>
                      <span>Board List Tool</span>
                      <input
                        value={settings.jira.boardToolName}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, boardToolName: event.currentTarget.value },
                          })}
                      />
                    </label>
                    <label>
                      <span>Board Issues Tool</span>
                      <input
                        value={settings.jira.boardIssuesToolName}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, boardIssuesToolName: event.currentTarget.value },
                          })}
                      />
                    </label>
                  </div>
                  <label>
                    <span>JQL</span>
                    <div class="jql-presets">
                      <button type="button" onclick={() => applyJqlPreset("assigned")}>Assigned to me</button>
                      <button type="button" onclick={() => applyJqlPreset("unassigned_todo")}>Todo + unassigned</button>
                      <button type="button" onclick={() => applyJqlPreset("unresolved")}>All unresolved</button>
                    </div>
                    <textarea
                      value={settings.jira.jql}
                      oninput={(event) =>
                        (settings = {
                          ...settings,
                          jira: { ...settings.jira, jql: event.currentTarget.value },
                        })}
                    ></textarea>
                  </label>
                </div>
                {/if}

                {#if settingsTab === "theme"}
                  <div class="settings-section theme-section">
                    <div>
                      <p class="section-kicker">Theme</p>
                      <h3>Appearance</h3>
                    </div>
                    <div class="theme-card">
                      <strong>Dark command center</strong>
                      <span>Current Spacesly theme. Future color, density, and typography controls will live here instead of crowding integration settings.</span>
                    </div>
                  </div>
                {/if}

                {#if settingsError}
                  <p class="settings-error">{settingsError}</p>
                {/if}

                {#if connectionMessage}
                  <p class="settings-success">{connectionMessage}</p>
                {/if}

                {#if settingsTab === "mcp" && selectedMcpTools.length > 0}
                  <details class="tool-list">
                    <summary>Available MCP tools ({selectedMcpTools.length})</summary>
                    <div>
                      {#each selectedMcpTools as tool}
                        <code>{tool}</code>
                      {/each}
                    </div>
                  </details>
                {/if}

                <footer>
                  {#if settingsTab === "mcp"}
                  <button type="button" onclick={removeSelectedServer} disabled={settings.mcpServers.length <= 1}>
                    Remove
                  </button>
                  <button type="button" onclick={testSelectedMcpConnection} disabled={testingConnection}>
                    {testingConnection ? "Testing..." : "Test connection"}
                  </button>
                  {/if}
                  {#if settingsTab === "jira"}
                  <button class="connect-jira" type="button" onclick={connectJira} disabled={connectingJira}>
                    {connectingJira ? "Connecting..." : "Connect Jira"}
                  </button>
                  <button type="button" onclick={testJiraConnection} disabled={testingConnection}>
                    {testingConnection ? "Testing..." : "Test connection"}
                  </button>
                  {/if}
                  {#if settingsTab === "agent"}
                  <button class="connect-jira" type="button" onclick={testWorkerConnection} disabled={testingWorker}>
                    {testingWorker ? "Testing Agent..." : "Test Agent"}
                  </button>
                  {/if}
                  <button class="save-settings" type="button" onclick={persistSettings}>Save settings</button>
                </footer>
              </form>
            {/if}
          </div>
        </div>
      </section>
    {/if}

    {#if error}
      <section class="state-panel">
        <strong>Unable to load workspace</strong>
        <p>{error}</p>
      </section>
        {:else if activeBoard}
      <section class="board-shell">
        <NotificationStack notice={appNotice} {syncError} onDismissNotice={dismissAppNotice} />

        <div
          class:with-console={agentConsoleOpen && hasAgentConsoleSession}
          class:is-hidden={workspaceMode !== "board"}
          class="workspace-body"
          style={agentConsoleOpen && hasAgentConsoleSession ? `--agent-console-width: ${layoutPrefs.agentConsoleWidth}px; --lane-width: ${layoutPrefs.laneWidth}px;` : `--lane-width: ${layoutPrefs.laneWidth}px;`}
        >
        <BoardWorkspace
          {displayColumns}
          {selectedCardId}
          {draggedCardId}
          {runningWorkerCardIds}
          cardMinHeight={layoutPrefs.cardMinHeight}
          {doneVisibleLimit}
          {hasAgentConsoleSession}
          {agentConsoleOpen}
          agentRunStatus={visibleAgentRunStatus}
          agentRunProgress={visibleAgentRunProgress}
          onResizeLane={(event) => beginLayoutResize(event, "laneWidth", 260, 460, "x")}
          onResizeCard={(event) => beginLayoutResize(event, "cardMinHeight", 170, 360, "y")}
          onOpenAgentConsole={openAgentConsole}
          onDropCard={(cardId, columnId) => void moveCardAndSync(cardId, columnId)}
          onSelectCard={selectCard}
          onQueueCard={queueCard}
          onStartAgent={(cardId) => void startWorkerForCard(cardId)}
          onDeleteCard={removeCard}
          onDragStartCard={(cardId) => {
            draggedCardId = cardId;
          }}
          onDragEndCard={() => {
            draggedCardId = null;
          }}
          onSetDoneVisibleLimit={setDoneVisibleLimit}
          onShowMoreLaneCards={showMoreLaneCards}
          onShowAllLaneCards={showAllLaneCards}
          onOpenNewTask={() => (newTaskOpen = true)}
          canStartAgent={canStartAgent}
          agentActionLabel={agentActionLabel}
          executionLabel={executionLabel}
          ticketLabel={ticketLabel}
          isBlocked={isBlocked}
          operatorNotesForCard={operatorNotesForCard}
        />

        {#if agentConsoleOpen && hasAgentConsoleSession}
          <div class="grid-resize-handle">
            <span
              class="drag-handle horizontal"
              role="separator"
              aria-orientation="horizontal"
              onpointerdown={(event) => beginLayoutResize(event, "agentConsoleWidth", 360, 720, "x", true)}
              onpointermove={moveLayoutResize}
              onpointerup={endLayoutResize}
              onpointercancel={endLayoutResize}
            ></span>
          </div>
          {#if agentConsoleModule}
            {@const AgentConsolePanel = agentConsoleModule.default}
            <AgentConsolePanel
              style={`--agent-log-height: ${layoutPrefs.agentLogHeight}px; --agent-output-height: ${layoutPrefs.agentOutputHeight}px; --agent-approval-height: ${layoutPrefs.agentApprovalHeight}px;`}
              title={visibleAgentRunTitle}
              status={visibleAgentRunStatus}
              progress={visibleAgentRunProgress}
              activityTitle={agentActivityTitle}
              activityDetail={agentActivityDetail}
              nextStep={agentNextStep}
              phases={agentPhases}
              logs={visibleAgentRunLogs}
              transcript={visibleAgentRunTranscript}
              output={visibleAgentRunOutput}
              result={visibleAgentRunResult}
              runStatus={visibleAgentRunStatus}
              terminalLines={visibleAgentTerminalLines}
              terminalInput={agentTerminalInput}
              runCardId={agentConsoleCardId}
              onClose={() => (agentConsoleOpen = false)}
              onResizeLog={(event) => beginLayoutResize(event, "agentLogHeight", 110, 320, "y")}
              onResizeOutput={(event) => beginLayoutResize(event, "agentOutputHeight", 140, 420, "y")}
              onTerminalInputChange={(value) => (agentTerminalInput = value)}
              onSubmitTerminalInput={submitAgentTerminalInput}
              onOpenCard={(cardId) => (selectedCardId = cardId)}
              onMarkBlockedDone={requestManualDoneConfirmation}
            />
          {:else}
            <aside class="agent-console" aria-label="Agent run console loading" style={`--agent-log-height: ${layoutPrefs.agentLogHeight}px; --agent-output-height: ${layoutPrefs.agentOutputHeight}px; --agent-approval-height: ${layoutPrefs.agentApprovalHeight}px;`}>
              <header>
                <div>
                  <p>Agent Console</p>
                  <h3>{visibleAgentRunTitle}</h3>
                </div>
                <div class={`run-state ${visibleAgentRunStatus}`}>{visibleAgentRunStatus}</div>
                <button type="button" aria-label="Close Agent console" onclick={() => (agentConsoleOpen = false)}>×</button>
              </header>
              <div class="console-progress" aria-label="Agent run progress">
                <div class="agent-progress-head">
                  <div>
                    <span>Now</span>
                    <strong>Loading console</strong>
                  </div>
                  <strong>{visibleAgentRunProgress}%</strong>
                </div>
                <progress max="100" value={visibleAgentRunProgress}></progress>
                <p>Preparing the Agent console only when opened.</p>
              </div>
            </aside>
          {/if}
        {/if}
        </div>
        {#if workspaceMode === "board" && newTaskOpen}
          <NewTaskPopover
            title={newTaskTitle}
            description={newTaskDescription}
            onTitleChange={(value) => (newTaskTitle = value)}
            onDescriptionChange={(value) => (newTaskDescription = value)}
            onClose={() => (newTaskOpen = false)}
            onCreate={createLocalTask}
          />
        {/if}

        {#if workspaceMode === "board" && selectedCard}
          <aside class="detail-popover" aria-label="Selected task detail">
            <button class="close-detail" type="button" aria-label="Close" onclick={() => (selectedCardId = null)}>×</button>
            <div class="task-status waiting">
              <span></span>
              <strong>{executionLabel(selectedCard.execution)}</strong>
            </div>
            <h3>{selectedCard.title}</h3>
            <p>
              {#each descriptionParts(selectedCard.description) as part}
                {#if part.url}
                  <a href={part.url} target="_blank" rel="noreferrer">{part.text}</a>
                {:else}
                  {part.text}
                {/if}
              {/each}
            </p>
            <dl>
              <div>
                <dt>Ticket</dt>
                <dd>
                  {#if selectedCard.url}
                    <a href={selectedCard.url} target="_blank" rel="noreferrer">{ticketLabel(selectedCard)}</a>
                  {:else}
                    {ticketLabel(selectedCard)}
                  {/if}
                </dd>
              </div>
              <div>
                <dt>Status</dt>
                <dd>{executionDetail(selectedCard.execution)}</dd>
              </div>
              <div>
                <dt>Labels</dt>
                <dd>{selectedCard.labels.length > 0 ? selectedCard.labels.join(", ") : "None"}</dd>
              </div>
            </dl>
            <footer>
              <span>
                {selectedCardAgentSession
                  ? `Agent terminal saved · ${selectedCardAgentSession.status}`
                  : "Drag this card to another column to update Spacesly locally."}
              </span>
              <div class="detail-actions">
                {#if isBlocked(selectedCard.execution)}
                  <button
                    type="button"
                    disabled={!canStartAgent(selectedCard, Boolean(runningWorkerCardIds[selectedCard.id]))}
                    onclick={() => void startWorkerForCard(selectedCard.id)}
                  >
                    {agentActionLabel(selectedCard, Boolean(runningWorkerCardIds[selectedCard.id]), Boolean(operatorNotesForCard(selectedCard.id)))}
                  </button>
                {/if}
                {#if selectedCardAgentSession}
                  <button type="button" class="open-console-action" onclick={() => openAgentConsole(selectedCard)}>
                    Open Agent Console
                  </button>
                {/if}
                <button type="button" onclick={() => (selectedCardId = null)}>Close</button>
              </div>
            </footer>
          </aside>
        {/if}

          <div
            class:collapsed={fileSidebarCollapsed}
            class:is-hidden={workspaceMode !== "files"}
            class="files-workspace"
            style={`--file-sidebar-width: ${layoutPrefs.fileSidebarWidth}px;`}
          >
            <div class="files-sidebar">
              <SegmentedControl
                ariaLabel="Workspace sidebar tabs"
                activeValue={workspaceSidebarTab}
                items={[
                  { value: "explorer", label: "Explorer" },
                  { value: "source-control", label: "Source Control", badge: sourceControlChangedCount > 0 ? sourceControlChangedCount : undefined },
                ]}
                onSelect={(value) => (workspaceSidebarTab = value as typeof workspaceSidebarTab)}
              />

              {#if workspaceSidebarTab === "explorer"}
                {#if fileBrowserModule}
                  {@const FileBrowserPane = fileBrowserModule.default}
                  <FileBrowserPane
                    {fileRootLabel}
                    {fileDirectory}
                    {fileLoading}
                    {fileError}
                    {fileEntries}
                    {fileFilter}
                    changedFiles={workspaceChangedFiles}
                    expandedFolders={expandedFileEntries}
                    expandingFolders={expandingFilePaths}
                    {activeEditorPath}
                    onOpenFolder={() => void openFolderFromDialog()}
                    onOpenFile={() => void openFileFromDialog()}
                    onCreateFile={() => void createNewFile()}
                    onRefreshDirectory={() => void refreshFileDirectory("")}
                    onOpenEntry={(entry) => void openFileEntry(entry)}
                    onToggleFolder={(entry) => void toggleFileFolder(entry)}
                    onFilterChange={(filter) => (fileFilter = filter)}
                    onClearFilter={clearFileFilter}
                    onCollapseAll={collapseAllFileFolders}
                    onToggleSidebar={toggleFileSidebar}
                  />
                {:else}
                  <aside class="file-browser-pane" aria-label="Workspace files loading">
                    <header>
                      <div>
                        <p>Explorer</p>
                        <h2>Loading browser</h2>
                      </div>
                    </header>
                    <div class="file-empty">Preparing file browser only when Files mode is used.</div>
                  </aside>
                {/if}
              {:else}
                {#if gitActionsModule}
                  {@const GitActionsPane = gitActionsModule.default}
                  <GitActionsPane
                    {workspaceGitInfo}
                    {workspaceGitLoading}
                    {workspaceGitError}
                    {switchingWorkspaceBranch}
                    hasDirtyEditors={hasDirtyEditorFiles}
                    stagedFiles={workspaceGitStatus.staged}
                    unstagedFiles={workspaceGitStatus.unstaged}
                    onStageFile={stageWorkspaceGitPath}
                    onStageAll={stageAllWorkspaceGitPaths}
                    onUnstageFile={unstageWorkspaceGitPath}
                    onUnstageAll={unstageAllWorkspaceGitPaths}
                    onSwitchBranch={(branch) => void switchWorkspaceBranch(branch)}
                    onPull={pullWorkspaceGitChanges}
                    onCommit={commitWorkspaceGitChanges}
                    onPush={pushWorkspaceGitChanges}
                    onMerge={mergeWorkspaceGitBranch}
                    onRebase={rebaseWorkspaceGitBranch}
                    onRefresh={() => refreshWorkspaceGitState()}
                    onOpenFile={(path) => void openFileEntry({ name: fileName(path), path, is_dir: false, size: 0 })}
                  />
                {:else}
                  <aside class="git-actions-pane git-actions-loading" aria-label="Git actions loading">
                    <header>
                      <div>
                        <p>Source control</p>
                        <h2>Loading actions</h2>
                      </div>
                    </header>
                    <div class="git-empty">Preparing git actions only when Files mode is used.</div>
                  </aside>
                {/if}
              {/if}
            </div>

            {#if fileSidebarCollapsed}
              <button class="file-sidebar-rail" type="button" onclick={toggleFileSidebar} aria-label="Show file browser">
                &gt;
              </button>
            {:else}
              <div class="grid-resize-handle file-resize-handle">
                <span
                  class="drag-handle horizontal"
                  role="separator"
                  aria-orientation="horizontal"
                  onpointerdown={(event) => beginLayoutResize(event, "fileSidebarWidth", 240, 560, "x")}
                  onpointermove={moveLayoutResize}
                  onpointerup={endLayoutResize}
                  onpointercancel={endLayoutResize}
                ></span>
              </div>
            {/if}

            {#if editorWorkspaceModule}
              {@const EditorWorkspace = editorWorkspaceModule.default}
              <EditorWorkspace
                {openEditorFiles}
                {activeEditorPath}
                {activeEditorFile}
                {activeEditorReady}
                {activeEditorDirty}
                {formattingFilePath}
                {savingFilePath}
                {editorDiagnostic}
                {fileStatusLabel}
                onFormatActiveFile={() => void formatActiveFile()}
                onSaveActiveFile={() => void saveActiveFile()}
                onSelectEditorTab={selectEditorTab}
                onCloseEditorTab={closeEditorTab}
                onSetEditorDirty={setEditorDirty}
              />
            {:else}
              <section class="code-editor-pane editor-loading" aria-label="Code editor loading">
                <header>
                  <div>
                    <p>Editor</p>
                    <h2>Loading editor</h2>
                  </div>
                </header>
                <div class="editor-empty">
                  <strong>Preparing workspace editor</strong>
                  <span>The editing bundle loads only when Files mode is used.</span>
                </div>
              </section>
            {/if}
          </div>

          <div class:is-hidden={workspaceMode !== "term"} class="term-workspace" style={`--terminal-width: ${layoutPrefs.terminalWidth}px;`}>
            <TerminalWorkspace
              workdir={workspaceShellWorkdir}
              opened={workspaceTerminalOpened}
              onWorkdirChange={(workdir) => {
                workspaceShellWorkdir = workdir;
                saveUiState();
              }}
              onContainerReady={(container) => {
                workspaceTerminalContainer = container;
              }}
            />

            <div class="grid-resize-handle">
              <span
                class="drag-handle horizontal"
                role="separator"
                aria-orientation="horizontal"
                onpointerdown={(event) => beginLayoutResize(event, "terminalWidth", 420, 1100, "x")}
                onpointermove={moveLayoutResize}
                onpointerup={endLayoutResize}
                onpointercancel={endLayoutResize}
              ></span>
            </div>

            {#if workspaceChatModule}
              {@const WorkspaceChatPane = workspaceChatModule.default}
              <WorkspaceChatPane
                title={settings.aiWorker.runtime === "opencode" ? settings.aiWorker.opencodeModel : selectedAiModel.label}
                onOpenRuntimeSettings={() => openSettings("agent")}
                sessions={workspaceChatSessions}
                activeSessionId={workspaceChatActiveSessionId}
                onNewSession={startWorkspaceChatSession}
                onSwitchSession={activateWorkspaceChatSession}
                messages={workspaceChatMessages}
                running={workspaceChatRunning}
                onTextareaReady={(element) => {
                  workspaceChatTextarea = element;
                }}
                onEndReady={(element) => {
                  workspaceChatEnd = element;
                }}
                onSubmit={() => void sendWorkspaceChat()}
                onKeydown={handleWorkspaceChatKeydown}
              />
            {:else}
              <section class="workspace-chat-pane" aria-label="Agent chat loading">
                <header>
                  <div>
                    <p>Agent Chat</p>
                    <h2>Loading chat</h2>
                  </div>
                </header>
                <div class="chat-empty">Preparing chat only when Terminal mode is used.</div>
              </section>
            {/if}
        </div>
      </section>
      {#if backlogStartConfirmation}
        <div class="confirm-backdrop" role="presentation" onclick={() => resolveBacklogStartConfirmation(false)}></div>
        <div class="confirm-panel" role="dialog" aria-modal="true" aria-labelledby="confirm-backlog-start-title">
          <header>
            <div>
              <p>Confirm start</p>
              <h2 id="confirm-backlog-start-title">Start backlog task?</h2>
            </div>
            <button type="button" aria-label="Close confirmation" onclick={() => resolveBacklogStartConfirmation(false)}>×</button>
          </header>
          <div class="confirm-body">
            <p>
              <strong>{backlogStartConfirmation.title}</strong>
              will move from Backlog to In Progress and begin Agent execution.
            </p>
            <p>This will create a running task immediately. Continue?</p>
          </div>
          <footer>
            <button type="button" onclick={() => resolveBacklogStartConfirmation(false)}>Cancel</button>
            <button class="confirm-primary" type="button" onclick={() => resolveBacklogStartConfirmation(true)}>Start Agent</button>
          </footer>
        </div>
      {/if}
      {#if manualDoneConfirmation}
        <div class="confirm-backdrop" role="presentation" onclick={() => void confirmManualDone(false)}></div>
        <div class="confirm-panel" role="dialog" aria-modal="true" aria-labelledby="confirm-manual-done-title">
          <header>
            <div>
              <p>Confirm manual completion</p>
              <h2 id="confirm-manual-done-title">Mark task Done?</h2>
            </div>
            <button type="button" aria-label="Close confirmation" onclick={() => void confirmManualDone(false)}>×</button>
          </header>
          <div class="confirm-body">
            <p>
              <strong>{manualDoneConfirmation.title}</strong>
              is currently blocked. Continue only if you manually solved the task outside the Agent.
            </p>
            <p>Spacesly will move the card to Done and update Jira if this task is linked.</p>
          </div>
          <footer>
            <button type="button" onclick={() => void confirmManualDone(false)}>Cancel</button>
            <button class="confirm-primary" type="button" onclick={() => void confirmManualDone(true)}>Done</button>
          </footer>
        </div>
      {/if}
    {:else}
      <section class="state-panel">Preparing workspace projection...</section>
    {/if}

    <footer class="statusbar">
      <span>{cacheStatusLabel}</span>
      <span>{boardResourceLabel}</span>
      <span>{cacheSizeLabel}</span>
      <span>{currentDate}</span>
      <span>{currentTime}</span>
    </footer>
  </section>
</main>
