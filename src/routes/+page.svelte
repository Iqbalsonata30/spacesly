<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import type { Terminal as XtermTerminal } from "@xterm/xterm";
  import type { FitAddon as XtermFitAddon } from "@xterm/addon-fit";
  import BoardWorkspace from "$lib/components/BoardWorkspace.svelte";
  import NewTaskPopover from "$lib/components/NewTaskPopover.svelte";
  import NotificationStack from "$lib/components/NotificationStack.svelte";
  import SegmentedControl from "$lib/components/SegmentedControl.svelte";
  import TerminalWorkspace from "$lib/components/TerminalWorkspace.svelte";
  import { formatEditorText, validateEditorSyntax } from "$lib/editorFormatting";
  import {
    createRecoveredDocumentSession,
    createDocumentSession,
    documentSnapshot,
    markDocumentExternalConflict,
    markDocumentSaved,
    replaceDocument,
    type CodeEditorHandle,
    type DocumentSession,
  } from "$lib/editorDocument";
  import { createEditorCommandRegistry, type EditorCommandId } from "$lib/editorCommands";
  import {
    canNavigateEditor,
    createEditorNavigation,
    editorNavigationTarget,
    pushEditorLocation,
    type EditorLocation,
  } from "$lib/editorNavigation";
  import {
    aiEditProposalIsStale,
    applyAiEditHunks,
    createAiEditProposal,
    type AiEditProposal,
  } from "$lib/aiEdit";
  import { chatTitleForCard, isGenericChatTitle, summarizeChatTitle } from "$lib/chatSession";
  import {
    displayPath,
    fileName,
    normalizeAbsolutePath,
    workspaceFileChangeIsStructural,
  } from "$lib/filesFeature";
  import { collectAncestorPaths, pruneExpandedFolderTree } from "$lib/fileBrowser";
  import {
    agentActionLabel,
    canStartAgent,
    descriptionParts,
    executionDetail,
    isBlocked,
    mergeSyncedWorkspace,
    recoverInterruptedAgentRuns,
    withCompletionMetadata,
  } from "$lib/boardWorkflow";
  import {
    agentSessionReplay,
    agentTaskCardProjection,
    agentWorkflowCheckpoint,
    agentWorkflowRecoveryDecision,
    appendAgentSessionEvent,
    clearActiveAgentRun,
    clearActiveAgentRuns,
    createAgentRunSession,
    createAgentSessionEvent,
    loadActiveAgentRunCardIds,
    markActiveAgentRun,
    runningAgentSessions,
    type AgentRunGitSnapshot,
    type AgentRunLog,
    type AgentRunSession,
    type AgentRunStatus,
    type AgentSessionEvent,
    type ExecutionRun,
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
    workspaceChatActionRequiresConfirmation,
    workspaceContextRevision,
    workspaceAgentContext,
    type WorkspaceChatAction,
    type WorkspaceChatActionContext,
    type WorkspaceChatActionProposal,
  } from "$lib/workspaceChat";
  import {
    cancelWorkspaceChatRun,
    confirmLegacyWorkspaceChatCancellation,
    createWorkspaceChatRun,
    settleWorkspaceChatCancellation,
    updateWorkspaceChatRun,
    workspaceChatProgressPercent,
    workspaceChatRunFor,
    workspaceChatRunStatus,
    type WorkspaceChatRuns,
  } from "$lib/workspaceChatRuns";
  import { createSourceControlStore } from "$lib/sourceControlStore.svelte";
  import {
    createPromptTaskEnvelope,
    ensureOpenCodePromptProfile,
    executePromptTaskSession,
    PROMPT_TASK_TEMPLATE_VERSION,
    waitForPromptTaskSession,
  } from "$lib/promptTaskSessions";
  import {
    AgentTaskSessionTimeoutError,
    agentEnvelopeFromSnapshot,
    executeAgentTaskSession,
    prepareAgentTaskSession,
    waitForAgentTaskSession,
  } from "$lib/agentTaskSessions";
  import "./page.css";
  import {
    addJiraComment,
    appendConversationMessage,
    aiWorkspaceTrustStatus,
    applyWorkspaceReplace,
    assignJiraIssue,
    beginAiRun,
    cancelAiRun,
    cancelTaskSession,
    cancelAiWorkerTask,
    chatAiWorker,
    proposeAiEdit,
    pruneConversations,
    executeAiWorkerTask,
    getJiraBoards,
    getAiRun,
    getTaskSession,
    getPathGitInfo,
    getWorkspaceGitInfo,
    getWorkspace,
    importConversations,
    listGlobalEnvironmentVariables,
    deleteGlobalEnvironmentVariable,
    deleteRecoverySnapshot,
    listDirectory,
    listConversations,
    loadConversationMessages,
    listRecoverySnapshots,
    lspCloseDocument,
    lspDiagnostics,
    lspDocumentSymbols,
    lspCompletion,
    lspCodeActions,
    lspGotoDefinition,
    lspHover,
    lspReferences,
    lspStartServer,
    lspStopServer,
    lspSyncDocument,
    aiProviderSecretStatuses,
    jiraSecretStatuses,
    mcpEnvironmentSecretStatuses,
    onWorkspaceFileChange,
    openPtyTerminal,
    previewWorkspaceReplace,
    closePtyTerminal,
    disconnectMcpServer,
    readFile,
    removeMcpConnector,
    resizePtyTerminal,
    saveAiProviderSecret,
    saveJiraSecret,
    saveJiraConnectionProfile,
    saveMcpEnvironmentSecret,
    saveExecutionRun,
    saveGlobalEnvironmentVariable,
    searchWorkspace,
    listActiveExecutionRuns,
    listTaskSessions,
    grantAiRunCapabilities,
    releaseAiWorkerRun,
    reserveAiWorkerRun,
    revealGlobalEnvironmentVariable,
    setWorkspaceRoot,
    syncJiraWorkspace,
    syncRecoverySnapshots,
    testAiWorker,
    testMcpServerConnection,
    transitionJiraIssue,
    trustAiWorkspace,
    unwatchWorkspaceFiles,
    watchWorkspaceFiles,
    writeFile,
    writePtyTerminal,
    workspaceRootPath,
    workspaceRootRevision,
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
    type ExecutionContract,
    type TaskSessionEvent,
    type TaskSessionSnapshot,
    type JiraMcpConfig,
    type JiraBoard,
    type LspDiagnostic,
    type LspDocumentSymbol,
    type LspLocation,
    type LspCodeAction,
    type LspCompletionResult,
    type LspServerConfig,
    type WorkspaceProjection,
    type WorkspaceFileChange,
    type WorkspaceSearchResult,
    type WorkspaceReplacePreviewResponse,
    type RecoverySnapshot,
    type RecoverySnapshotInput,
    type GlobalEnvironmentVariable,
    testJiraMcpConnection,
  } from "$lib/ipc";
  import { lspConfigForPath } from "$lib/lspConfig";
  import { shouldPollLspDiagnostics } from "$lib/lspEditor";
  import {
    createMcpServer,
    loadLegacySettingsSecrets,
    loadSettings,
    saveSettings,
    secretsFromSettings,
    settingsWithoutSecrets,
    type AppSettings,
    type AppSecrets,
  } from "$lib/settings";
  import { aiProviders, defaultModelForProvider, modelById, providerById } from "$lib/aiModels";
  import { formatJiraExecutionComment } from "$lib/jiraComment";
  import { opencodeModelOptions } from "$lib/opencodeModels";
  import {
    cachedWorkspaceSizeBytes,
    loadCachedWorkspace,
    locallyDeleteCachedCard,
    locallyDeletedCachedCardIds,
    restoreLocallyDeletedCachedCards,
    saveCachedWorkspace,
  } from "$lib/workspaceCache";

  const initialSettings = loadSettings();
  const initialAppSecrets = loadLegacySettingsSecrets();
  const AGENT_STATUS_KEY = "spacesly.agent.status.v1";
  const MAX_AGENT_LOGS = 120;
  const MAX_AGENT_TERMINAL_LINES = 80;
  const MAX_AGENT_SESSION_EVENTS = 120;
  const MAX_AGENT_SESSION_REPLAY_CHARS = 12_000;
  const MAX_RETAINED_AGENT_SESSIONS = 50;
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
  const RECOVERY_SYNC_DELAY_MS = 500;
  const RECOVERY_MAX_CONTENT_BYTES = 1_000_000;
  // Reduced from 1_500ms — polling every 1.5s generated ~40 IPC calls/min even when idle.
  // 5s strikes a better balance: diagnostics still update promptly after edits, but idle CPU
  // usage drops significantly. After a document change the poll is reset to 400ms anyway
  // (see the lsp-sync path), so the user still sees fast feedback during active editing.
  const LSP_DIAGNOSTIC_POLL_MS = 5_000;
  const NOTICE_AUTO_DISMISS_MS = 3_000;
  const ERROR_NOTICE_AUTO_DISMISS_MS = 5_000;
  const LAYOUT_PREFS_KEY = "spacesly.layout.v1";
  const UI_STATE_KEY = "spacesly.ui.v1";
  const EDITOR_VIM_MODE_KEY = "spacesly.editor.vim-mode.v1";
  const editorCommands = createEditorCommandRegistry();

  type LayoutPrefs = {
    laneWidth: number;
    cardMinHeight: number;
    fileSidebarWidth: number;
    terminalWidth: number;
    agentConsoleWidth: number;
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

  type OpenEditorFile = DocumentSession;

  const defaultLayoutPrefs: LayoutPrefs = {
    laneWidth: 300,
    cardMinHeight: 220,
    fileSidebarWidth: 340,
    terminalWidth: 680,
    agentConsoleWidth: 420,
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
  let deletedJiraCardCount = $state(0);
  let error = $state<string | null>(null);
  let syncError = $state<string | null>(null);
  let syncing = $state(false);
  let testingConnection = $state(false);
  let loadingBoards = $state(false);
  let connectingJira = $state(false);
  let testingWorker = $state(false);
  let runningWorkerCardIds = $state<Record<string, true>>({});
  let runningWorkerRunIds = $state<Record<string, string>>({});
  let connectionMessage = $state<string | null>(null);
  let workerStatus = $state<AiWorkerStatus | null>(null);
  let agentConnectionStates = $state<Record<string, AgentConnectionState>>(
    loadAgentConnectionStates(),
  );
  let mcpConnectionStates = $state<Record<string, McpConnectionState>>({});
  let appNotice = $state<{ tone: "info" | "success" | "error"; message: string } | null>(null);
  let mcpToolsByServer = $state<Record<string, string[]>>({});
  let settingsOpen = $state(false);
  let settingsTab = $state<"agent" | "rules" | "skills" | "mcp" | "environment" | "jira" | "theme">(
    "agent",
  );
  let settingsError = $state<string | null>(null);
  let settings = $state<AppSettings>(initialSettings);
  let appSecrets = $state<AppSecrets>(initialAppSecrets);
  let aiProviderSecrets = $state<Record<string, boolean>>({});
  let mcpEnvironmentSecrets = $state<Record<string, string[]>>({});
  let jiraSecrets = $state<Record<string, boolean>>({});
  let secretsHydrated = $state(false);
  const mcpEnvEditedServerIds = new SvelteSet<string>();
  type GlobalEnvironmentDraft = GlobalEnvironmentVariable & {
    draft?: boolean;
    revealed?: boolean;
    editing?: boolean;
  };
  let globalEnvironmentVariables = $state<GlobalEnvironmentDraft[]>([]);
  let globalEnvironmentSearch = $state("");
  let globalEnvironmentHydrated = $state(false);
  let globalEnvironmentLoading = $state(false);
  let workspaceCacheHydrated = $state(false);
  let durableRunsHydrated = $state(false);
  let durableConversationWorkspaceId = $state<string | null>(null);
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
  let agentTerminalInput = $state("");
  let agentRunSessions = $state<Record<string, AgentRunSession>>({});
  let latestAgentSessionId = $state<string | null>(null);
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
  let editorWorkspaceModule = $state<
    typeof import("$lib/components/EditorWorkspace.svelte") | null
  >(null);
  let editorWorkspaceRuntime: Promise<
    typeof import("$lib/components/EditorWorkspace.svelte")
  > | null = null;
  let fileBrowserModule = $state<typeof import("$lib/components/FileBrowserPane.svelte") | null>(
    null,
  );
  let fileBrowserRuntime: Promise<typeof import("$lib/components/FileBrowserPane.svelte")> | null =
    null;
  let gitActionsModule = $state<typeof import("$lib/components/GitActionsPane.svelte") | null>(
    null,
  );
  let gitActionsRuntime: Promise<typeof import("$lib/components/GitActionsPane.svelte")> | null =
    null;
  let workspaceSearchModule = $state<
    typeof import("$lib/components/WorkspaceSearchPane.svelte") | null
  >(null);
  let workspaceSearchRuntime: Promise<
    typeof import("$lib/components/WorkspaceSearchPane.svelte")
  > | null = null;
  let workspaceChatModule = $state<
    typeof import("$lib/components/WorkspaceChatPane.svelte") | null
  >(null);
  let workspaceChatRuntime: Promise<
    typeof import("$lib/components/WorkspaceChatPane.svelte")
  > | null = null;
  let mcpConnectionModule = $state<
    typeof import("$lib/components/McpConnectionSettings.svelte") | null
  >(null);
  let mcpConnectionRuntime: Promise<
    typeof import("$lib/components/McpConnectionSettings.svelte")
  > | null = null;
  let agentConsoleModule = $state<typeof import("$lib/components/AgentConsolePanel.svelte") | null>(
    null,
  );
  let agentConsoleRuntime: Promise<
    typeof import("$lib/components/AgentConsolePanel.svelte")
  > | null = null;
  let workspaceChatTextarea: HTMLTextAreaElement | null = $state(null);
  let workspaceChatEnd: HTMLDivElement | null = $state(null);
  let agentRulesTextarea: HTMLTextAreaElement | null = $state(null);
  let agentSkillsTextarea: HTMLTextAreaElement | null = $state(null);
  let workspaceChatRuns = $state<WorkspaceChatRuns>({});
  const recoveringPromptSessionIds = new SvelteSet<number>();
  let workspaceChatSession = $state<ChatSessionState>(initialUiState.workspaceChatSession);
  let workspaceChatSessions = $state<ChatSessionState[]>(initialUiState.workspaceChatSessions);
  let workspaceChatActiveSessionId = $state<string>(
    initialUiState.workspaceChatActiveSessionId ?? initialUiState.workspaceChatSession.id,
  );
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
  let fileDirectoryLoaded = $state(false);
  let fileError = $state<string | null>(null);
  let fileFilter = $state("");
  let expandedFileEntries = $state<Record<string, FileEntry[]>>({});
  let expandingFilePaths = $state<Record<string, true>>({});
  let expandingFileFolder = $state<Record<string, number>>({});
  let fileTreeRevision = 0;
  let fileFolderRequestId = 0;
  let fileSidebarCollapsed = $state(false);
  let workspaceSidebarTab = $state<"explorer" | "search" | "source-control">("explorer");
  let workspaceFilesRoot = $state(initialUiState.workspaceFilesRoot);
  let workspaceFilesDirectory = $state(initialUiState.workspaceFilesDirectory);
  let workspaceSearchQuery = $state("");
  let workspaceSearchCaseSensitive = $state(false);
  let workspaceSearchResults = $state<WorkspaceSearchResult[]>([]);
  let workspaceSearchLoading = $state(false);
  let workspaceSearchError = $state<string | null>(null);
  let workspaceSearchFilesSearched = $state(0);
  let workspaceSearchTruncated = $state(false);
  let workspaceSearchTimer: ReturnType<typeof setTimeout> | null = null;
  let workspaceSearchRequestId = 0;
  let workspaceReplaceOpen = $state(false);
  let workspaceReplacement = $state("");
  let workspaceReplacePreview = $state<WorkspaceReplacePreviewResponse | null>(null);
  let workspaceReplaceLoading = $state(false);
  let workspaceReplaceApplying = $state(false);
  let workspaceReplaceError = $state<string | null>(null);
  let workspaceReplaceRequestId = 0;
  let openEditorFiles = $state<OpenEditorFile[]>([]);
  let editorStateVersion = $state(0);
  let activeEditorPath = $state<string | null>(null);
  let activeEditorHandle = $state<CodeEditorHandle | null>(null);
  let editorVimMode = $state(
    typeof localStorage !== "undefined" && localStorage.getItem(EDITOR_VIM_MODE_KEY) === "true",
  );
  let savingFilePath = $state<string | null>(null);
  let formattingFilePath = $state<string | null>(null);
  let editorDiagnostic = $state<string | null>(null);
  let aiEditProposal = $state<AiEditProposal | null>(null);
  let aiEditSelectedHunkIds = $state<string[]>([]);
  let aiEditGenerating = $state(false);
  let aiEditRunId = $state<string | null>(null);
  let aiEditTaskSessionId = $state<number | null>(null);
  let aiEditError = $state<string | null>(null);
  let aiEditRequestId = 0;
  let aiEditPinnedDocumentIds = $state<string[]>([]);
  let editorNavigation = $state(createEditorNavigation());
  let workspaceRoot = $state<string | null>(null);
  let workspaceGitInfo = $state<GitWorkspaceInfo | null>(null);
  let workspaceGitLoading = $state(false);
  let workspaceGitError = $state<string | null>(null);
  let workspaceGitStatus = $state<GitStatus>({ staged: [], unstaged: [] });
  let selectedWorkspaceBranch = $state("");
  let switchingWorkspaceBranch = $state(false);
  let editorDiagnosticTimer: ReturnType<typeof setTimeout> | null = null;
  let editorDiagnosticRequestId = 0;
  let activeLspDiagnostics = $state<LspDiagnostic[]>([]);
  let activeLspSymbols = $state<LspDocumentSymbol[]>([]);
  let activeLspReferences = $state<LspLocation[]>([]);
  let lspReferencesLoading = $state(false);
  let lspReferenceRequestId = 0;
  let activeLspSymbolRevision: number | null = null;
  let lspSymbolsLoading = $state(false);
  let lspSymbolRequestId = 0;
  let activeLspCodeActions = $state<LspCodeAction[]>([]);
  let lspCodeActionsLoading = $state(false);
  let lspCodeActionRevision: number | null = null;
  let activeLspStatus = $state("idle");
  let lspServerStates = $state<Record<string, "starting" | "running" | "error">>({});
  let lspStartPromises = new SvelteMap<string, Promise<boolean>>();
  let lspSyncTimer: ReturnType<typeof setTimeout> | null = null;
  let lspDiagnosticPollTimer: ReturnType<typeof setTimeout> | null = null;
  let workspaceFileChangeTimer: ReturnType<typeof setTimeout> | null = null;
  let recoverySyncTimer: ReturnType<typeof setTimeout> | null = null;
  let recoveryRestoreChecked = false;
  let recoverySyncRequestId = 0;
  let recoverySyncDisabled = false;
  let recoverySyncPromise: Promise<void> = Promise.resolve();
  let pendingWorkspaceFilePaths = new SvelteSet<string>();
  let pendingWorkspaceStructuralChange = false;
  let unlistenWorkspaceFileChanges: (() => void) | null = null;
  let workspaceGitInfoRequestId = 0;
  let workspaceGitStatusRequestId = 0;
  let allowWindowClose = false;
  let unlistenWindowClose: (() => void) | null = null;
  let workspaceProjectionRequest: Promise<void> | null = null;
  let backlogStartConfirmation = $state<{ cardId: string; title: string } | null>(null);
  let backlogStartConfirmationResolve: ((confirmed: boolean) => void) | null = null;
  let manualDoneConfirmation = $state<{ cardId: string; title: string } | null>(null);
  const sourceControl = createSourceControlStore({
    workspaceId: () => workspace?.id,
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
    let disposed = false;
    // Launch cache hydration and the fallback workspace load in parallel.
    // If the cached workspace arrives first it sets `workspace`; the default
    // projection then sees the guard `if (workspace ...)` and aborts.
    // If the cache misses or errors, the default projection is already in
    // flight — reducing cold-start time by 30–50 % compared to waiting for
    // the cache round-trip to complete before firing the second IPC call.
    void hydrateCachedWorkspace();
    void loadDefaultWorkspaceProjection();
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (allowWindowClose || !openEditorFiles.some((file) => file.dirty)) return;
      event.preventDefault();
      event.returnValue = true;
    };
    window.addEventListener("beforeunload", beforeUnload);
    const visibilityChange = () => scheduleLspDiagnosticsPoll(0);
    document.addEventListener("visibilitychange", visibilityChange);
    const unregisterEditorCommands = [
      editorCommands.register("editor.save", saveActiveFile),
      editorCommands.register("editor.format", formatActiveFile),
      editorCommands.register("editor.close", () => {
        if (activeEditorPath) return closeEditorTab(activeEditorPath);
      }),
      editorCommands.register("editor.nextTab", () => selectAdjacentEditorTab(1)),
      editorCommands.register("editor.previousTab", () => selectAdjacentEditorTab(-1)),
      editorCommands.register("editor.goToDefinition", goToDefinition),
      editorCommands.register("editor.quickFix", requestLspCodeActions),
      editorCommands.register("editor.navigateBack", () => navigateEditorHistory(-1)),
      editorCommands.register("editor.navigateForward", () => navigateEditorHistory(1)),
      editorCommands.register("editor.findReferences", findReferences),
    ];
    const editorKeydown = (event: KeyboardEvent) => {
      if (workspaceMode !== "files" || settingsOpen) return;
      const modifier = event.metaKey || event.ctrlKey;
      let command: EditorCommandId | null = null;
      if (modifier && event.key.toLowerCase() === "s") command = "editor.save";
      else if (modifier && event.shiftKey && event.key.toLowerCase() === "f")
        command = "editor.format";
      else if (modifier && event.key.toLowerCase() === "w") command = "editor.close";
      else if (event.ctrlKey && event.key === "Tab")
        command = event.shiftKey ? "editor.previousTab" : "editor.nextTab";
      if (!command || !editorCommands.execute(command)) return;
      event.preventDefault();
    };
    window.addEventListener("keydown", editorKeydown);
    if ("__TAURI_INTERNALS__" in window) {
      void onWorkspaceFileChange(queueWorkspaceFileChange)
        .then((unlisten) => {
          if (disposed) unlisten();
          else unlistenWorkspaceFileChanges = unlisten;
        })
        .catch(() => {});
      void import("@tauri-apps/api/window")
        .then(async ({ getCurrentWindow }) => {
          const appWindow = getCurrentWindow();
          const unlisten = await appWindow.onCloseRequested(async (event) => {
            if (allowWindowClose) return;
            if (!openEditorFiles.some((file) => file.dirty)) return;
            event.preventDefault();
            if (!(await resolveDirtyEditors(openEditorFiles, "closing Spacesly"))) return;
            recoverySyncDisabled = true;
            try {
              await clearCurrentRecoverySnapshots();
            } catch (reason) {
              console.warn("Failed to clear recovery snapshots while closing", reason);
            }
            allowWindowClose = true;
            await appWindow.destroy();
          });
          if (disposed) unlisten();
          else unlistenWindowClose = unlisten;
        })
        .catch(() => {});
    }
    const timer = window.setInterval(() => {
      now = new Date();
    }, 60_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      if (lspSyncTimer) clearTimeout(lspSyncTimer);
      if (lspDiagnosticPollTimer) clearTimeout(lspDiagnosticPollTimer);
      window.removeEventListener("beforeunload", beforeUnload);
      window.removeEventListener("keydown", editorKeydown);
      document.removeEventListener("visibilitychange", visibilityChange);
      for (const unregister of unregisterEditorCommands) unregister();
      if (workspaceFileChangeTimer) clearTimeout(workspaceFileChangeTimer);
      unlistenWindowClose?.();
      unlistenWindowClose = null;
      unlistenWorkspaceFileChanges?.();
      unlistenWorkspaceFileChanges = null;
      if (workspace) void unwatchWorkspaceFiles(workspace.id).catch(() => {});
      void stopWorkspaceLspServers();
    };
  });

  $effect(() => {
    // Backstop: in rare cases where onMount fires before the Tauri IPC bridge is ready,
    // this re-triggers once workspaceCacheHydrated flips. loadDefaultWorkspaceProjection
    // is idempotent — it aborts immediately when workspace is already set.
    if (workspace || !workspaceCacheHydrated) return;

    void loadDefaultWorkspaceProjection();
  });

  $effect(() => {
    if (!workspace || durableRunsHydrated) return;
    durableRunsHydrated = true;
    void hydrateDurableExecutionRuns();
  });

  $effect(() => {
    if (!workspace || durableConversationWorkspaceId === workspace.id) return;
    void hydrateDurableConversations(workspace.id);
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
      void loadWorkspaceSearchRuntime();
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
    if (settingsOpen && settingsTab === "environment") {
      void loadGlobalEnvironmentRuntime();
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
    if (workspaceMode === "files" && workspace && !fileDirectoryLoaded && !fileLoading) {
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
    if (!workspace || !workspaceRoot || !("__TAURI_INTERNALS__" in window)) return;
    void watchWorkspaceFiles(workspace.id).catch((reason: unknown) => {
      fileError = reason instanceof Error ? reason.message : String(reason);
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
    if (workspaceMode !== "files") return;

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
    scheduleLspDiagnosticsPoll();
  });

  $effect(() => {
    if (appNoticeTimer) {
      clearTimeout(appNoticeTimer);
      appNoticeTimer = null;
    }

    if (!appNotice) return;

    const notice = appNotice;
    appNoticeTimer = setTimeout(
      () => {
        if (appNotice === notice) appNotice = null;
        appNoticeTimer = null;
      },
      notice.tone === "error" ? ERROR_NOTICE_AUTO_DISMISS_MS : NOTICE_AUTO_DISMISS_MS,
    );
  });

  onDestroy(() => {
    if (appNoticeTimer) clearTimeout(appNoticeTimer);
    if (recoverySyncTimer) clearTimeout(recoverySyncTimer);
    if (terminalFrameId !== null) window.cancelAnimationFrame(terminalFrameId);
    for (const run of Object.values(workspaceChatRuns)) {
      if (run.streamFrame !== null) window.cancelAnimationFrame(run.streamFrame);
    }
    if (editorDiagnosticTimer) clearTimeout(editorDiagnosticTimer);
    resolveBacklogStartConfirmation(false);
    flushUiState();
    if (!recoverySyncDisabled) void syncDirtyRecoverySnapshots();
    workspaceTerminalResizeObserver?.disconnect();
    void closePtyTerminal(workspaceTerminalId).catch(() => {});
    workspaceTerminal?.dispose();
  });

  let activeBoard = $derived<BoardProjection | null>(workspace?.projects[0]?.boards[0] ?? null);
  let workspaceChatRequestContextValue = $derived.by(() => {
    const context = workspaceAgentContext(activeBoard);
    return { context, revision: workspaceContextRevision(context) };
  });
  let activeWorkspaceChatRun = $derived(
    workspaceChatRunFor(workspaceChatRuns, workspaceChatActiveSessionId),
  );
  let workspaceChatSessionStatuses = $derived(
    Object.fromEntries(
      workspaceChatSessions.map((session) => {
        const run = workspaceChatRunFor(workspaceChatRuns, session.id);
        return [
          session.id,
          {
            status: workspaceChatRunStatus(run),
            progress: workspaceChatProgressPercent(run),
          },
        ];
      }),
    ),
  );
  let displayColumns = $derived<BoardDisplayColumn[]>(
    activeBoard?.columns.map((column) => {
      const cards = visibleCardsForColumn(column);
      return {
        ...column,
        cards,
        totalCardCount: column.cards.length,
        hiddenLaneCardCount:
          column.intent === "done" ? 0 : Math.max(0, column.cards.length - cards.length),
        hiddenDoneCardCount:
          column.intent === "done" ? Math.max(0, column.cards.length - cards.length) : 0,
      };
    }) ?? [],
  );
  let boardIndex = $derived.by((): BoardIndex => {
    const cards: CardProjection[] = [];
    const cardById = new SvelteMap<string, CardProjection>();
    const columnById = new SvelteMap<string, BoardProjection["columns"][number]>();
    const columnByIntent = new SvelteMap<ColumnIntent, BoardProjection["columns"][number]>();
    const cardColumnIntentById = new SvelteMap<string, ColumnIntent>();

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
    selectedCardId ? (activeCardById.get(selectedCardId) ?? null) : null,
  );
  let selectedCardAgentSession = $derived<AgentRunSession | null>(
    selectedCardId ? (agentRunSessions[selectedCardId] ?? null) : null,
  );
  let agentTaskCardProjections = $derived(
    Object.fromEntries(
      Object.entries(agentRunSessions).map(([cardId, session]) => [
        cardId,
        agentTaskCardProjection(session),
      ]),
    ),
  );
  let runningAgentTaskSessions = $derived(runningAgentSessions(agentRunSessions));
  let activeEditorFile = $derived.by((): OpenEditorFile | null => {
    return editorStateVersion >= 0 && activeEditorPath
      ? (openEditorFiles.find((file) => file.path === activeEditorPath) ?? null)
      : null;
  });
  let activeEditorReady = $derived(Boolean(activeEditorHandle));
  let activeEditorDirty = $derived(Boolean(activeEditorFile?.dirty));
  let aiEditStale = $derived(
    Boolean(
      aiEditProposal &&
      (!activeEditorFile ||
        aiEditProposalIsStale(
          aiEditProposal,
          activeEditorFile.id,
          activeEditorFile.revision,
          Object.fromEntries(openEditorFiles.map((file) => [file.id, file.revision])),
        )),
    ),
  );
  let canNavigateEditorBack = $derived(canNavigateEditor(editorNavigation, -1));
  let canNavigateEditorForward = $derived(canNavigateEditor(editorNavigation, 1));
  let aiEditContextOptions = $derived(
    openEditorFiles
      .filter((file) => file.id !== activeEditorFile?.id)
      .map((file) => ({
        id: file.id,
        path: file.path,
        pinned: aiEditPinnedDocumentIds.includes(file.id),
        characters: file.state?.doc.length ?? file.initialValue.length,
      })),
  );
  let aiEditContextCharacters = $derived(
    (activeEditorFile?.state?.doc.length ?? activeEditorFile?.initialValue.length ?? 0) +
      openEditorFiles
        .filter(
          (file) => file.id !== activeEditorFile?.id && aiEditPinnedDocumentIds.includes(file.id),
        )
        .reduce((total, file) => total + (file.state?.doc.length ?? file.initialValue.length), 0),
  );
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
  let workspaceChangedFiles = $derived([
    ...workspaceGitStatus.staged,
    ...workspaceGitStatus.unstaged,
  ]);
  let sourceControlChangedCount = $derived(workspaceChangedFiles.length);
  let selectedServer = $derived(
    settings.mcpServers.find((server) => server.id === selectedServerId) ?? settings.mcpServers[0],
  );
  let selectedMcpTools = $derived(
    selectedServer ? (mcpToolsByServer[selectedServer.id] ?? []) : [],
  );
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
  let cacheStatusLabel = $derived(
    cacheSavedAt ? `Cached ${relativeTime(cacheSavedAt)}` : "No cached board",
  );
  let boardResourceLabel = $derived(`${renderedCardCount}/${activeCards.length} cards rendered`);
  let cacheSizeLabel = $derived(
    cacheSavedAt ? `Cache ${formatBytes(cachedWorkspaceSizeBytes())}` : "Cache empty",
  );
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
    agentConsoleCardId ? (agentRunSessions[agentConsoleCardId] ?? null) : null,
  );
  let visibleAgentRunTitle = $derived(visibleAgentSession?.title ?? "No active run");
  let visibleAgentRunStatus = $derived<AgentRunStatus>(visibleAgentSession?.status ?? "idle");
  let visibleAgentRunProgress = $derived(visibleAgentSession?.progress ?? 0);
  let visibleAgentRunOutput = $derived(visibleAgentSession?.output ?? "");
  let visibleAgentRunResult = $derived(visibleAgentSession?.result ?? null);
  let visibleAgentRunLogs = $derived(visibleAgentSession?.logs ?? []);
  let visibleAgentTerminalLines = $derived(visibleAgentSession?.terminalLines ?? []);
  let visibleAgentRunTranscript = $derived(visibleAgentSession?.transcript ?? []);
  let visibleExecutionRun = $derived(visibleAgentSession?.executionRun ?? null);
  let hasAgentConsoleSession = $derived(Boolean(visibleAgentSession));
  let latestAgentSession = $derived<AgentRunSession | null>(
    latestAgentSessionId ? (agentRunSessions[latestAgentSessionId] ?? null) : null,
  );
  let settingsTitle = $derived(
    {
      agent: "Agent",
      rules: "Agent Rules",
      skills: "Agent Skills",
      mcp: "MCP Connections",
      jira: "Jira Sync",
      theme: "Theme",
      environment: "Global Environment",
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
      const legacyAiKeys = localSecrets.ai_api_keys;
      for (const [providerId, apiKey] of Object.entries(legacyAiKeys)) {
        if (apiKey.trim()) await saveAiProviderSecret(providerId, apiKey);
      }
      for (const server of settings.mcpServers) {
        if (server.kind !== "jira" && server.command.trim()) {
          const localEnvironment = localSecrets.mcp_env[server.id];
          await saveMcpEnvironmentSecret(
            server.id,
            server.command,
            server.args,
            localEnvironment ? localEnvironment : null,
          );
        }
      }
      const jiraServer = settings.mcpServers.find((server) => server.id === settings.jira.serverId);
      if (settings.jira.baseUrl.trim()) {
        await saveJiraConnectionProfile({
          base_url: settings.jira.baseUrl,
          auth_mode: settings.jira.authMode,
          username: settings.jira.username,
          command: jiraServer?.command ?? "",
          args: jiraServer?.args ?? [],
        });
      }
      if (localSecrets.jira_api_token.trim()) {
        await saveJiraSecret("api_token", localSecrets.jira_api_token);
      }
      if (localSecrets.jira_personal_access_token.trim()) {
        await saveJiraSecret("personal_access_token", localSecrets.jira_personal_access_token);
      }
      if (localSecrets.jira_password.trim()) {
        await saveJiraSecret("password", localSecrets.jira_password);
      }

      aiProviderSecrets = await aiProviderSecretStatuses();
      mcpEnvironmentSecrets = await mcpEnvironmentSecretStatuses();
      jiraSecrets = await jiraSecretStatuses();
      appSecrets = {
        jira_api_token: "",
        jira_personal_access_token: "",
        jira_password: "",
        ai_api_keys: {},
        mcp_env: {},
      };
      settings = settingsWithoutSecrets(settings);
      saveSettings(settings);
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
      deletedJiraCardCount = locallyDeletedCachedCardIds().length;
      if (cached && !workspace) {
        const storage = typeof localStorage === "undefined" ? undefined : localStorage;
        const interruptedCardIds = loadActiveAgentRunCardIds(storage);
        workspace = recoverInterruptedAgentRuns(cached.workspace, interruptedCardIds);
        cacheSavedAt = cached.savedAt;
        if (interruptedCardIds.length > 0) {
          clearActiveAgentRuns(storage);
          cacheSavedAt = Date.now();
          saveCachedWorkspace(workspace);
          appNotice = {
            tone: "info",
            message: `${interruptedCardIds.length} Agent run${interruptedCardIds.length === 1 ? " was" : "s were"} interrupted when Spacesly closed. Review and retry when ready.`,
          };
        } else {
          appNotice = {
            tone: "info",
            message: "Loaded saved cards. Sync Jira only when you need fresh updates.",
          };
        }
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

  async function hydrateDurableExecutionRuns() {
    try {
      const runs = await listActiveExecutionRuns();
      const retainedTaskSessions =
        typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
          ? await listTaskSessions()
          : [];
      for (const run of runs) {
        const cardId = run.contract.task_id;
        const card = activeCardById.get(cardId);
        if (!card || agentRunSessions[cardId]) continue;
        const ticketTitle = run.contract.ticket.title || card.title;
        const retainedTaskSession = retainedTaskSessions
          .filter((session) => agentEnvelopeFromSnapshot(session)?.execution_run_id === run.run_id)
          .sort((left, right) => right.id - left.id)[0];
        const taskEnvelope = retainedTaskSession
          ? agentEnvelopeFromSnapshot(retainedTaskSession)
          : null;
        const checkpoint = agentWorkflowCheckpoint(run);
        const recoveryDecision = agentWorkflowRecoveryDecision(checkpoint);
        const recoveredStatus: AgentRunStatus = !recoveryDecision.safe
          ? "blocked"
          : retainedTaskSession
            ? taskSessionIsTerminal(retainedTaskSession)
              ? "blocked_for_resume"
              : "running"
            : run.status === "blocked" || run.status === "failed"
              ? "blocked"
              : "running";
        agentRunSessions[cardId] = createAgentRunSession(
          cardId,
          ticketTitle,
          recoveredStatus,
          recoveredStatus === "blocked" ? 75 : 55,
          "Recovered durable execution state.",
          null,
          [
            {
              id: `recovered-${run.run_id}`,
              at: new Date().toLocaleTimeString(),
              tone: recoveredStatus === "blocked" ? "error" : "info",
              label: "recovery",
              message: !recoveryDecision.safe
                ? recoveryDecision.reason
                : recoveredStatus === "blocked"
                  ? retainedTaskSession
                    ? `A terminal Task Session was recovered at ${checkpoint}. Automatic Jira writeback is blocked; Continue resumes from this persisted boundary.`
                    : "This execution was recovered after the application restarted and needs review."
                  : "This execution is still active in the durable execution store.",
            },
          ],
          [],
          run.contract.repository
            ? {
                repo_root: run.contract.repository.root_path,
                current_branch: run.contract.repository.branch,
                head_commit: run.contract.repository.head_commit,
              }
            : null,
          [],
          run,
          retainedTaskSession?.id ?? null,
          taskEnvelope?.conversation_id ?? null,
          retainedTaskSession?.state ?? null,
          checkpoint,
        );
        if (retainedTaskSession) {
          void watchRecoveredAgentTask(cardId, retainedTaskSession.id);
        }
        latestAgentSessionId = cardId;
      }
      agentRunSessions = retainAgentSessions(agentRunSessions);
    } catch (reason) {
      appNotice = {
        tone: "error",
        message: `Durable execution state could not be loaded: ${reason instanceof Error ? reason.message : String(reason)}`,
      };
    }
  }

  function taskSessionIsTerminal(session: TaskSessionSnapshot): boolean {
    return (
      session.state === "succeeded" ||
      session.state === "failed" ||
      session.state === "blocked" ||
      session.state === "cancelled"
    );
  }

  async function loadDefaultWorkspaceProjection() {
    if (workspace || workspaceProjectionRequest) return workspaceProjectionRequest;

    workspaceProjectionRequest = getWorkspace()
      .then((projection) => {
        if (!workspace) workspace = projection;
      })
      .catch((reason: unknown) => {
        if (!workspace) error = reason instanceof Error ? reason.message : String(reason);
      })
      .finally(() => {
        workspaceProjectionRequest = null;
      });

    return workspaceProjectionRequest;
  }

  async function persistSettingsAndSecrets(value: AppSettings) {
    for (const [providerId, apiKey] of Object.entries(appSecrets.ai_api_keys)) {
      await saveAiProviderSecret(providerId, apiKey);
    }
    for (const server of value.mcpServers) {
      if (server.kind !== "jira" && server.command.trim()) {
        const environment =
          mcpEnvEditedServerIds.has(server.id) || Object.keys(server.env).length > 0
            ? server.env
            : null;
        await saveMcpEnvironmentSecret(server.id, server.command, server.args, environment);
      }
    }
    const jiraServer = value.mcpServers.find((server) => server.id === value.jira.serverId);
    if (value.jira.baseUrl.trim()) {
      await saveJiraConnectionProfile({
        base_url: value.jira.baseUrl,
        auth_mode: value.jira.authMode,
        username: value.jira.username,
        command: jiraServer?.command ?? "",
        args: jiraServer?.args ?? [],
      });
    }
    if (appSecrets.jira_api_token.trim()) {
      await saveJiraSecret("api_token", appSecrets.jira_api_token);
    }
    if (appSecrets.jira_personal_access_token.trim()) {
      await saveJiraSecret("personal_access_token", appSecrets.jira_personal_access_token);
    }
    if (appSecrets.jira_password.trim()) {
      await saveJiraSecret("password", appSecrets.jira_password);
    }
    aiProviderSecrets = await aiProviderSecretStatuses();
    mcpEnvironmentSecrets = await mcpEnvironmentSecretStatuses();
    jiraSecrets = await jiraSecretStatuses();
    appSecrets = {
      jira_api_token: "",
      jira_personal_access_token: "",
      jira_password: "",
      ai_api_keys: {},
      mcp_env: {},
    };
    mcpEnvEditedServerIds.clear();
    saveSettings(settingsWithoutSecrets(value));
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
          recentCardIds: workspaceChatSession.recentCardIds.slice(
            0,
            MAX_WORKSPACE_CHAT_RECENT_CARDS,
          ),
        },
        workspaceChatSessions: workspaceChatSessions.map((session) => ({
          ...session,
          messages:
            session.id === workspaceChatSession.id
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
    const changed = workspaceMode !== mode;
    workspaceMode = mode;
    if (changed) saveUiState();
    if (mode === "term") {
      if (workspaceTerminalOpened) {
        scheduleWorkspaceTerminalActivation();
      } else {
        scheduleWorkspaceTerminalInit();
      }
    } else if (mode === "files") {
      void refreshFileDirectory(fileDirectory);
    }
    void tick().then(() => {
      if (workspaceMode === mode) window.dispatchEvent(new Event("resize"));
    });
  }

  async function refreshFileDirectory(relativePath = fileDirectory): Promise<boolean> {
    if (!workspace) return false;
    const workspaceId = workspace.id;
    const revision = fileTreeRevision + 1;
    fileTreeRevision = revision;
    fileLoading = true;
    fileDirectoryLoaded = false;
    fileError = null;
    fileDirectory = relativePath;
    workspaceFilesDirectory = relativePath;
    saveUiState();
    try {
      const entries = await listDirectory(workspaceId, relativePath);
      if (revision !== fileTreeRevision) return false;
      fileEntries = entries;
      return true;
    } catch (reason: unknown) {
      if (revision !== fileTreeRevision) return false;
      fileError = reason instanceof Error ? reason.message : String(reason);
      return false;
    } finally {
      if (revision === fileTreeRevision) {
        fileLoading = false;
        fileDirectoryLoaded = true;
      }
    }
  }

  async function refreshWorkspaceRootLabel() {
    if (!workspace) return;
    const root = await workspaceRootPath(workspace.id);
    workspaceRoot = normalizeAbsolutePath(root);
    workspaceFilesRoot = workspaceRoot;
    fileRootLabel = displayPath(root);
  }

  function queueWorkspaceFileChange(change: WorkspaceFileChange) {
    if (!workspace || change.workspace_id !== workspace.id) return;
    for (const path of change.paths) pendingWorkspaceFilePaths.add(path);
    pendingWorkspaceStructuralChange ||= workspaceFileChangeIsStructural(change.kind);
    if (workspaceFileChangeTimer) clearTimeout(workspaceFileChangeTimer);
    workspaceFileChangeTimer = setTimeout(() => {
      workspaceFileChangeTimer = null;
      const changedPaths = pendingWorkspaceFilePaths;
      const refreshExplorer = pendingWorkspaceStructuralChange;
      pendingWorkspaceFilePaths = new SvelteSet<string>();
      pendingWorkspaceStructuralChange = false;
      void refreshOpenEditorFilesFromDisk(changedPaths);
      if (refreshExplorer) void refreshFileDirectory(fileDirectory);
      void refreshWorkspaceGitState();
    }, 250);
  }

  async function refreshOpenEditorFilesFromDisk(changedPaths?: Set<string>) {
    if (!workspace || openEditorFiles.length === 0) return;

    const activePath = activeEditorPath;
    const workspaceId = workspace.id;
    const filesAtStart = [...openEditorFiles];
    const refreshedResults = await Promise.all(
      filesAtStart.map(async (file) => {
        if (changedPaths && !changedPaths.has(file.path)) {
          return {
            path: file.path,
            file,
            snapshot: null,
            preserved: false,
            missing: false,
            ignored: true,
            conflicted: false,
          };
        }
        if (file.dirty) {
          return {
            path: file.path,
            file,
            snapshot: null,
            preserved: true,
            missing: false,
            ignored: false,
            conflicted: Boolean(changedPaths),
          };
        }
        try {
          const snapshot = await readFile(workspaceId, file.path);
          return {
            path: file.path,
            file,
            snapshot,
            preserved: false,
            missing: false,
            ignored: false,
            conflicted: false,
          };
        } catch {
          return {
            path: file.path,
            file: null,
            snapshot: null,
            preserved: false,
            missing: true,
            ignored: false,
            conflicted: false,
          };
        }
      }),
    );
    const resultByPath = new Map(refreshedResults.map((result) => [result.path, result]));
    const missingFiles: string[] = [];
    let preservedDirtyCount = 0;
    let externalConflictCount = 0;
    openEditorFiles = openEditorFiles.flatMap((current) => {
      const result = resultByPath.get(current.path);
      if (!result || result.ignored) return [current];
      if (result.preserved || current.dirty) {
        preservedDirtyCount += 1;
        if (result.conflicted || Boolean(changedPaths && current.dirty)) {
          markDocumentExternalConflict(current);
          externalConflictCount += 1;
        }
        return [{ ...current }];
      }
      if (result.missing || !result.file || !result.snapshot) {
        missingFiles.push(current.path);
        return [];
      }
      if (current.version === result.snapshot.version) return [current];
      if (current.path === activeEditorPath && activeEditorHandle) {
        activeEditorHandle.setValue(result.snapshot.content, "disk");
      } else {
        replaceDocument(current, result.snapshot.content, "disk");
      }
      markDocumentSaved(current, result.snapshot.content);
      current.version = result.snapshot.version;
      current.rootRevision = result.snapshot.root_revision;
      current.encoding = result.snapshot.encoding;
      current.lineEnding = result.snapshot.line_ending;
      current.externalConflict = false;
      return [{ ...current }];
    });
    activeEditorPath =
      (activePath && openEditorFiles.some((file) => file.path === activePath)
        ? activePath
        : null) ??
      openEditorFiles[0]?.path ??
      null;
    saveUiState();

    if (missingFiles.length > 0) {
      appNotice = {
        tone: "error",
        message: `${missingFiles.length} open file${missingFiles.length === 1 ? " was" : "s were"} missing on this branch and closed.`,
      };
    }
    if (preservedDirtyCount > 0) {
      appNotice = {
        tone: "info",
        message: `${preservedDirtyCount} dirty editor${preservedDirtyCount === 1 ? " was" : "s were"} not reloaded from disk.`,
      };
    }
    if (externalConflictCount > 0) {
      appNotice = {
        tone: "error",
        message: `${externalConflictCount} dirty editor${externalConflictCount === 1 ? " changed" : "s changed"} on disk. Your edits were preserved; reload or compare before saving.`,
      };
    }

    await tick();
    activeEditorHandle?.focus();
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
    if (!workspaceGitInfo?.is_git_repo) return false;
    try {
      const result = await sourceControl.commit(message);
      syncSourceControlState();
      return result !== null;
    } catch (reason: unknown) {
      workspaceGitError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: workspaceGitError };
      return false;
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
      await setWorkspaceRoot(workspace.id, savedRoot);
      workspaceRoot = savedRoot;
      workspaceFilesRoot = savedRoot;
      fileRootLabel = displayPath(savedRoot);
    }

    const savedActivePath = initialUiState.workspaceFilesActivePath;
    const targetDirectory = "";

    if (fileDirectory !== targetDirectory) {
      await refreshFileDirectory(targetDirectory);
    }

    const restoredPath = await restoreRecoveredEditorSnapshots();
    if (restoredPath) {
      activeEditorPath = restoredPath;
      await expandFileAncestors(restoredPath);
      saveUiState();
      return;
    }

    if (savedActivePath) {
      const existingFile = openEditorFiles.find((file) => file.path === savedActivePath);
      if (existingFile) {
        activeEditorPath = savedActivePath;
        await expandFileAncestors(savedActivePath);
        saveUiState();
        return;
      }

      await openFileEntry({
        name: fileName(savedActivePath),
        path: savedActivePath,
        is_dir: false,
        size: 0,
      });
    }

    saveUiState();
  }

  async function restoreRecoveredEditorSnapshots(): Promise<string | null> {
    if (!workspace || recoveryRestoreChecked) return null;
    const workspaceId = workspace.id;
    recoveryRestoreChecked = true;
    let snapshots: RecoverySnapshot[];
    try {
      snapshots = await listRecoverySnapshots(workspace.id);
    } catch (reason: unknown) {
      console.warn("Failed to load recovery snapshots", reason);
      return null;
    }
    if (snapshots.length === 0) return null;

    const changedCount = snapshots.filter((snapshot) => snapshot.disk_status === "changed").length;
    const missingCount = snapshots.filter((snapshot) => snapshot.disk_status === "missing").length;
    const detail = [
      changedCount ? `${changedCount} changed on disk` : "",
      missingCount ? `${missingCount} missing on disk` : "",
    ]
      .filter(Boolean)
      .join(", ");
    const restore = window.confirm(
      `Spacesly found ${snapshots.length} unsaved recovery snapshot${snapshots.length === 1 ? "" : "s"}${detail ? ` (${detail})` : ""}. Press OK to restore them, or Cancel to discard recovery data.`,
    );
    if (!restore) {
      await Promise.allSettled(
        snapshots.map((snapshot) => deleteRecoverySnapshot(workspaceId, snapshot.path)),
      );
      await syncDirtyRecoverySnapshots();
      return null;
    }

    const existingPaths = new Set(openEditorFiles.map((file) => file.path));
    const recovered = snapshots
      .filter((snapshot) => !existingPaths.has(snapshot.path))
      .map((snapshot) =>
        createRecoveredDocumentSession({
          workspaceId,
          path: snapshot.path,
          name: snapshot.name,
          content: snapshot.content,
          persistedContent: snapshot.persisted_content,
          version: snapshot.persisted_version,
          rootRevision: snapshot.root_revision,
          encoding: snapshot.encoding,
          lineEnding: snapshot.line_ending,
          revision: Math.max(1, snapshot.revision),
          scrollTop: snapshot.scroll_top,
          externalConflict: snapshot.disk_status !== "unchanged",
        }),
      );
    if (recovered.length === 0) return null;
    openEditorFiles = [...openEditorFiles, ...recovered];
    appNotice = {
      tone: "info",
      message: `Restored ${recovered.length} unsaved editor snapshot${recovered.length === 1 ? "" : "s"}.`,
    };
    scheduleRecoverySync();
    return recovered[0].path;
  }

  async function openFolderFromDialog() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const selected = await openDialogIfAvailable({
      directory: true,
      multiple: false,
      defaultPath: workspaceRoot ?? undefined,
    });
    if (typeof selected !== "string") return;
    if (!(await resolveDirtyEditors(openEditorFiles, "opening another folder"))) return;
    cancelAiEdit();
    cancelWorkspaceChat();

    try {
      await clearCurrentRecoverySnapshots().catch((reason: unknown) => {
        console.warn("Failed to clear recovery snapshots before opening a folder", reason);
      });
      await stopWorkspaceLspServers();
      const selectedRoot = normalizeAbsolutePath(await setWorkspaceRoot(workspace.id, selected));
      resetWorkspaceSearch();
      workspaceRoot = selectedRoot;
      workspaceFilesRoot = selectedRoot;
      fileRootLabel = displayPath(selectedRoot);
      fileDirectory = "";
      workspaceFilesDirectory = "";
      openEditorFiles = [];
      aiEditPinnedDocumentIds = [];
      editorNavigation = createEditorNavigation();
      activeEditorHandle = null;
      activeEditorPath = null;
      expandedFileEntries = {};
      expandingFilePaths = {};
      expandingFileFolder = {};
      fileTreeRevision += 1;
      fileFilter = "";
      if (!(await refreshFileDirectory(""))) {
        throw new Error(fileError ?? "Failed to load the selected folder.");
      }
      await refreshWorkspaceGitState();
      saveUiState();
      appNotice = { tone: "success", message: `Opened folder ${fileRootLabel}` };
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: fileError };
    }
  }

  async function openFileFromDialog() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const selected = await openDialogIfAvailable({
      directory: false,
      multiple: false,
      defaultPath: workspaceRoot ?? undefined,
    });
    if (typeof selected !== "string") return;
    if (!(await resolveDirtyEditors(openEditorFiles, "opening a file from another folder"))) return;
    cancelAiEdit();
    cancelWorkspaceChat();

    const normalized = normalizeAbsolutePath(selected);
    const separator = normalized.lastIndexOf("/");
    const parent = separator > 0 ? normalized.slice(0, separator) : normalized;
    const name = separator > 0 ? normalized.slice(separator + 1) : normalized;
    await clearCurrentRecoverySnapshots().catch((reason: unknown) => {
      console.warn("Failed to clear recovery snapshots before opening a file", reason);
    });
    await stopWorkspaceLspServers();
    await setWorkspaceRoot(workspace.id, parent);
    resetWorkspaceSearch();
    fileDirectory = "";
    workspaceFilesDirectory = "";
    expandedFileEntries = {};
    expandingFilePaths = {};
    expandingFileFolder = {};
    openEditorFiles = [];
    aiEditPinnedDocumentIds = [];
    editorNavigation = createEditorNavigation();
    activeEditorHandle = null;
    activeEditorPath = null;
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

  function updateWorkspaceSearchQuery(query: string) {
    workspaceSearchQuery = query;
    invalidateWorkspaceReplacePreview();
    scheduleWorkspaceSearch();
  }

  function resetWorkspaceSearch() {
    if (workspaceSearchTimer) clearTimeout(workspaceSearchTimer);
    workspaceSearchTimer = null;
    workspaceSearchRequestId += 1;
    workspaceSearchQuery = "";
    workspaceSearchResults = [];
    workspaceSearchLoading = false;
    workspaceSearchError = null;
    workspaceSearchFilesSearched = 0;
    workspaceSearchTruncated = false;
    workspaceReplaceRequestId += 1;
    workspaceReplacePreview = null;
    workspaceReplaceLoading = false;
    workspaceReplaceApplying = false;
    workspaceReplaceError = null;
  }

  function updateWorkspaceSearchCaseSensitive(enabled: boolean) {
    workspaceSearchCaseSensitive = enabled;
    invalidateWorkspaceReplacePreview();
    scheduleWorkspaceSearch(0);
  }

  function updateWorkspaceReplacement(replacement: string) {
    workspaceReplacement = replacement;
    invalidateWorkspaceReplacePreview();
  }

  function invalidateWorkspaceReplacePreview() {
    workspaceReplaceRequestId += 1;
    workspaceReplacePreview = null;
    workspaceReplaceLoading = false;
    workspaceReplaceError = null;
  }

  async function previewWorkspaceReplacement() {
    if (!workspace || workspaceSearchQuery.trim().length < 2) return;
    const query = workspaceSearchQuery.trim();
    const replacement = workspaceReplacement;
    const caseSensitive = workspaceSearchCaseSensitive;
    const requestId = ++workspaceReplaceRequestId;
    workspaceReplaceLoading = true;
    workspaceReplaceError = null;
    try {
      const preview = await previewWorkspaceReplace({
        workspace_id: workspace.id,
        query,
        replacement,
        case_sensitive: caseSensitive,
      });
      if (
        requestId !== workspaceReplaceRequestId ||
        workspaceSearchQuery.trim() !== query ||
        workspaceReplacement !== replacement ||
        workspaceSearchCaseSensitive !== caseSensitive
      )
        return;
      workspaceReplacePreview = preview;
    } catch (reason: unknown) {
      if (requestId !== workspaceReplaceRequestId) return;
      workspaceReplaceError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (requestId === workspaceReplaceRequestId) workspaceReplaceLoading = false;
    }
  }

  async function applyWorkspaceReplacement() {
    if (!workspace || !workspaceReplacePreview || workspaceReplacePreview.truncated) return;
    const preview = workspaceReplacePreview;
    const affectedPaths = new Set(preview.files.map((file) => file.file_path));
    const dirtyFiles = openEditorFiles.filter((file) => file.dirty && affectedPaths.has(file.path));
    if (dirtyFiles.length > 0) {
      workspaceReplaceError = `Save or discard changes in ${dirtyFiles.map((file) => file.name).join(", ")} before replacing.`;
      return;
    }
    if (
      !window.confirm(
        `Apply ${preview.total_replacements} replacements across ${preview.files.length} files? Each file write is atomic, but this multi-file operation cannot be rolled back as one filesystem transaction.`,
      )
    )
      return;

    workspaceReplaceApplying = true;
    workspaceReplaceError = null;
    try {
      const result = await applyWorkspaceReplace({
        workspace_id: workspace.id,
        query: workspaceSearchQuery.trim(),
        replacement: workspaceReplacement,
        case_sensitive: workspaceSearchCaseSensitive,
        files: preview.files.map((file) => ({
          file_path: file.file_path,
          version: file.version,
          replacement_count: file.replacement_count,
        })),
        truncated: preview.truncated,
      });
      const changedPaths = new Set(result.files.map((file) => file.file_path));
      workspaceReplacePreview = null;
      await refreshOpenEditorFilesFromDisk(changedPaths);
      await Promise.all([refreshFileDirectory(fileDirectory), refreshWorkspaceGitState()]);
      await runWorkspaceSearch();
      appNotice = {
        tone: "success",
        message: `Applied ${result.total_replacements} replacements across ${result.files.length} files.`,
      };
    } catch (reason: unknown) {
      workspaceReplaceError = reason instanceof Error ? reason.message : String(reason);
      await refreshOpenEditorFilesFromDisk();
      void refreshWorkspaceGitState();
    } finally {
      workspaceReplaceApplying = false;
    }
  }

  function scheduleWorkspaceSearch(delay = 250) {
    if (workspaceSearchTimer) clearTimeout(workspaceSearchTimer);
    const query = workspaceSearchQuery.trim();
    if (query.length < 2) {
      workspaceSearchRequestId += 1;
      workspaceSearchLoading = false;
      workspaceSearchResults = [];
      workspaceSearchError = null;
      workspaceSearchFilesSearched = 0;
      workspaceSearchTruncated = false;
      return;
    }
    workspaceSearchTimer = setTimeout(() => {
      workspaceSearchTimer = null;
      void runWorkspaceSearch();
    }, delay);
  }

  async function runWorkspaceSearch() {
    if (!workspace) return;
    const query = workspaceSearchQuery.trim();
    if (query.length < 2) return;
    const caseSensitive = workspaceSearchCaseSensitive;
    const requestId = ++workspaceSearchRequestId;
    workspaceSearchLoading = true;
    workspaceSearchError = null;
    try {
      const response = await searchWorkspace({
        workspace_id: workspace.id,
        query,
        case_sensitive: caseSensitive,
        max_results: 500,
      });
      if (
        requestId !== workspaceSearchRequestId ||
        workspaceSearchQuery.trim() !== query ||
        workspaceSearchCaseSensitive !== caseSensitive
      )
        return;
      workspaceSearchResults = response.results;
      workspaceSearchFilesSearched = response.files_searched;
      workspaceSearchTruncated = response.truncated;
    } catch (reason: unknown) {
      if (requestId !== workspaceSearchRequestId) return;
      workspaceSearchError = reason instanceof Error ? reason.message : String(reason);
      workspaceSearchResults = [];
    } finally {
      if (requestId === workspaceSearchRequestId) workspaceSearchLoading = false;
    }
  }

  async function openWorkspaceSearchResult(result: WorkspaceSearchResult) {
    const source =
      activeEditorFile && activeEditorHandle
        ? { path: activeEditorFile.path, ...activeEditorHandle.getCursorPosition() }
        : null;
    const target = {
      path: result.file_path,
      line: result.line,
      character: result.character,
    };
    if (!(await navigateToEditorLocation(target))) return;
    editorNavigation = source
      ? pushEditorLocation(pushEditorLocation(editorNavigation, source), target)
      : pushEditorLocation(editorNavigation, target);
  }

  async function createNewFile() {
    if (!workspace) return;
    workspaceSidebarTab = "explorer";
    const target = window.prompt(
      "New file name",
      fileDirectory ? `${fileDirectory}/untitled.txt` : "untitled.txt",
    );
    if (!target) return;

    const normalized = target.replace(/^\/+/, "").trim();
    if (!normalized) return;

    try {
      await writeFile(workspace.id, normalized, "");
      await refreshWorkspaceGitState();
      await refreshFileDirectory(fileDirectory);
      await openFileEntry({ name: fileName(normalized), path: normalized, is_dir: false, size: 0 });
      appNotice = { tone: "success", message: `Created ${normalized}` };
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not create ${normalized}: ${fileError}` };
    }
  }

  async function openFileEntry(entry: FileEntry) {
    if (entry.is_dir) {
      await toggleFileFolder(entry);
      return;
    }

    if (!workspace) return;
    if (activeEditorPath !== entry.path) {
      activeEditorHandle = null;
      activeLspDiagnostics = [];
      activeLspStatus = lspConfigForPath(entry.path) ? "loading" : "unsupported";
    }
    activeEditorPath = entry.path;
    saveUiState();
    if (openEditorFiles.some((file) => file.path === entry.path)) {
      await tick();
      activeEditorHandle?.focus();
      scheduleLspSync(entry.path);
      return;
    }

    fileLoading = true;
    fileError = null;
    try {
      const snapshot = await readFile(workspace.id, entry.path);
      openEditorFiles = [
        ...openEditorFiles,
        createDocumentSession({
          workspaceId: workspace.id,
          path: entry.path,
          name: entry.name,
          content: snapshot.content,
          version: snapshot.version,
          rootRevision: snapshot.root_revision,
          encoding: snapshot.encoding,
          lineEnding: snapshot.line_ending,
        }),
      ];
      await expandFileAncestors(entry.path);
      await tick();
      activeEditorHandle?.focus();
      scheduleEditorDiagnostics();
      scheduleLspSync(entry.path);
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      activeEditorPath = openEditorFiles.at(-1)?.path ?? null;
    } finally {
      fileLoading = false;
    }
  }

  async function toggleFileFolder(entry: FileEntry) {
    if (!workspace || !entry.is_dir || expandingFileFolder[entry.path]) return;

    if (expandedFileEntries[entry.path]) {
      expandedFileEntries = pruneExpandedFolderTree(expandedFileEntries, entry.path);
      return;
    }

    const requestId = ++fileFolderRequestId;
    expandingFileFolder = { ...expandingFileFolder, [entry.path]: requestId };
    expandingFilePaths = { ...expandingFilePaths, [entry.path]: true };
    fileError = null;
    try {
      const children = await listDirectory(workspace.id, entry.path);
      if (expandingFileFolder[entry.path] !== requestId) return;
      expandedFileEntries = { ...expandedFileEntries, [entry.path]: children };
    } catch (reason: unknown) {
      if (expandingFileFolder[entry.path] !== requestId) return;
      fileError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (expandingFileFolder[entry.path] === requestId) {
        const { [entry.path]: _removed, ...remaining } = expandingFileFolder;
        expandingFileFolder = remaining;
        const { [entry.path]: _finished, ...loadingRemaining } = expandingFilePaths;
        expandingFilePaths = loadingRemaining;
      }
    }
  }

  function clearFileFilter() {
    fileFilter = "";
  }

  function collapseAllFileFolders() {
    expandedFileEntries = {};
    expandingFilePaths = {};
    expandingFileFolder = {};
  }

  async function expandFileAncestors(path: string) {
    if (!workspace) return;

    for (const current of collectAncestorPaths(path)) {
      if (expandedFileEntries[current] || expandingFileFolder[current]) continue;
      const folderEntry: FileEntry = {
        name: current.split("/").at(-1) ?? current,
        path: current,
        is_dir: true,
        size: 0,
      };
      await toggleFileFolder(folderEntry);
    }
  }

  function setEditorDirty(path: string, dirty: boolean) {
    openEditorFiles = openEditorFiles.map((file) =>
      file.path === path ? { ...file, dirty } : file,
    );
    scheduleRecoverySync();
    if (path === activeEditorPath) {
      scheduleEditorDiagnostics();
    }
  }

  function onEditorChange(path: string) {
    // DocumentSession is already mutated by CodeMirror; invalidate derived editor state without
    // cloning the entire tab collection on every transaction.
    editorStateVersion += 1;
    if (path === activeEditorPath) {
      activeLspCodeActions = [];
      lspCodeActionRevision = null;
      activeLspSymbolRevision = null;
      activeLspReferences = [];
      lspReferenceRequestId += 1;
      lspReferencesLoading = false;
      scheduleEditorDiagnostics();
    }
    scheduleLspSync(path);
    scheduleRecoverySync();
  }

  function scheduleRecoverySync() {
    if (!workspace || recoverySyncDisabled) return;
    if (recoverySyncTimer) clearTimeout(recoverySyncTimer);
    recoverySyncTimer = setTimeout(() => {
      recoverySyncTimer = null;
      void syncDirtyRecoverySnapshots();
    }, RECOVERY_SYNC_DELAY_MS);
  }

  async function syncDirtyRecoverySnapshots() {
    if (!workspace || recoverySyncDisabled) return;
    const requestId = ++recoverySyncRequestId;
    const snapshots = openEditorFiles.flatMap(recoverySnapshotForSession);
    const workspaceId = workspace.id;
    const sync = recoverySyncPromise
      .catch(() => {})
      .then(() => syncRecoverySnapshots(workspaceId, snapshots));
    recoverySyncPromise = sync;
    try {
      await sync;
    } catch (reason: unknown) {
      if (requestId !== recoverySyncRequestId) return;
      console.warn("Failed to sync recovery snapshots", reason);
    }
  }

  async function clearCurrentRecoverySnapshots() {
    if (!workspace) return;
    if (recoverySyncTimer) {
      clearTimeout(recoverySyncTimer);
      recoverySyncTimer = null;
    }
    await recoverySyncPromise.catch(() => {});
    await syncRecoverySnapshots(workspace.id, []);
  }

  function recoverySnapshotForSession(file: OpenEditorFile): RecoverySnapshotInput[] {
    if (!file.dirty) return [];
    const content = documentSnapshot(file).value;
    if (new TextEncoder().encode(content).length > RECOVERY_MAX_CONTENT_BYTES) return [];
    return [
      {
        path: file.path,
        name: file.name,
        content,
        persisted_version: file.version,
        root_revision: file.rootRevision,
        encoding: file.encoding,
        line_ending: file.lineEnding,
        revision: file.revision,
        scroll_top: Math.max(0, Math.trunc(file.scrollTop)),
      },
    ];
  }

  function onEditorReady(handle: CodeEditorHandle | null) {
    activeEditorHandle = handle;
  }

  function selectEditorTab(path: string) {
    if (activeEditorPath === path) return;
    activeEditorHandle = null;
    activeLspDiagnostics = [];
    activeLspSymbols = [];
    activeLspSymbolRevision = null;
    activeLspReferences = [];
    lspReferenceRequestId += 1;
    lspReferencesLoading = false;
    activeLspCodeActions = [];
    lspCodeActionRevision = null;
    activeLspStatus = lspConfigForPath(path) ? "loading" : "unsupported";
    activeEditorPath = path;
    saveUiState();
    void tick().then(() => {
      activeEditorHandle?.focus();
      scheduleEditorDiagnostics();
      scheduleLspSync(path);
    });
  }

  function scheduleLspSync(path: string) {
    if (lspSyncTimer) clearTimeout(lspSyncTimer);
    if (workspaceSearchTimer) clearTimeout(workspaceSearchTimer);
    lspSyncTimer = setTimeout(() => {
      lspSyncTimer = null;
      void syncDocumentWithLsp(path);
    }, 350);
  }

  async function ensureLspServer(config: LspServerConfig): Promise<boolean> {
    const state = lspServerStates[config.server_id];
    if (state === "running") return true;
    if (state === "error") return false;
    const existing = lspStartPromises.get(config.server_id);
    if (existing) return existing;
    const promise = (async () => {
      if (!workspace) return false;
      lspServerStates = { ...lspServerStates, [config.server_id]: "starting" };
      if (
        activeEditorFile &&
        lspConfigForPath(activeEditorFile.path)?.server_id === config.server_id
      ) {
        activeLspStatus = "starting";
      }
      try {
        await lspStartServer(workspace.id, config);
        lspServerStates = { ...lspServerStates, [config.server_id]: "running" };
        return true;
      } catch (reason: unknown) {
        lspServerStates = { ...lspServerStates, [config.server_id]: "error" };
        const message = reason instanceof Error ? reason.message : String(reason);
        if (
          activeEditorFile &&
          lspConfigForPath(activeEditorFile.path)?.server_id === config.server_id
        ) {
          activeLspStatus = "unavailable";
          editorDiagnostic = `${config.server_id}: ${message}`;
        }
        return false;
      } finally {
        lspStartPromises.delete(config.server_id);
      }
    })();
    lspStartPromises.set(config.server_id, promise);
    return promise;
  }

  async function syncDocumentWithLsp(path: string) {
    if (!workspace) return;
    const file = openEditorFiles.find((entry) => entry.path === path);
    if (!file) return;
    const config = lspConfigForPath(path);
    if (!config) {
      if (path === activeEditorPath) {
        activeLspStatus = "unsupported";
        activeLspDiagnostics = [];
      }
      return;
    }
    if (!(await ensureLspServer(config))) return;
    const snapshot = documentSnapshot(file);
    try {
      await lspSyncDocument(
        workspace.id,
        config.server_id,
        file.path,
        config.language_id,
        snapshot.revision,
        snapshot.value,
      );
      if (path === activeEditorPath) {
        activeLspStatus = "running";
        scheduleLspDiagnosticsPoll(400);
        void refreshActiveLspSymbols();
      }
    } catch (reason: unknown) {
      if (path === activeEditorPath) {
        activeLspStatus = "error";
        editorDiagnostic = reason instanceof Error ? reason.message : String(reason);
      }
    }
  }

  async function refreshActiveLspDiagnostics() {
    if (
      !shouldPollLspDiagnostics(workspaceMode === "files", document.visibilityState === "visible")
    ) {
      return;
    }
    if (!workspace || !activeEditorFile) return;
    const file = activeEditorFile;
    const config = lspConfigForPath(file.path);
    if (!config || lspServerStates[config.server_id] !== "running") return;
    const revision = file.revision;
    try {
      const report = await lspDiagnostics(workspace.id, config.server_id, file.path);
      if (file.path !== activeEditorPath || file.revision !== revision) return;
      if (report.version !== null && report.version !== revision) return;
      activeLspDiagnostics = report.diagnostics;
      activeLspStatus = "running";
    } catch {
      activeLspStatus = "error";
    }
  }

  function scheduleLspDiagnosticsPoll(delay = LSP_DIAGNOSTIC_POLL_MS) {
    if (lspDiagnosticPollTimer) {
      clearTimeout(lspDiagnosticPollTimer);
      lspDiagnosticPollTimer = null;
    }
    const path = activeEditorPath;
    const config = path ? lspConfigForPath(path) : null;
    if (
      !config ||
      lspServerStates[config.server_id] !== "running" ||
      !shouldPollLspDiagnostics(workspaceMode === "files", document.visibilityState === "visible")
    ) {
      return;
    }
    lspDiagnosticPollTimer = setTimeout(async () => {
      lspDiagnosticPollTimer = null;
      await refreshActiveLspDiagnostics();
      scheduleLspDiagnosticsPoll();
    }, delay);
  }

  async function refreshActiveLspSymbols(force = false) {
    if (!workspace || !activeEditorFile) return;
    const file = activeEditorFile;
    const revision = file.revision;
    if (!force && activeLspSymbolRevision === revision) return;
    const config = lspConfigForPath(file.path);
    if (!config || lspServerStates[config.server_id] !== "running") return;
    const requestId = ++lspSymbolRequestId;
    lspSymbolsLoading = true;
    try {
      const symbols = await lspDocumentSymbols(workspace.id, config.server_id, file.path);
      if (
        requestId !== lspSymbolRequestId ||
        activeEditorPath !== file.path ||
        file.revision !== revision
      )
        return;
      activeLspSymbols = symbols;
      activeLspSymbolRevision = revision;
    } catch {
      if (requestId === lspSymbolRequestId && activeEditorPath === file.path) activeLspSymbols = [];
    } finally {
      if (requestId === lspSymbolRequestId) lspSymbolsLoading = false;
    }
  }

  async function goToDefinition() {
    if (!workspace || !activeEditorFile || !activeEditorHandle) return;
    const file = activeEditorFile;
    const revision = file.revision;
    const sourcePosition = activeEditorHandle.getCursorPosition();
    const config = lspConfigForPath(file.path);
    if (!config) {
      appNotice = { tone: "info", message: "No language server is configured for this file." };
      return;
    }
    if (!(await ensureLspServer(config))) return;

    try {
      await syncDocumentWithLsp(file.path);
      const location = await lspGotoDefinition(
        workspace.id,
        config.server_id,
        file.path,
        sourcePosition,
      );
      if (activeEditorPath !== file.path || file.revision !== revision) return;
      if (!location) {
        appNotice = { tone: "info", message: "No definition found at the cursor." };
        return;
      }

      const source = { path: file.path, ...sourcePosition };
      const target = {
        path: location.file_path,
        line: location.line,
        character: location.character,
      };
      if (!(await navigateToEditorLocation(target))) return;
      editorNavigation = pushEditorLocation(pushEditorLocation(editorNavigation, source), target);
    } catch (reason: unknown) {
      const message = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not find definition: ${message}` };
    }
  }

  async function findReferences() {
    if (!workspace || !activeEditorFile || !activeEditorHandle) return;
    const file = activeEditorFile;
    const revision = file.revision;
    const position = activeEditorHandle.getCursorPosition();
    const config = lspConfigForPath(file.path);
    if (!config) {
      appNotice = { tone: "info", message: "No language server is configured for this file." };
      return;
    }
    if (!(await ensureLspServer(config))) return;
    const requestId = ++lspReferenceRequestId;
    lspReferencesLoading = true;
    activeLspReferences = [];
    try {
      await syncDocumentWithLsp(file.path);
      const references = await lspReferences(workspace.id, config.server_id, file.path, position);
      if (
        requestId !== lspReferenceRequestId ||
        activeEditorPath !== file.path ||
        file.revision !== revision
      )
        return;
      activeLspReferences = references;
      if (references.length === 0) {
        appNotice = { tone: "info", message: "No references found at the cursor." };
      }
    } catch (reason: unknown) {
      if (requestId !== lspReferenceRequestId) return;
      const message = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not find references: ${message}` };
    } finally {
      if (requestId === lspReferenceRequestId) lspReferencesLoading = false;
    }
  }

  async function navigateEditorHistory(direction: -1 | 1) {
    const target = editorNavigationTarget(editorNavigation, direction);
    if (!target || !(await navigateToEditorLocation(target.location))) return;
    editorNavigation = target.state;
  }

  async function navigateToDocumentSymbol(symbol: LspDocumentSymbol) {
    if (!activeEditorFile || !activeEditorHandle) return;
    const source = { path: activeEditorFile.path, ...activeEditorHandle.getCursorPosition() };
    const target = {
      path: activeEditorFile.path,
      line: symbol.selection_range.start.line,
      character: symbol.selection_range.start.character,
    };
    if (!(await navigateToEditorLocation(target))) return;
    editorNavigation = pushEditorLocation(pushEditorLocation(editorNavigation, source), target);
  }

  async function navigateToReference(location: LspLocation) {
    const source =
      activeEditorFile && activeEditorHandle
        ? { path: activeEditorFile.path, ...activeEditorHandle.getCursorPosition() }
        : null;
    const target = {
      path: location.file_path,
      line: location.line,
      character: location.character,
    };
    if (!(await navigateToEditorLocation(target))) return;
    editorNavigation = source
      ? pushEditorLocation(pushEditorLocation(editorNavigation, source), target)
      : pushEditorLocation(editorNavigation, target);
  }

  async function navigateToEditorLocation(location: EditorLocation): Promise<boolean> {
    await openFileEntry({
      name: fileName(location.path),
      path: location.path,
      is_dir: false,
      size: 0,
    });
    if (activeEditorPath !== location.path) return false;
    await tick();
    activeEditorHandle?.setCursorPosition(location.line, location.character);
    return Boolean(activeEditorHandle);
  }

  async function requestLspHover(position: { line: number; character: number }) {
    if (!workspace || !activeEditorFile) return null;
    const file = activeEditorFile;
    const config = lspConfigForPath(file.path);
    if (!config || !(await ensureLspServer(config))) return null;

    try {
      await syncDocumentWithLsp(file.path);
      if (activeEditorPath !== file.path) return null;
      return (await lspHover(workspace.id, config.server_id, file.path, position))?.text ?? null;
    } catch {
      return null;
    }
  }

  async function requestLspCompletion(position: {
    line: number;
    character: number;
  }): Promise<LspCompletionResult | null> {
    if (!workspace || !activeEditorFile) return null;
    const file = activeEditorFile;
    const revision = file.revision;
    const config = lspConfigForPath(file.path);
    if (!config || !(await ensureLspServer(config))) return null;
    try {
      await syncDocumentWithLsp(file.path);
      if (activeEditorPath !== file.path || file.revision !== revision) return null;
      const result = await lspCompletion(workspace.id, config.server_id, {
        file_path: file.path,
        position,
      });
      return activeEditorPath === file.path && file.revision === revision ? result : null;
    } catch {
      return null;
    }
  }

  async function requestLspCodeActions() {
    if (!workspace || !activeEditorFile || !activeEditorHandle || lspCodeActionsLoading) return;
    const file = activeEditorFile;
    const revision = file.revision;
    const config = lspConfigForPath(file.path);
    if (!config || !(await ensureLspServer(config))) return;
    const cursor = activeEditorHandle.getCursorPosition();
    const diagnostic = activeLspDiagnostics.find(
      (entry) =>
        (cursor.line > entry.range.start.line ||
          (cursor.line === entry.range.start.line &&
            cursor.character >= entry.range.start.character)) &&
        (cursor.line < entry.range.end.line ||
          (cursor.line === entry.range.end.line && cursor.character <= entry.range.end.character)),
    );
    const range = diagnostic?.range ?? { start: cursor, end: cursor };
    lspCodeActionsLoading = true;
    try {
      await syncDocumentWithLsp(file.path);
      const actions = await lspCodeActions(workspace.id, config.server_id, {
        file_path: file.path,
        range,
        diagnostics: diagnostic ? [diagnostic] : activeLspDiagnostics,
        only: ["quickfix"],
      });
      if (activeEditorPath !== file.path || file.revision !== revision) return;
      activeLspCodeActions = actions;
      lspCodeActionRevision = revision;
      if (actions.length === 0) {
        appNotice = { tone: "info", message: "No quick fixes are available at the cursor." };
      }
    } catch (reason: unknown) {
      const message = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not load quick fixes: ${message}` };
    } finally {
      lspCodeActionsLoading = false;
    }
  }

  function applyLspCodeAction(action: LspCodeAction) {
    if (!activeEditorFile || !activeEditorHandle) return;
    if (lspCodeActionRevision !== activeEditorFile.revision) {
      activeLspCodeActions = [];
      appNotice = { tone: "info", message: "The document changed. Reload quick fixes first." };
      return;
    }
    if (!activeEditorHandle.applyTextEdits(action.edits)) {
      appNotice = { tone: "error", message: "The quick fix contained invalid overlapping edits." };
      return;
    }
    activeLspCodeActions = [];
    lspCodeActionRevision = null;
    appNotice = { tone: "success", message: `Applied quick fix: ${action.title}` };
  }

  async function stopWorkspaceLspServers() {
    if (!workspace) return;
    const serverIds = Object.entries(lspServerStates)
      .filter(([, state]) => state === "running" || state === "starting")
      .map(([serverId]) => serverId);
    await Promise.all(
      serverIds.map((serverId) => lspStopServer(workspace!.id, serverId).catch(() => false)),
    );
    lspServerStates = {};
    lspStartPromises.clear();
    activeLspDiagnostics = [];
    activeLspSymbols = [];
    activeLspSymbolRevision = null;
    lspSymbolRequestId += 1;
    activeLspReferences = [];
    lspReferenceRequestId += 1;
    lspReferencesLoading = false;
    activeLspStatus = "idle";
  }

  function selectAdjacentEditorTab(direction: -1 | 1) {
    if (openEditorFiles.length < 2) return;
    const currentIndex = openEditorFiles.findIndex((file) => file.path === activeEditorPath);
    const nextIndex =
      (Math.max(currentIndex, 0) + direction + openEditorFiles.length) % openEditorFiles.length;
    selectEditorTab(openEditorFiles[nextIndex].path);
  }

  function executeEditorCommand(command: EditorCommandId) {
    editorCommands.execute(command);
  }

  function toggleEditorVimMode() {
    editorVimMode = !editorVimMode;
    localStorage.setItem(EDITOR_VIM_MODE_KEY, String(editorVimMode));
  }

  async function closeEditorTab(path: string) {
    const target = openEditorFiles.find((file) => file.path === path);
    if (target && !(await resolveDirtyEditors([target], `closing ${target.name}`))) return;
    const lspConfig = lspConfigForPath(path);
    if (workspace && lspConfig && lspServerStates[lspConfig.server_id] === "running") {
      void lspCloseDocument(workspace.id, lspConfig.server_id, path).catch(() => {});
    }
    const index = openEditorFiles.findIndex((file) => file.path === path);
    if (target) aiEditPinnedDocumentIds = aiEditPinnedDocumentIds.filter((id) => id !== target.id);
    openEditorFiles = openEditorFiles.filter((file) => file.path !== path);
    scheduleRecoverySync();
    if (activeEditorPath === path) {
      activeEditorHandle = null;
      activeEditorPath =
        openEditorFiles[Math.max(0, index - 1)]?.path ?? openEditorFiles[0]?.path ?? null;
      activeLspDiagnostics = [];
      activeLspSymbols = [];
      activeLspSymbolRevision = null;
      lspSymbolRequestId += 1;
      activeLspReferences = [];
      lspReferenceRequestId += 1;
      lspReferencesLoading = false;
      if (activeEditorPath) scheduleLspSync(activeEditorPath);
      else activeLspStatus = "idle";
    }
    saveUiState();
  }

  async function saveActiveFile() {
    if (!activeEditorPath) return;
    await saveEditorFile(activeEditorPath);
  }

  async function saveEditorFile(path: string): Promise<boolean> {
    if (!workspace || savingFilePath !== null) return false;
    const file = openEditorFiles.find((entry) => entry.path === path);
    if (!file) return false;
    const snapshot = documentSnapshot(file);
    const workspaceId = workspace.id;
    savingFilePath = path;
    fileError = null;
    try {
      const result = await writeFile(
        workspaceId,
        path,
        snapshot.value,
        file.version || null,
        file.rootRevision,
        file.encoding,
        file.lineEnding,
      );
      const dirty = markDocumentSaved(file, snapshot.value);
      file.version = result.version;
      file.rootRevision = result.root_revision;
      openEditorFiles = openEditorFiles.map((file) =>
        file.path === path
          ? {
              ...file,
              version: result.version,
              rootRevision: result.root_revision,
              dirty,
            }
          : file,
      );
      appNotice = {
        tone: dirty ? "info" : "success",
        message: dirty ? `Saved ${path}; newer edits remain unsaved.` : `Saved ${path}`,
      };
      void syncDirtyRecoverySnapshots();
      if (path === activeEditorPath) await validateActiveEditorSyntax();
      await refreshWorkspaceGitState();
      void refreshFileDirectory(fileDirectory);
      return true;
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not save ${path}: ${fileError}` };
      return false;
    } finally {
      if (savingFilePath === path) savingFilePath = null;
    }
  }

  async function resolveDirtyEditors(files: OpenEditorFile[], action: string): Promise<boolean> {
    const dirtyFiles = files.filter((file) => file.dirty);
    if (dirtyFiles.length === 0) return true;
    const names = dirtyFiles.map((file) => file.name).join(", ");
    const saveFirst = window.confirm(
      `${dirtyFiles.length} file${dirtyFiles.length === 1 ? " has" : "s have"} unsaved changes (${names}). Press OK to save before ${action}, or Cancel to choose whether to discard them.`,
    );
    if (saveFirst) {
      for (const file of dirtyFiles) {
        if (!(await saveEditorFile(file.path))) return false;
      }
      return !openEditorFiles.some((file) =>
        dirtyFiles.some((dirtyFile) => dirtyFile.path === file.path && file.dirty),
      );
    }
    return window.confirm(
      `Discard unsaved changes in ${names} and continue ${action}? Press Cancel to keep editing.`,
    );
  }

  async function formatActiveFile() {
    const editor = activeEditorHandle;
    if (!activeEditorPath || !editor) return;

    const path = activeEditorPath;
    const snapshot = editor.getSnapshot();
    formattingFilePath = path;
    fileError = null;
    try {
      const formatted = await formatEditorText(path, snapshot.value);
      if (editor.getSnapshot().revision !== snapshot.revision) {
        appNotice = {
          tone: "info",
          message: `Formatting for ${path} was cancelled because the document changed.`,
        };
        return;
      }
      editor.setValue(formatted, "format");
      if (path === activeEditorPath) await validateActiveEditorSyntax();
      appNotice = { tone: "success", message: `Formatted ${path}` };
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not format ${path}: ${fileError}` };
    } finally {
      if (formattingFilePath === path) formattingFilePath = null;
    }
  }

  async function requestAiEdit(instruction: string) {
    if (aiEditGenerating) return;
    const file = activeEditorFile;
    const config = buildAiWorkerConfig();
    if (!file || !config) return;
    const requestId = ++aiEditRequestId;
    aiEditGenerating = true;
    aiEditRunId = null;
    aiEditTaskSessionId = null;
    aiEditError = null;
    aiEditProposal = null;
    aiEditSelectedHunkIds = [];
    if (config.runtime === "opencode" && !(await ensureAiWorkspaceTrusted(config))) {
      if (requestId === aiEditRequestId) aiEditGenerating = false;
      return;
    }
    if (activeEditorFile?.id !== file.id) {
      if (requestId === aiEditRequestId) aiEditGenerating = false;
      return;
    }
    const snapshot = documentSnapshot(file);
    const selection = activeEditorHandle?.getSelectionSnapshot() ?? null;
    const diagnostics = activeLspDiagnostics.map((diagnostic) =>
      [
        diagnostic.source,
        diagnostic.code === null ? null : String(diagnostic.code),
        `${diagnostic.message} (line ${diagnostic.range.start.line + 1})`,
      ]
        .filter(Boolean)
        .join(": "),
    );
    const contextFiles = openEditorFiles.filter(
      (entry) => entry.id !== file.id && aiEditPinnedDocumentIds.includes(entry.id),
    );
    const contextSnapshots = contextFiles.map((entry) => ({
      file: entry,
      snapshot: documentSnapshot(entry),
    }));
    const contextRevisions = Object.fromEntries(
      contextSnapshots.map(({ file: entry, snapshot: contextSnapshot }) => [
        entry.id,
        contextSnapshot.revision,
      ]),
    );
    let lastEditEventSequence = 0;
    try {
      const editInput = {
        kind: "edit" as const,
        input: {
          file_path: file.path,
          instruction,
          content: snapshot.value,
          selection,
          context_files: contextSnapshots.map(({ file: entry, snapshot: contextSnapshot }) => ({
            file_path: entry.path,
            content: contextSnapshot.value,
          })),
          diagnostics,
        },
      };
      let result: { run_id: string; summary: string; content: string };
      if (config.runtime === "opencode" && "__TAURI_INTERNALS__" in window) {
        const [profile, rootRevision] = await Promise.all([
          ensureOpenCodePromptProfile(config),
          workspaceRootRevision(config.workspace_id),
        ]);
        const envelope = await createPromptTaskEnvelope(
          {
            workspace_id: config.workspace_id,
            kind: "edit",
            subject_id: file.id,
            conversation_id: null,
            execution_run_id: null,
            runtime_profile_id: profile.runtimeProfileId,
            model: profile.model,
            connector_ids: [],
            requested_capabilities: [],
            prompt_template_version: PROMPT_TASK_TEMPLATE_VERSION,
            context_revision: String(rootRevision),
            rules_revision: profile.rulesRevision,
            skills_revision: profile.skillsRevision,
          },
          editInput,
        );
        const execution = await executePromptTaskSession(
          `Edit ${file.path}`,
          envelope,
          undefined,
          (session) => {
            if (requestId !== aiEditRequestId) {
              void cancelTaskSession(session.id).catch(() => false);
              return;
            }
            aiEditTaskSessionId = session.id;
          },
        );
        if (!("file_path" in execution.result)) {
          throw new Error("Edit Task Session returned an unexpected result kind.");
        }
        result = {
          run_id: "",
          summary: execution.result.summary,
          content: execution.result.content,
        };
      } else {
        const run = await beginAiRun("edit");
        if (requestId !== aiEditRequestId) {
          await cancelAiRun(run.run_id).catch(() => false);
          return;
        }
        aiEditRunId = run.run_id;
        result = await proposeAiEdit(
          config,
          {
            run_id: run.run_id,
            file_path: file.path,
            instruction,
            content: snapshot.value,
            selection,
            context_files: editInput.input.context_files,
            diagnostics,
          },
          (event) => {
            if (event.run_id !== run.run_id || requestId !== aiEditRequestId) return;
            if (event.sequence <= lastEditEventSequence) return;
            lastEditEventSequence = event.sequence;
            if (event.type === "run_failed") {
              aiEditError = "AI Edit generation failed in the backend runtime.";
            } else if (event.type === "run_cancelled") {
              aiEditError = "AI Edit generation was cancelled.";
            }
          },
        );
      }
      if (requestId !== aiEditRequestId) return;
      const proposal = createAiEditProposal({
        documentId: file.id,
        path: file.path,
        baseRevision: snapshot.revision,
        baseValue: snapshot.value,
        proposedValue: result.content,
        summary: result.summary,
        contextRevisions,
      });
      if (proposal.hunks.length === 0) {
        aiEditError = "The model returned no changes.";
        return;
      }
      aiEditProposal = proposal;
      aiEditSelectedHunkIds = proposal.hunks.map((hunk) => hunk.id);
    } catch (reason: unknown) {
      if (requestId !== aiEditRequestId) return;
      const runId = aiEditRunId;
      const taskSessionId = aiEditTaskSessionId;
      aiEditRunId = null;
      aiEditTaskSessionId = null;
      if (runId) void cancelAiRun(runId).catch(() => {});
      if (taskSessionId !== null) void cancelTaskSession(taskSessionId).catch(() => false);
      aiEditError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (requestId === aiEditRequestId) {
        aiEditRunId = null;
        aiEditTaskSessionId = null;
        aiEditGenerating = false;
      }
    }
  }

  function toggleAiEditContext(documentId: string) {
    if (aiEditPinnedDocumentIds.includes(documentId)) {
      aiEditPinnedDocumentIds = aiEditPinnedDocumentIds.filter((id) => id !== documentId);
      return;
    }
    const document = openEditorFiles.find((file) => file.id === documentId);
    if (!document) return;
    if (new TextEncoder().encode(documentSnapshot(document).value).byteLength > 128 * 1024) {
      aiEditError = `${document.name} exceeds the 128 KiB context-file limit.`;
      return;
    }
    if (aiEditPinnedDocumentIds.length >= 8) {
      aiEditError = "AI Edit supports at most 8 pinned context files.";
      return;
    }
    const activeBytes = activeEditorFile
      ? new TextEncoder().encode(documentSnapshot(activeEditorFile).value).byteLength
      : 0;
    const contextBytes = openEditorFiles
      .filter((file) => aiEditPinnedDocumentIds.includes(file.id) || file.id === documentId)
      .filter((file) => file.id !== activeEditorFile?.id)
      .reduce(
        (total, file) => total + new TextEncoder().encode(documentSnapshot(file).value).byteLength,
        0,
      );
    if (activeBytes + contextBytes > 512 * 1024) {
      aiEditError = "Active and pinned AI context exceeds the 512 KiB combined limit.";
      return;
    }
    aiEditPinnedDocumentIds = [...aiEditPinnedDocumentIds, documentId];
    aiEditError = null;
  }

  function cancelAiEdit() {
    aiEditRequestId += 1;
    aiEditGenerating = false;
    aiEditError = null;
    const runId = aiEditRunId;
    const taskSessionId = aiEditTaskSessionId;
    aiEditRunId = null;
    aiEditTaskSessionId = null;
    if (runId) void cancelAiRun(runId).catch(() => {});
    if (taskSessionId !== null) void cancelTaskSession(taskSessionId).catch(() => false);
  }

  function toggleAiEditHunk(id: string) {
    aiEditSelectedHunkIds = aiEditSelectedHunkIds.includes(id)
      ? aiEditSelectedHunkIds.filter((value) => value !== id)
      : [...aiEditSelectedHunkIds, id];
  }

  function applyAiEdit(selectedHunkIds: string[]) {
    const proposal = aiEditProposal;
    const file = activeEditorFile;
    if (
      !proposal ||
      !file ||
      aiEditProposalIsStale(
        proposal,
        file.id,
        file.revision,
        Object.fromEntries(openEditorFiles.map((entry) => [entry.id, entry.revision])),
      )
    ) {
      aiEditError = "The document changed after this proposal was generated. Regenerate it first.";
      return;
    }
    const value = applyAiEditHunks(proposal.baseValue, proposal.hunks, new Set(selectedHunkIds));
    if (value !== proposal.baseValue) {
      if (activeEditorHandle) activeEditorHandle.setValue(value, "ai");
      else replaceDocument(file, value, "ai");
    }
    aiEditProposal = null;
    aiEditSelectedHunkIds = [];
    aiEditError = null;
    appNotice = {
      tone: "success",
      message: `Applied AI edit to ${file.path}. Review and save it.`,
    };
  }

  function rejectAiEdit() {
    aiEditProposal = null;
    aiEditSelectedHunkIds = [];
    aiEditError = null;
  }

  async function reloadActiveFileFromDisk() {
    const file = activeEditorFile;
    if (!workspace || !file || !activeEditorHandle) return;
    if (
      file.dirty &&
      !window.confirm(`Discard local edits in ${file.name} and reload the version from disk?`)
    ) {
      return;
    }
    try {
      const snapshot = await readFile(workspace.id, file.path);
      activeEditorHandle.setValue(snapshot.content, "disk");
      activeEditorHandle.markSaved(snapshot.content);
      file.version = snapshot.version;
      file.rootRevision = snapshot.root_revision;
      file.encoding = snapshot.encoding;
      file.lineEnding = snapshot.line_ending;
      file.externalConflict = false;
      openEditorFiles = openEditorFiles.map((entry) =>
        entry.path === file.path ? { ...file } : entry,
      );
      appNotice = { tone: "success", message: `Reloaded ${file.path} from disk.` };
    } catch (reason: unknown) {
      fileError = reason instanceof Error ? reason.message : String(reason);
      appNotice = { tone: "error", message: `Could not reload ${file.path}: ${fileError}` };
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
    const file = activeEditorFile;
    if (!activeEditorPath || !file) {
      editorDiagnostic = null;
      return;
    }
    const path = activeEditorPath;
    const snapshot = documentSnapshot(file);
    const requestId = ++editorDiagnosticRequestId;
    const diagnostic = await validateEditorSyntax(path, snapshot.value);
    if (
      requestId !== editorDiagnosticRequestId ||
      path !== activeEditorPath ||
      documentSnapshot(file).revision !== snapshot.revision
    ) {
      return;
    }
    editorDiagnostic = diagnostic;
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
      resizePtyTerminal(workspaceTerminalId, workspaceTerminal.rows, workspaceTerminal.cols).catch(
        () => {},
      );
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

  async function loadGlobalEnvironmentRuntime() {
    if (globalEnvironmentHydrated) return;
    globalEnvironmentHydrated = true;
    globalEnvironmentLoading = true;
    try {
      globalEnvironmentVariables = (await listGlobalEnvironmentVariables()).map((env) => ({
        ...env,
        draft: false,
        revealed: false,
        editing: false,
      }));
    } catch (reason) {
      settingsError = `Failed to load global environment: ${reason instanceof Error ? reason.message : String(reason)}`;
    } finally {
      globalEnvironmentLoading = false;
    }
  }

  async function addGlobalEnvironmentVariable() {
    const variable: GlobalEnvironmentDraft = {
      id: `draft-${Date.now().toString(36)}`,
      key: "",
      value: "",
      secret: false,
      enabled: true,
      value_set: false,
      draft: true,
      revealed: false,
      editing: true,
    };
    globalEnvironmentVariables = [variable, ...globalEnvironmentVariables];
  }

  function updateGlobalEnvironmentDraft(id: string, values: Partial<GlobalEnvironmentDraft>) {
    globalEnvironmentVariables = globalEnvironmentVariables.map((env) =>
      env.id === id ? { ...env, ...values } : env,
    );
  }

  async function saveGlobalEnvironmentEntry(variable: GlobalEnvironmentDraft) {
    if (globalEnvironmentLoading) return;
    if (!variable.key.trim()) {
      settingsError = "Environment key is required.";
      return;
    }
    globalEnvironmentLoading = true;
    settingsError = null;
    try {
      const saved = await saveGlobalEnvironmentVariable({
        id: variable.draft ? null : variable.id,
        key: variable.key.trim(),
        value: variable.draft || variable.revealed ? variable.value : undefined,
        secret: variable.secret,
        enabled: variable.enabled,
      });
      globalEnvironmentVariables = globalEnvironmentVariables
        .filter((env) => env.id !== variable.id || !variable.draft)
        .concat({
          ...saved,
          draft: false,
          revealed: false,
          editing: false,
        });
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      globalEnvironmentLoading = false;
    }
  }

  async function revealGlobalEnvironment(id: string) {
    try {
      const value = await revealGlobalEnvironmentVariable(id);
      updateGlobalEnvironmentDraft(id, { value, revealed: true });
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
    }
  }

  function hideGlobalEnvironment(id: string) {
    updateGlobalEnvironmentDraft(id, { value: "", revealed: false });
  }

  async function removeGlobalEnvironment(id: string) {
    if (globalEnvironmentLoading) return;
    const variable = globalEnvironmentVariables.find((env) => env.id === id);
    if (!variable) return;
    if (variable.draft) {
      globalEnvironmentVariables = globalEnvironmentVariables.filter((env) => env.id !== id);
      return;
    }
    globalEnvironmentLoading = true;
    settingsError = null;
    try {
      await deleteGlobalEnvironmentVariable(id);
      globalEnvironmentVariables = globalEnvironmentVariables.filter((env) => env.id !== id);
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      globalEnvironmentLoading = false;
    }
  }

  function normalizeLayoutPrefs(value: Partial<LayoutPrefs>): LayoutPrefs {
    return {
      laneWidth: clampNumber(value.laneWidth, 260, 460, defaultLayoutPrefs.laneWidth),
      cardMinHeight: clampNumber(value.cardMinHeight, 170, 360, defaultLayoutPrefs.cardMinHeight),
      fileSidebarWidth: clampNumber(
        value.fileSidebarWidth,
        240,
        560,
        defaultLayoutPrefs.fileSidebarWidth,
      ),
      terminalWidth: clampNumber(value.terminalWidth, 420, 1100, defaultLayoutPrefs.terminalWidth),
      agentConsoleWidth: clampNumber(
        value.agentConsoleWidth,
        360,
        720,
        defaultLayoutPrefs.agentConsoleWidth,
      ),
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

  function resizeLayout(
    key: keyof LayoutPrefs,
    delta: number,
    min: number,
    max: number,
    invert = false,
  ) {
    layoutPrefs = {
      ...layoutPrefs,
      [key]: clampNumber(
        layoutPrefs[key] + (invert ? -delta : delta),
        min,
        max,
        defaultLayoutPrefs[key],
      ),
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
    resizeLayout(
      layoutResizeDrag.key,
      delta,
      layoutResizeDrag.min,
      layoutResizeDrag.max,
      layoutResizeDrag.invert,
    );
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
          return [
            [
              key,
              {
                connected: entry.connected === true,
                testedAt: Number(entry.testedAt) || 0,
                message: String(entry.message ?? ""),
              },
            ],
          ];
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

  function invalidateMcpConnection(serverId: string) {
    const { [serverId]: _state, ...remainingStates } = mcpConnectionStates;
    const { [serverId]: _tools, ...remainingTools } = mcpToolsByServer;
    mcpConnectionStates = remainingStates;
    mcpToolsByServer = remainingTools;
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
    return state.status === "connected" ? "Test passed" : "Test failed";
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

    const cards = [...column.cards].sort(
      (left, right) => completedSortValue(right) - completedSortValue(left),
    );
    return doneVisibleLimit === "all" ? cards : cards.slice(0, doneVisibleLimit);
  }

  function showMoreLaneCards(column: BoardDisplayColumn) {
    laneVisibleLimits = {
      ...laneVisibleLimits,
      [column.id]: Math.min(
        column.totalCardCount,
        (laneVisibleLimits[column.id] ?? DEFAULT_LANE_VISIBLE_LIMIT) + LANE_VISIBLE_INCREMENT,
      ),
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

    if (!settings.jira.baseUrl.trim() || !hasJiraCredential()) {
      syncError = "Open Settings and fill Jira URL plus the selected credential before syncing.";
      openSettings("jira");
      return null;
    }

    if (settings.jira.authMode !== "pat" && !settings.jira.username.trim()) {
      syncError =
        "Open Settings and fill the Jira email/username required by the selected auth method.";
      openSettings("jira");
      return null;
    }

    return {
      server: {
        command: server?.command ?? "",
        args: server?.args ?? [],
        scope_id: workspace?.id ?? "workspace-personal",
        env: {
          JIRA_URL: settings.jira.baseUrl,
          JIRA_BASE_URL: settings.jira.baseUrl,
          ATLASSIAN_SITE_URL: settings.jira.baseUrl,
          JIRA_USERNAME: settings.jira.username,
          JIRA_EMAIL: settings.jira.username,
          ATLASSIAN_EMAIL: settings.jira.username,
        },
      },
      secret_id: "jira-default",
      auth: {
        base_url: settings.jira.baseUrl,
        auth_mode: settings.jira.authMode,
        username: settings.jira.username,
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
    const hasConfiguredApiKey =
      Boolean(effectiveApiKey.trim()) || aiProviderSecrets[effectiveProvider.id] === true;

    if (effectiveSettings.aiWorker.runtime === "api" && !hasConfiguredApiKey) {
      openSettings("agent");
      appNotice = {
        tone: "error",
        message: `Open Settings and fill ${effectiveProvider.apiKeyLabel}.`,
      };
      return null;
    }

    if (
      effectiveSettings.aiWorker.runtime === "opencode" &&
      !effectiveSettings.aiWorker.opencodeCommand.trim()
    ) {
      openSettings("agent");
      appNotice = { tone: "error", message: "Open Settings and fill the OpenCode command." };
      return null;
    }

    return {
      workspace_id: workspace?.id ?? "workspace-personal",
      runtime: effectiveSettings.aiWorker.runtime,
      provider_name: effectiveProvider.label,
      provider_id: effectiveProvider.id,
      base_url: effectiveProvider.baseUrl,
      api_style: effectiveProvider.apiStyle,
      model:
        effectiveSettings.aiWorker.runtime === "opencode"
          ? effectiveSettings.aiWorker.opencodeModel
          : effectiveModel.id,
      opencode_command: effectiveSettings.aiWorker.opencodeCommand,
      opencode_model: effectiveSettings.aiWorker.opencodeModel,
      opencode_workdir: effectiveSettings.aiWorker.opencodeWorkdir.trim() || null,
      opencode_auto_approve: false,
      agent_rules: effectiveSettings.aiWorker.agentRules,
      agent_skills: effectiveSettings.aiWorker.agentSkills,
      temperature: effectiveSettings.aiWorker.temperature,
      mcp_servers: effectiveSettings.mcpServers.flatMap((server) => {
        const command = [server.command.trim(), ...server.args].filter(Boolean);
        if (command.length === 0) return [];
        return [
          {
            name: `spacesly-${server.id}`,
            secret_id: server.id,
            command,
          },
        ];
      }),
    };
  }

  async function ensureAiWorkspaceTrusted(config: AiWorkerConfig): Promise<boolean> {
    if (config.runtime !== "opencode") return true;
    if (!workspace) return false;
    try {
      const workingDirectory = config.opencode_workdir?.trim() || null;
      const status = await aiWorkspaceTrustStatus(workspace.id, workingDirectory);
      if (status.trusted) return true;
      const confirmed = window.confirm(
        `Trust ${status.path} for AI tool execution? OpenCode agents may read and modify files and run commands inside this workspace.`,
      );
      if (!confirmed) return false;
      await trustAiWorkspace(workspace.id, workingDirectory);
      return true;
    } catch (reason: unknown) {
      appNotice = {
        tone: "error",
        message: reason instanceof Error ? reason.message : String(reason),
      };
      return false;
    }
  }

  async function hydrateDurableConversations(workspaceId: string) {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
    durableConversationWorkspaceId = workspaceId;
    try {
      const records = await listConversations(workspaceId);
      if (records.length === 0) {
        await importConversations(
          workspaceId,
          workspaceChatSessions.slice(0, MAX_CHAT_SESSIONS).map((session) => ({
            id: session.id,
            title: session.title,
            messages: session.messages,
          })),
        );
        void recoverRetainedChatTaskSessions(workspaceId);
        return;
      }

      const retainedRecords = records.slice(0, MAX_CHAT_SESSIONS);
      if (records.length > retainedRecords.length) {
        void pruneConversations(
          workspaceId,
          retainedRecords.map((record) => record.id),
        ).catch(() => {});
      }
      const hydrated = await Promise.all(
        retainedRecords.map(async (record) => {
          const messages = await loadConversationMessages(workspaceId, record.id);
          const existing = workspaceChatSessions.find((session) => session.id === record.id);
          const fallback = existing ?? createWorkspaceChatSession([], record.title);
          return {
            ...fallback,
            id: record.id,
            title: record.title,
            createdAt: record.created_at,
            updatedAt: record.updated_at,
            messages: messages.map(({ id, role, text }) => ({ id, role, text })),
          } satisfies ChatSessionState;
        }),
      );
      if (hydrated.length === 0) return;
      const activeId = workspaceChatActiveSessionId ?? hydrated[0].id;
      const active = hydrated.find((session) => session.id === activeId) ?? hydrated[0];
      workspaceChatSessions = hydrated.slice(0, MAX_CHAT_SESSIONS);
      workspaceChatActiveSessionId = active.id;
      workspaceChatSession = active;
      workspaceChatMessages = active.messages;
      saveUiState();
      void recoverRetainedChatTaskSessions(workspaceId);
    } catch (reason: unknown) {
      durableConversationWorkspaceId = null;
      appNotice = {
        tone: "error",
        message: reason instanceof Error ? reason.message : String(reason),
      };
    }
  }

  async function recoverRetainedChatTaskSessions(workspaceId: string) {
    const sessions = await listTaskSessions().catch(() => []);
    for (const snapshot of sessions) {
      if (recoveringPromptSessionIds.has(snapshot.id)) continue;
      let envelope: import("$lib/ipc/taskSessions").TaskSessionEnvelopeV2;
      try {
        envelope = JSON.parse(snapshot.request.payload);
      } catch {
        continue;
      }
      if (
        envelope.schema_version !== 2 ||
        envelope.session.session.kind !== "chat" ||
        envelope.session.session.workspace_id !== workspaceId
      ) {
        continue;
      }
      const conversationId = envelope.session.session.conversation_id;
      if (typeof conversationId !== "string") continue;
      recoveringPromptSessionIds.add(snapshot.id);
      void waitForPromptTaskSession(snapshot.id, envelope)
        .then(async () => {
          const messages = await loadConversationMessages(workspaceId, conversationId);
          const session = workspaceChatSessions.find((entry) => entry.id === conversationId);
          if (!session) return;
          const updated = {
            ...session,
            updatedAt: Date.now(),
            messages: messages.map(({ id, role, text }) => ({ id, role, text })),
          };
          workspaceChatSessions = [
            updated,
            ...workspaceChatSessions.filter((entry) => entry.id !== updated.id),
          ];
          if (updated.id === workspaceChatActiveSessionId) {
            workspaceChatSession = updated;
            workspaceChatMessages = updated.messages;
          }
          saveUiState();
        })
        .catch(() => {})
        .finally(() => recoveringPromptSessionIds.delete(snapshot.id));
    }
  }

  function appendWorkspaceChat(
    message: Omit<WorkspaceChatMessage, "id">,
    targetSessionId = workspaceChatActiveSessionId,
  ) {
    const session = workspaceChatSessions.find((entry) => entry.id === targetSessionId);
    if (!session) return null;
    const entry: WorkspaceChatMessage = {
      ...message,
      id: `chat-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`,
    };
    const updatedSession = {
      ...session,
      updatedAt: Date.now(),
      messages: capList([...session.messages, entry], MAX_WORKSPACE_CHAT_MESSAGES),
      title:
        isGenericChatTitle(session.title) && message.role === "user"
          ? summarizeChatTitle(message.text)
          : session.title,
    };
    workspaceChatSessions = [
      updatedSession,
      ...workspaceChatSessions.filter((entry) => entry.id !== targetSessionId),
    ].slice(0, MAX_CHAT_SESSIONS);
    if (targetSessionId === workspaceChatActiveSessionId) {
      workspaceChatSession = updatedSession;
      workspaceChatMessages = updatedSession.messages;
    }
    const activityEntry = appendWorkspaceChatActivity(
      {
        kind: "message",
        label: message.role,
        text: message.text,
      },
      targetSessionId,
    );
    saveUiState();
    const persisted =
      workspace?.id && "__TAURI_INTERNALS__" in window && entry.role !== "agent"
        ? appendConversationMessage(workspace.id, targetSessionId, updatedSession.title, {
            id: entry.id,
            role: entry.role,
            text: entry.text,
          }).catch((reason: unknown) => {
            appNotice = {
              tone: "error",
              message: reason instanceof Error ? reason.message : String(reason),
            };
            return null;
          })
        : null;
    scrollWorkspaceChatToLatest();
    return {
      entry,
      persisted,
      activityId: activityEntry?.id ?? null,
      previousTitle: session.title,
    };
  }

  function removeWorkspaceChatMessage(
    messageId: string,
    targetSessionId: string,
    activityId: string | null,
    previousTitle: string,
  ) {
    const session = workspaceChatSessions.find((entry) => entry.id === targetSessionId);
    if (!session) return;
    const updatedSession = {
      ...session,
      updatedAt: Date.now(),
      title: previousTitle,
      messages: session.messages.filter((message) => message.id !== messageId),
      activities: activityId
        ? session.activities.filter((activity) => activity.id !== activityId)
        : session.activities,
    };
    workspaceChatSessions = [
      updatedSession,
      ...workspaceChatSessions.filter((entry) => entry.id !== targetSessionId),
    ].slice(0, MAX_CHAT_SESSIONS);
    if (targetSessionId === workspaceChatActiveSessionId) {
      workspaceChatSession = updatedSession;
      workspaceChatMessages = updatedSession.messages;
    }
    saveUiState();
  }

  function appendWorkspaceChatActivity(
    activity: Omit<WorkspaceChatActivity, "id" | "at">,
    targetSessionId = workspaceChatActiveSessionId,
  ) {
    const session = workspaceChatSessions.find((entry) => entry.id === targetSessionId);
    if (!session) return;
    const entry: WorkspaceChatActivity = {
      ...activity,
      id: `chat-activity-${Date.now().toString(36)}-${session.activities.length}`,
      at: Date.now(),
    };
    const updatedSession = {
      ...session,
      updatedAt: Date.now(),
      activities: capList([...session.activities, entry], MAX_WORKSPACE_CHAT_ACTIVITIES),
    };
    workspaceChatSessions = [
      updatedSession,
      ...workspaceChatSessions.filter((entry) => entry.id !== targetSessionId),
    ].slice(0, MAX_CHAT_SESSIONS);
    if (targetSessionId === workspaceChatActiveSessionId) {
      workspaceChatSession = {
        ...workspaceChatSession,
        ...updatedSession,
      };
    }
    saveUiState();
    return entry;
  }

  function syncWorkspaceChatSession() {
    const normalizedSession = {
      ...workspaceChatSession,
      messages: workspaceChatMessages,
      activities: workspaceChatSession.activities,
    };
    workspaceChatSession = normalizedSession;
    workspaceChatActiveSessionId = normalizedSession.id;
    workspaceChatSessions = [
      normalizedSession,
      ...workspaceChatSessions.filter((session) => session.id !== normalizedSession.id),
    ].slice(0, MAX_CHAT_SESSIONS);
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
    workspaceChatSessions = [session, ...workspaceChatSessions].slice(0, MAX_CHAT_SESSIONS);
    workspaceChatActiveSessionId = session.id;
    workspaceChatSession = session;
    workspaceChatMessages = session.messages;
    saveUiState();
    scrollWorkspaceChatToLatest();
  }

  function setWorkspaceChatSessionCard(
    cardId: string | null,
    options: { created?: boolean } = {},
    targetSessionId = workspaceChatActiveSessionId,
  ) {
    const targetSession = workspaceChatSessions.find((session) => session.id === targetSessionId);
    if (!targetSession) return;
    const card = cardId ? (activeCardById.get(cardId) ?? null) : null;
    const recentCardIds = cardId
      ? capList(
          [cardId, ...targetSession.recentCardIds.filter((entry) => entry !== cardId)],
          MAX_WORKSPACE_CHAT_RECENT_CARDS,
        )
      : targetSession.recentCardIds;
    const updatedSession = {
      ...targetSession,
      updatedAt: Date.now(),
      title:
        card && (options.created || isGenericChatTitle(targetSession.title))
          ? chatTitleForCard(card)
          : targetSession.title,
      lastCardId: cardId ?? targetSession.lastCardId,
      lastCreatedCardId: options.created && cardId ? cardId : targetSession.lastCreatedCardId,
      recentCardIds,
    };
    workspaceChatSessions = [
      updatedSession,
      ...workspaceChatSessions.filter((session) => session.id !== targetSessionId),
    ].slice(0, MAX_CHAT_SESSIONS);
    if (targetSessionId === workspaceChatActiveSessionId) {
      workspaceChatSession = updatedSession;
      workspaceChatMessages = updatedSession.messages;
    }
    saveUiState();
  }

  function workspaceChatActionContext(session = workspaceChatSession): WorkspaceChatActionContext {
    return {
      activeCardIds,
      selectedCardId,
      lastCardId: session.lastCardId,
      lastCreatedCardId: session.lastCreatedCardId,
      recentCardIds: session.recentCardIds,
    };
  }

  function workspaceChatSessionPromptContext(session: ChatSessionState): string {
    const recentMessages = session.messages
      .filter((message) => message.id !== "chat-welcome")
      .slice(-10)
      .map((message) => `${message.role}: ${singleLine(message.text, 500)}`);
    const recentActivities = session.activities
      .slice(-8)
      .map((activity) => `${activity.kind}/${activity.label}: ${singleLine(activity.text, 300)}`);

    return [
      chatSessionContext(workspaceChatActionContext(session)),
      recentMessages.length > 0
        ? ["Recent chat turns:", ...recentMessages].join("\n")
        : "Recent chat turns: none.",
      recentActivities.length > 0
        ? ["Recent tool activity:", ...recentActivities].join("\n")
        : "Recent tool activity: none.",
    ].join("\n\n");
  }

  function workspaceChatRequestContext(): { context: string; revision: string } {
    return workspaceChatRequestContextValue;
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
      if (!workspaceTerminal || !workspaceFitAddon || !workspaceTerminalContainer?.offsetHeight)
        return;
      workspaceFitAddon.fit();
      resizePtyTerminal(workspaceTerminalId, workspaceTerminal.rows, workspaceTerminal.cols).catch(
        () => {},
      );
    });
    workspaceTerminalResizeObserver.observe(workspaceTerminalContainer);

    try {
      await openPtyTerminal(
        workspaceTerminalId,
        workspace?.id ?? null,
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

  async function openDialogIfAvailable(
    options: Parameters<typeof import("@tauri-apps/plugin-dialog").open>[0],
  ) {
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

  function loadWorkspaceSearchRuntime() {
    workspaceSearchRuntime ??= import("$lib/components/WorkspaceSearchPane.svelte").then(
      (module) => {
        workspaceSearchModule = module;
        return module;
      },
    );
    return workspaceSearchRuntime;
  }

  function loadWorkspaceChatRuntime() {
    workspaceChatRuntime ??= import("$lib/components/WorkspaceChatPane.svelte").then((module) => {
      workspaceChatModule = module;
      return module;
    });

    return workspaceChatRuntime;
  }

  function loadMcpConnectionRuntime() {
    mcpConnectionRuntime ??= import("$lib/components/McpConnectionSettings.svelte").then(
      (module) => {
        mcpConnectionModule = module;
        return module;
      },
    );

    return mcpConnectionRuntime;
  }

  function loadAgentConsoleRuntime() {
    agentConsoleRuntime ??= import("$lib/components/AgentConsolePanel.svelte").then((module) => {
      agentConsoleModule = module;
      return module;
    });

    return agentConsoleRuntime;
  }

  function flushWorkspaceChatStream(sessionId: string, generation: number) {
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, sessionId, (run) =>
      run.generation !== generation
        ? run
        : {
            ...run,
            streamFrame: null,
            streamingText: run.streamingText + run.streamBuffer,
            streamBuffer: "",
          },
    );
  }

  function queueWorkspaceChatDelta(sessionId: string, generation: number, delta: string) {
    const run = workspaceChatRunFor(workspaceChatRuns, sessionId);
    if (run.generation !== generation) return;
    const streamFrame =
      run.streamFrame ??
      requestAnimationFrame(() => flushWorkspaceChatStream(sessionId, generation));
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, sessionId, (current) =>
      current.generation === generation
        ? { ...current, streamBuffer: current.streamBuffer + delta, streamFrame }
        : current,
    );
  }

  function resetWorkspaceChatStream(sessionId: string, generation?: number) {
    const run = workspaceChatRunFor(workspaceChatRuns, sessionId);
    if (generation !== undefined && run.generation !== generation) return;
    if (run.streamFrame !== null) cancelAnimationFrame(run.streamFrame);
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, sessionId, (current) => ({
      ...current,
      streamFrame: null,
      streamBuffer: "",
      streamingText: "",
    }));
  }

  async function sendWorkspaceChat() {
    const message = workspaceChatTextarea?.value.trim() ?? "";
    const currentRun = workspaceChatRunFor(workspaceChatRuns, workspaceChatActiveSessionId);
    if (!message || currentRun.running) return;
    const requestSessionId = workspaceChatActiveSessionId;
    const requestSession = workspaceChatSessions.find((session) => session.id === requestSessionId);
    if (!requestSession) return;
    const requestSessionContext = workspaceChatSessionPromptContext(requestSession);
    const requestWorkspaceContext = workspaceChatRequestContext();

    if (workspaceChatTextarea) workspaceChatTextarea.value = "";
    const appendedUser = appendWorkspaceChat({ role: "user", text: message }, requestSessionId);
    focusWorkspaceChatInput();

    const localActions = fastWorkspaceChatActions(
      message,
      workspaceChatActionContext(requestSession),
    );
    if (localActions.length > 0) {
      if (localActions.some(workspaceChatActionRequiresConfirmation)) {
        proposeWorkspaceChatActions(localActions, "command", null, requestSessionId);
        appendWorkspaceChat(
          {
            role: "system",
            text: "Review and approve the proposed workspace actions before they run.",
          },
          requestSessionId,
        );
        focusWorkspaceChatInput();
        return;
      }
      const actionSummary = await applyWorkspaceChatActions(localActions, requestSessionId);
      appendWorkspaceChat({ role: "system", text: actionSummary }, requestSessionId);
      focusWorkspaceChatInput();
      return;
    }

    const config = buildAiWorkerConfig();
    if (!config) return;
    const requestId = currentRun.generation + 1;
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, requestSessionId, (run) => ({
      ...createWorkspaceChatRun(requestId),
      actionProposal: run.actionProposal,
      running: true,
      state: "queued",
    }));

    try {
      if (!(await ensureAiWorkspaceTrusted(config))) return;
      const durableUser = appendedUser?.persisted ? await appendedUser.persisted : null;
      if (!durableUser) {
        if (appendedUser) {
          removeWorkspaceChatMessage(
            appendedUser.entry.id,
            requestSessionId,
            appendedUser.activityId,
            appendedUser.previousTitle,
          );
        }
        throw new Error("Chat message must be durably saved before model execution.");
      }
      let result: { run_id: string; message: string };
      if (config.runtime === "opencode" && "__TAURI_INTERNALS__" in window) {
        const [profile, rootRevision] = await Promise.all([
          ensureOpenCodePromptProfile(config),
          workspaceRootRevision(config.workspace_id),
        ]);
        const chatInput = {
          kind: "chat" as const,
          input: {
            message_id: durableUser.id,
            message_sequence: durableUser.sequence,
            message,
            terminal_context: requestWorkspaceContext.context,
            session_context: requestSessionContext,
          },
        };
        const envelope = await createPromptTaskEnvelope(
          {
            workspace_id: config.workspace_id,
            kind: "chat",
            subject_id: null,
            conversation_id: requestSessionId,
            execution_run_id: null,
            runtime_profile_id: profile.runtimeProfileId,
            model: profile.model,
            connector_ids: [],
            requested_capabilities: [],
            prompt_template_version: PROMPT_TASK_TEMPLATE_VERSION,
            context_revision: String(rootRevision),
            rules_revision: profile.rulesRevision,
            skills_revision: profile.skillsRevision,
          },
          chatInput,
        );
        let streamedAttemptId: number | null = null;
        const execution = await executePromptTaskSession(
          `Chat ${requestSessionId}`,
          envelope,
          (event) => {
            const run = workspaceChatRunFor(workspaceChatRuns, requestSessionId);
            if (requestId !== run.generation) return;
            workspaceChatRuns = updateWorkspaceChatRun(
              workspaceChatRuns,
              requestSessionId,
              (current) => ({
                ...current,
                state:
                  event.kind === "lifecycle" &&
                  typeof event.payload === "object" &&
                  event.payload !== null &&
                  !Array.isArray(event.payload) &&
                  typeof (event.payload as Record<string, unknown>).state === "string"
                    ? ((event.payload as Record<string, unknown>).state as typeof current.state)
                    : current.state,
                progress: event.progress ?? current.progress,
              }),
            );
            if (event.kind !== "runtime") return;
            if (event.attempt_id === null) return;
            if (streamedAttemptId !== event.attempt_id) {
              resetWorkspaceChatStream(requestSessionId, requestId);
              streamedAttemptId = event.attempt_id;
            }
            const payload =
              typeof event.payload === "object" &&
              event.payload !== null &&
              !Array.isArray(event.payload)
                ? (event.payload as Record<string, unknown>)
                : null;
            if (
              payload !== null &&
              payload.type === "text_delta" &&
              typeof payload.text === "string"
            ) {
              queueWorkspaceChatDelta(requestSessionId, requestId, payload.text);
            }
          },
          (session) => {
            if (requestId !== workspaceChatRunFor(workspaceChatRuns, requestSessionId).generation) {
              void cancelTaskSession(session.id).catch(() => false);
              return;
            }
            workspaceChatRuns = updateWorkspaceChatRun(
              workspaceChatRuns,
              requestSessionId,
              (run) => ({ ...run, taskSessionId: session.id, state: session.state }),
            );
          },
        );
        if (!("conversation_id" in execution.result)) {
          throw new Error("Chat Task Session returned an unexpected result kind.");
        }
        result = { run_id: "", message: execution.result.message };
      } else {
        const run = await beginAiRun("chat");
        if (requestId !== workspaceChatRunFor(workspaceChatRuns, requestSessionId).generation) {
          await cancelAiRun(run.run_id).catch(() => false);
          return;
        }
        workspaceChatRuns = updateWorkspaceChatRun(
          workspaceChatRuns,
          requestSessionId,
          (current) => ({ ...current, legacyRunId: run.run_id, state: "running" }),
        );
        result = await chatAiWorker(
          config,
          {
            run_id: run.run_id,
            conversation_id: requestSessionId,
            message_id: durableUser.id,
            message_sequence: durableUser.sequence,
            message,
            terminal_context: null,
            context_revision: null,
            session_context: null,
            session_key: `chat:${requestSessionId}`,
          },
          (event) => {
            const current = workspaceChatRunFor(workspaceChatRuns, requestSessionId);
            if (event.run_id !== run.run_id || requestId !== current.generation) return;
            if (event.sequence <= current.lastEventSequence) return;
            workspaceChatRuns = updateWorkspaceChatRun(
              workspaceChatRuns,
              requestSessionId,
              (state) => ({ ...state, lastEventSequence: event.sequence }),
            );
            if (event.type === "text_delta") {
              queueWorkspaceChatDelta(requestSessionId, requestId, event.delta);
            }
          },
        );
      }
      if (requestId !== workspaceChatRunFor(workspaceChatRuns, requestSessionId).generation) return;
      const response = result.message;
      const actions = extractWorkspaceActions(response);
      if (workspace?.id) {
        const messages = await loadConversationMessages(workspace.id, requestSessionId);
        const session = workspaceChatSessions.find((entry) => entry.id === requestSessionId);
        if (session) {
          const updated = {
            ...session,
            updatedAt: Date.now(),
            messages: messages.map(({ id, role, text }) => ({ id, role, text })),
          };
          workspaceChatSessions = [
            updated,
            ...workspaceChatSessions.filter((entry) => entry.id !== requestSessionId),
          ];
          if (requestSessionId === workspaceChatActiveSessionId) {
            workspaceChatSession = updated;
            workspaceChatMessages = updated.messages;
          }
        }
      }
      if (requestId !== workspaceChatRunFor(workspaceChatRuns, requestSessionId).generation) return;
      if (actions.length > 0) {
        if (actions.some(workspaceChatActionRequiresConfirmation)) {
          proposeWorkspaceChatActions(actions, "model", result.run_id || null, requestSessionId);
          appendWorkspaceChat(
            {
              role: "system",
              text: "The AI proposed workspace mutations. Review them before applying.",
            },
            requestSessionId,
          );
        } else {
          const actionSummary = await applyWorkspaceChatActions(actions, requestSessionId);
          appendWorkspaceChat({ role: "system", text: actionSummary }, requestSessionId);
        }
      }
    } catch (reason) {
      const current = workspaceChatRunFor(workspaceChatRuns, requestSessionId);
      if (requestId !== current.generation) return;
      const runId = current.legacyRunId;
      const taskSessionId = current.taskSessionId;
      if (runId) void cancelAiRun(runId).catch(() => {});
      if (taskSessionId !== null) void cancelTaskSession(taskSessionId).catch(() => false);
      appendWorkspaceChat(
        {
          role: "system",
          text: reason instanceof Error ? reason.message : String(reason),
        },
        requestSessionId,
      );
      workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, requestSessionId, (run) => ({
        ...run,
        error: reason instanceof Error ? reason.message : String(reason),
        state: "failed",
      }));
    } finally {
      if (requestId === workspaceChatRunFor(workspaceChatRuns, requestSessionId).generation) {
        resetWorkspaceChatStream(requestSessionId, requestId);
        workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, requestSessionId, (run) => ({
          ...run,
          running: false,
          legacyRunId: null,
          taskSessionId: null,
          lastEventSequence: 0,
          state: run.error ? "failed" : "succeeded",
        }));
        if (requestSessionId === workspaceChatActiveSessionId) focusWorkspaceChatInput();
      }
    }
  }

  function cancelWorkspaceChat() {
    const sessionId = workspaceChatActiveSessionId;
    const run = workspaceChatRunFor(workspaceChatRuns, sessionId);
    if (run.streamFrame !== null) cancelAnimationFrame(run.streamFrame);
    const runId = run.legacyRunId;
    const taskSessionId = run.taskSessionId;
    workspaceChatRuns = cancelWorkspaceChatRun(workspaceChatRuns, sessionId);
    const cancellationGeneration = workspaceChatRunFor(workspaceChatRuns, sessionId).generation;
    const settleCancellation = (terminalState: "cancelled" | "failed" | "succeeded" | null) => {
      workspaceChatRuns = settleWorkspaceChatCancellation(
        workspaceChatRuns,
        sessionId,
        cancellationGeneration,
        terminalState,
      );
    };
    if (runId)
      void confirmLegacyWorkspaceChatCancellation(runId, {
        cancel: cancelAiRun,
        getRun: getAiRun,
      })
        .then(settleCancellation)
        .catch(() => settleCancellation(null));
    if (taskSessionId !== null) {
      void cancelTaskSession(taskSessionId)
        .then(async (accepted) => {
          if (!accepted) return null;
          const deadline = Date.now() + 5_000;
          while (Date.now() < deadline) {
            const snapshot = await getTaskSession(taskSessionId);
            if (!snapshot) return null;
            if (snapshot.state === "cancelled") return "cancelled" as const;
            if (["failed", "blocked", "succeeded"].includes(snapshot.state)) {
              return "failed" as const;
            }
            await new Promise((resolve) => setTimeout(resolve, 50));
          }
          return null;
        })
        .then(settleCancellation)
        .catch(() => settleCancellation(null));
    }
    if (!runId && taskSessionId === null) settleCancellation("cancelled");
    focusWorkspaceChatInput();
  }

  function proposeWorkspaceChatActions(
    actions: WorkspaceChatAction[],
    source: WorkspaceChatActionProposal["source"],
    runId: string | null,
    sessionId = workspaceChatActiveSessionId,
  ) {
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, sessionId, (run) => ({
      ...run,
      actionProposal: { id: crypto.randomUUID(), sessionId, runId, source, actions },
    }));
  }

  async function applyWorkspaceChatActionProposal() {
    const proposal = activeWorkspaceChatRun.actionProposal;
    if (!proposal || proposal.sessionId !== workspaceChatSession.id) return;
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, proposal.sessionId, (run) => ({
      ...run,
      actionProposal: null,
    }));
    const actionSummary = await applyWorkspaceChatActions(proposal.actions, proposal.sessionId);
    appendWorkspaceChat({ role: "system", text: actionSummary }, proposal.sessionId);
  }

  function rejectWorkspaceChatActionProposal() {
    const proposal = activeWorkspaceChatRun.actionProposal;
    if (!proposal || proposal.sessionId !== workspaceChatSession.id) return;
    workspaceChatRuns = updateWorkspaceChatRun(workspaceChatRuns, proposal.sessionId, (run) => ({
      ...run,
      actionProposal: null,
    }));
    appendWorkspaceChat(
      { role: "system", text: "Proposed workspace actions were rejected." },
      proposal.sessionId,
    );
  }

  async function applyWorkspaceChatActions(
    actions: WorkspaceChatAction[],
    targetSessionId = workspaceChatActiveSessionId,
  ): Promise<string> {
    const results: string[] = [];
    let pendingAgentStarts: Promise<void>[] = [];

    const flushAgentStarts = async () => {
      if (pendingAgentStarts.length === 0) return;
      const starts = pendingAgentStarts;
      pendingAgentStarts = [];
      await Promise.allSettled(starts);
    };

    for (const action of actions.slice(0, 5)) {
      if (action.type !== "start_agent") await flushAgentStarts();
      if (action.type === "create_task") {
        const card = createBoardTask(
          action.title,
          action.description ?? "Created by Spacesly Agent chat.",
        );
        if (card) {
          setWorkspaceChatSessionCard(card.id, { created: true }, targetSessionId);
          appendWorkspaceChatActivity(
            {
              kind: "tool",
              label: "create_task",
              text: `Created ${ticketLabel(card)}: ${card.title}`,
              cardId: card.id,
            },
            targetSessionId,
          );
          results.push(
            `Created local task "${card.title}" in Todo. Queue it or start the Agent when ready.`,
          );
        } else {
          results.push(`Could not create task "${action.title}".`);
        }
        continue;
      }

      if (action.type === "sync_jira") {
        await syncJira();
        appendWorkspaceChatActivity(
          {
            kind: "tool",
            label: "sync_jira",
            text: "Requested Jira sync from chat.",
          },
          targetSessionId,
        );
        results.push(
          "Jira sync requested. Watch the board notice for fetched card count or errors.",
        );
        continue;
      }

      const card = resolveActionCard(action, targetSessionId);
      if (!card) {
        results.push(`Could not find the requested card for ${action.type}.`);
        continue;
      }

      if (action.type === "select_card") {
        selectCard(card);
        setWorkspaceChatSessionCard(card.id, {}, targetSessionId);
        setWorkspaceMode("board");
        appendWorkspaceChatActivity(
          {
            kind: "tool",
            label: "select_card",
            text: `Opened ${ticketLabel(card)}: ${card.title}`,
            cardId: card.id,
          },
          targetSessionId,
        );
        results.push(
          `Opened ${ticketLabel(card)}: "${card.title}". Current state: ${executionDetail(card.execution)}.`,
        );
        continue;
      }

      if (action.type === "delete_card") {
        const removed = removeCard(card.id);
        appendWorkspaceChatActivity(
          {
            kind: "tool",
            label: "delete_card",
            text: `${removed ? "Removed" : "Failed to remove"} ${ticketLabel(card)}: ${card.title}`,
            cardId: card.id,
          },
          targetSessionId,
        );
        results.push(
          removed
            ? `Removed ${ticketLabel(card)} from Spacesly.`
            : `Could not remove ${ticketLabel(card)}.`,
        );
        continue;
      }

      if (action.type === "start_agent") {
        setWorkspaceChatSessionCard(card.id, {}, targetSessionId);
        appendWorkspaceChatActivity(
          {
            kind: "tool",
            label: "start_agent",
            text: `Requested Agent start for ${ticketLabel(card)}: ${card.title}`,
            cardId: card.id,
          },
          targetSessionId,
        );
        pendingAgentStarts.push(startWorkerForCard(card.id, true));
        results.push(
          `Agent start requested for ${ticketLabel(card)}: "${card.title}". Open Agent Console from the board toolbar when you need run details.`,
        );
        continue;
      }

      if (action.type === "move_card") {
        const columnId = columnIdForChatTarget(action.target);
        if (!columnId) {
          results.push(`Could not find ${action.target} column.`);
          continue;
        }
        await moveCardAndSync(card.id, columnId);
        setWorkspaceChatSessionCard(card.id, {}, targetSessionId);
        appendWorkspaceChatActivity(
          {
            kind: "tool",
            label: "move_card",
            text: `Moved ${ticketLabel(card)} to ${chatTargetLabel(action.target)}.`,
            cardId: card.id,
          },
          targetSessionId,
        );
        results.push(
          `Moved ${ticketLabel(card)} to ${chatTargetLabel(action.target)}. Jira write-back is attempted only for In Progress and Done.`,
        );
      }
    }

    await flushAgentStarts();

    return results.length > 0
      ? `Command result: ${results.join(" ")}`
      : "Command result: no board actions were applied.";
  }

  function columnIdForChatTarget(
    target: "todo" | "queued" | "in_progress" | "done",
  ): string | null {
    const intent: ColumnIntent = target === "todo" ? "backlog" : target;
    return activeColumnByIntent.get(intent)?.id ?? null;
  }

  function hasJiraCredential(): boolean {
    if (settings.jira.authMode === "pat") {
      return (
        Boolean(appSecrets.jira_personal_access_token.trim()) || jiraSecrets.personal_access_token
      );
    }
    if (settings.jira.authMode === "password") {
      return Boolean(appSecrets.jira_password.trim()) || jiraSecrets.password;
    }
    return Boolean(appSecrets.jira_api_token.trim()) || jiraSecrets.api_token;
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
      appNotice = {
        tone: "error",
        message: "Choose a Jira board first. Open Settings and click Connect Jira.",
      };
      openSettings("jira");
      return;
    }

    syncing = true;
    syncError = null;
    appNotice = {
      tone: "info",
      message: "Refreshing from Jira. Saved cards stay visible if Jira is slow.",
    };
    await tick();

    try {
      const projection = mergeSyncedWorkspace(
        applyConfiguredBoardName(await syncJiraWorkspace(config)),
        activeBoard?.columns,
        LEGACY_SEED_CARD_ID,
        SYNC_RETAIN_MISSING_CARD_MS,
        locallyDeletedCachedCardIds(),
      );
      workspace = projection;
      cacheSavedAt = Date.now();
      saveCachedWorkspace(projection);
      const syncedCards =
        projection.projects[0]?.boards[0]?.columns.reduce(
          (count, column) =>
            count +
            column.cards.reduce((total, card) => total + (card.source !== "local" ? 1 : 0), 0),
          0,
        ) ?? 0;
      appNotice = {
        tone: syncedCards > 0 ? "success" : "info",
        message:
          syncedCards > 0
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

  async function restoreDeletedJiraCards() {
    if (deletedJiraCardCount === 0) return;
    const restored = restoreLocallyDeletedCachedCards();
    deletedJiraCardCount = 0;
    if (workspace) saveCachedWorkspace(workspace);
    appNotice = {
      tone: "info",
      message: `Restored ${restored} deleted Jira card tombstone${restored === 1 ? "" : "s"}. Refreshing Jira...`,
    };
    await syncJira();
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
      connectionMessage =
        status.board_count > 0 || status.issue_count > 0
          ? `Jira test passed. Found ${status.board_count} board${status.board_count === 1 ? "" : "s"} and ${status.issue_count} sample ticket${status.issue_count === 1 ? "" : "s"}.`
          : `MCP test passed with ${status.tool_count} tools, but Jira returned no boards or tickets yet. Try Connect Jira, a project key, or a board name.`;
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
    if (selectedServer.kind !== "jira" && Object.keys(selectedServer.env).length > 0) {
      await saveMcpEnvironmentSecret(
        serverId,
        selectedServer.command,
        selectedServer.args,
        selectedServer.env,
      );
      mcpEnvironmentSecrets = await mcpEnvironmentSecretStatuses();
    }

    const serverConfig =
      selectedServer.kind === "jira"
        ? buildJiraConfig()?.server
        : {
            command: selectedServer.command,
            args: selectedServer.args,
            scope_id: workspace?.id ?? "workspace-personal",
            env: {},
            secret_id: serverId,
          };
    if (!serverConfig) return;

    testingConnection = true;
    connectionMessage = null;
    settingsError = null;

    try {
      const status = await testMcpServerConnection(serverConfig);
      rememberMcpTools(serverId, status.tools);
      connectionMessage = `MCP test passed. ${status.tool_count} tool${status.tool_count === 1 ? "" : "s"} available.`;
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

  async function disconnectSelectedMcpServer(): Promise<boolean> {
    if (!selectedServer) return false;
    const serverId = selectedServer.id;
    if (selectedServer.kind !== "jira" && Object.keys(selectedServer.env).length > 0) {
      await saveMcpEnvironmentSecret(
        serverId,
        selectedServer.command,
        selectedServer.args,
        selectedServer.env,
      );
      mcpEnvironmentSecrets = await mcpEnvironmentSecretStatuses();
    }
    const serverConfig =
      selectedServer.kind === "jira"
        ? buildJiraConfig()?.server
        : {
            command: selectedServer.command,
            args: selectedServer.args,
            scope_id: workspace?.id ?? "workspace-personal",
            env: {},
            secret_id: serverId,
          };
    if (!serverConfig) return false;

    try {
      await disconnectMcpServer(serverConfig);
      invalidateMcpConnection(serverId);
      connectionMessage = "MCP server disconnected.";
      return true;
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
      return false;
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
        connectionMessage =
          "Connected to Jira, but no boards were returned for this account/filter.";
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

  async function removeSelectedServer() {
    if (!selectedServer) return;
    const serverId = selectedServer.id;

    settingsError = null;
    await disconnectSelectedMcpServer();
    try {
      await removeMcpConnector(serverId);
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
      return;
    }
    settingsError = null;
    invalidateMcpConnection(serverId);

    const remaining = settings.mcpServers.filter((server) => server.id !== serverId);
    const nextServer = remaining[0];
    settings = {
      ...settings,
      mcpServers: remaining,
      jira: { ...settings.jira, serverId: nextServer?.id ?? "" },
    };
    selectedServerId = nextServer?.id ?? "";
  }

  function updateSelectedServer(
    values: Partial<{
      name: string;
      kind: AppSettings["mcpServers"][number]["kind"];
      command: string;
      args: string[];
      env: Record<string, string>;
    }>,
  ) {
    if (!selectedServer) return;

    const serverId = selectedServer.id;
    if ("env" in values) mcpEnvEditedServerIds.add(serverId);
    invalidateMcpConnection(serverId);
    settings = {
      ...settings,
      mcpServers: settings.mcpServers.map((server) =>
        server.id === serverId ? { ...server, ...values } : server,
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

  function updateActiveBoard(transform: (board: BoardProjection) => BoardProjection): boolean {
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

  function moveCard(
    cardId: string,
    targetColumnId: string,
    execution?: ExecutionState,
    announce = true,
  ) {
    const movedCard = activeCardById.get(cardId);
    if (!movedCard) return;

    const targetColumn = activeColumnById.get(targetColumnId);
    if (!targetColumn) return;

    const targetIntent = targetColumn.intent;
    const cardForTarget = {
      ...(targetIntent ? withCompletionMetadata(movedCard, targetIntent) : movedCard),
      ...(execution !== undefined ? { execution } : {}),
    };

    if (
      !updateActiveBoard((board) => ({
        ...board,
        columns: board.columns.map((column) => {
          const cards = column.cards.filter((card) => card.id !== cardId);
          return column.id === targetColumnId
            ? { ...column, cards: [...cards, cardForTarget] }
            : { ...column, cards };
        }),
      }))
    )
      return;

    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
    if (announce) appNotice = { tone: "info", message: "Card moved in Spacesly." };
  }

  function executionForColumn(columnId: string): ExecutionState | null {
    const intent = activeColumnById.get(columnId)?.intent;
    if (intent === "backlog") return "idle";
    if (intent === "queued") return "queued";
    if (intent === "in_progress") return "running";
    if (intent === "done") return { completed: { summary: "Marked Done manually." } };
    return null;
  }

  function removeCard(cardId: string): boolean {
    const card = activeCardById.get(cardId);
    if (!card || card.execution === "running") {
      appNotice = { tone: "error", message: "Running cards cannot be removed." };
      return false;
    }

    if (
      !updateActiveBoard((board) => ({
        ...board,
        columns: board.columns.map((column) => ({
          ...column,
          cards: column.cards.filter((entry) => entry.id !== cardId),
        })),
      }))
    )
      return false;

    if (card.source !== "local") {
      locallyDeleteCachedCard(cardId);
      deletedJiraCardCount = locallyDeletedCachedCardIds().length;
    }
    if (selectedCardId === cardId) selectedCardId = null;
    const { [cardId]: _removed, ...remainingSessions } = agentRunSessions;
    agentRunSessions = remainingSessions;
    if (latestAgentSessionId === cardId) {
      latestAgentSessionId =
        Object.values(remainingSessions).sort(
          (left, right) => sessionActivityAt(right) - sessionActivityAt(left),
        )[0]?.cardId ?? null;
    }
    if (agentConsoleCardId === cardId) {
      const fallback = Object.values(remainingSessions)[0] ?? null;
      agentConsoleCardId = fallback?.cardId ?? null;
      if (!fallback) agentConsoleOpen = false;
    }
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
    appNotice = {
      tone: "success",
      message:
        card.source === "local"
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

    if (
      !updateActiveBoard((board) => ({
        ...board,
        columns: board.columns.map((column) =>
          column.intent === "backlog" ? { ...column, cards: [...column.cards, card] } : column,
        ),
      }))
    )
      return null;
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
    appNotice = {
      tone: "success",
      message: "Task created. Queue it or click Start when you want the Agent to run it.",
    };
  }

  function beginAgentRun(
    card: CardProjection,
    continuation = false,
    gitSnapshot: AgentRunGitSnapshot | null = gitSnapshotFromInfo(workspaceGitInfo),
  ) {
    const previousSession = agentRunSessions[card.id];
    agentConsoleCardId = card.id;
    const session = createAgentRunSession(
      card.id,
      card.title,
      "running",
      continuation ? Math.max(previousSession?.progress ?? 0, 20) : 5,
      continuation && previousSession?.output
        ? previousSession.output
        : "Waiting for Agent output...",
      continuation && previousSession?.result ? previousSession.result : null,
      continuation && previousSession ? previousSession.logs : [],
      continuation && previousSession
        ? previousSession.terminalLines
        : [
            {
              id: `term-${Date.now().toString(36)}`,
              prompt: "system",
              text: "Agent execution session opened. Use the input below for approvals, constraints, or operator notes.",
            },
          ],
      continuation && previousSession ? previousSession.gitSnapshot : gitSnapshot,
      continuation && previousSession
        ? (previousSession.transcript ?? [])
        : [
            createAgentSessionEvent(
              "system",
              "Agent execution session opened. Use the input below for approvals, constraints, or operator notes.",
            ),
          ],
      continuation && previousSession ? (previousSession.executionRun ?? null) : null,
      null,
      continuation ? (previousSession?.conversationId ?? null) : null,
    );
    agentRunSessions = retainAgentSessions({ ...agentRunSessions, [card.id]: session });
    latestAgentSessionId = card.id;
    appendStructuredAgentLogForCard(
      card.id,
      "info",
      continuation ? "continue" : "start",
      continuation
        ? `Agent continuation started for ${ticketLabel(card)}.`
        : `Agent run started for ${ticketLabel(card)}.`,
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

  function agentSessionForCard(cardId: string): AgentRunSession | null {
    return agentRunSessions[cardId] ?? null;
  }

  function updateAgentSessionForCard(
    cardId: string,
    transform: (session: AgentRunSession) => AgentRunSession,
  ) {
    const session = agentSessionForCard(cardId);
    if (!session) return;

    const nextSession = transform(session);
    latestAgentSessionId = cardId;
    agentRunSessions[cardId] = nextSession;
    agentRunSessions = retainAgentSessions(agentRunSessions);
  }

  function retainAgentSessions(
    sessions: Record<string, AgentRunSession>,
  ): Record<string, AgentRunSession> {
    const entries = Object.entries(sessions);
    if (entries.length <= MAX_RETAINED_AGENT_SESSIONS) return sessions;

    const candidates = entries
      .filter(([cardId, session]) => session.status !== "running" && cardId !== agentConsoleCardId)
      .sort(([, left], [, right]) => sessionActivityAt(left) - sessionActivityAt(right));

    const retained = { ...sessions };
    for (const [cardId] of candidates) {
      if (Object.keys(retained).length <= MAX_RETAINED_AGENT_SESSIONS) break;
      delete retained[cardId];
    }
    return retained;
  }

  function sessionActivityAt(session: AgentRunSession): number {
    return session.transcript.at(-1)?.at ?? 0;
  }

  function appendAgentSessionTranscriptForCard(
    cardId: string,
    type: AgentSessionEvent["type"],
    text: string,
  ) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      transcript: appendAgentSessionEvent(
        session.transcript ?? [],
        createAgentSessionEvent(type, text),
        MAX_AGENT_SESSION_EVENTS,
      ),
    }));
  }

  function openAgentRunForCard(card: CardProjection) {
    const session = agentRunSessions[card.id];
    if (!session) {
      appNotice = {
        tone: "info",
        message: "This card does not have an Agent terminal session yet.",
      };
      return;
    }

    agentConsoleOpen = true;
    agentConsoleCardId = session.cardId;
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
      return;
    }

    appNotice = { tone: "info", message: "No Agent console session is available yet." };
  }

  function selectCard(card: CardProjection) {
    selectedCardId = card.id;
  }

  function setAgentRunStatusForCard(cardId: string, status: AgentRunStatus) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, status }));
  }

  function setAgentRunOutputForCard(cardId: string, output: string) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      output: capText(output, MAX_AGENT_OUTPUT_CHARS),
    }));
  }

  function setAgentRunResultForCard(cardId: string, result: AiWorkerTaskResult | null) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, result }));
  }

  async function setExecutionRunForCard(cardId: string, executionRun: ExecutionRun) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, executionRun }));
    try {
      const persisted = await saveExecutionRun(executionRun);
      updateAgentSessionForCard(cardId, (session) => ({
        ...session,
        executionRun: persisted,
      }));
    } catch (reason) {
      appNotice = {
        tone: "error",
        message: `Execution state could not be persisted: ${reason instanceof Error ? reason.message : String(reason)}`,
      };
      throw reason;
    }
  }

  async function persistExecutionRunUpdateForCard(
    cardId: string,
    transform: (run: ExecutionRun) => ExecutionRun,
  ) {
    const run = agentSessionForCard(cardId)?.executionRun;
    if (!run) throw new Error("Execution run is unavailable for workflow checkpointing.");
    const next = transform(run);
    await setExecutionRunForCard(cardId, next);
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      workflowCheckpoint: agentWorkflowCheckpoint(next),
    }));
  }

  async function watchRecoveredAgentTask(cardId: string, sessionId: number) {
    try {
      const execution = await waitForAgentTaskSession(sessionId, {
        onEvent: (event) => projectTaskSessionEvent(cardId, event),
      });
      const run = agentSessionForCard(cardId)?.executionRun;
      const checkpoint = run ? agentWorkflowCheckpoint(run) : "agent_result_committed";
      updateAgentSessionForCard(cardId, (session) => ({
        ...session,
        status: "blocked_for_resume",
        progress: Math.max(session.progress, 75),
        output: agentResultText(execution.result),
        result: execution.result,
        taskSessionId: sessionId,
        taskSessionState: execution.session.state,
        workflowCheckpoint: checkpoint,
      }));
      appendStructuredAgentLogForCard(
        cardId,
        "info",
        "recovery",
        `Authoritative Agent result restored at ${checkpoint}.`,
        [`Task Session: ${sessionId}`, `Terminal state: ${execution.session.state}`],
        ["No verification or Jira side effect was repeated automatically."],
        ["Use Continue to resume from the displayed durable checkpoint."],
      );
    } catch (reason) {
      const snapshot = await getTaskSession(sessionId).catch(() => null);
      if (snapshot && !taskSessionIsTerminal(snapshot)) return;
      appendStructuredAgentLogForCard(
        cardId,
        "error",
        "recovery",
        reason instanceof Error ? reason.message : String(reason),
        [`Task Session: ${sessionId}`],
        [],
        ["Review the retained Task Session before continuing."],
      );
    }
  }

  function updateExecutionRunForCard(
    cardId: string,
    transform: (run: ExecutionRun) => ExecutionRun,
  ) {
    const session = agentSessionForCard(cardId);
    const currentRun = session?.executionRun;
    if (!currentRun) return;
    const nextRun = transform(currentRun);
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      executionRun: nextRun,
    }));
    void saveExecutionRun(nextRun).catch((reason) => {
      appNotice = {
        tone: "error",
        message: `Execution state could not be persisted: ${reason instanceof Error ? reason.message : String(reason)}`,
      };
    });
  }

  function setAgentRunGitSnapshotForCard(cardId: string, snapshot: AgentRunGitSnapshot | null) {
    updateAgentSessionForCard(cardId, (session) => ({ ...session, gitSnapshot: snapshot }));
  }

  function setAgentProgressForCard(cardId: string, value: number) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      progress: Math.max(session.progress, Math.min(100, value)),
    }));
  }

  function appendAgentLogForCard(
    cardId: string,
    tone: AgentRunLog["tone"],
    label: string,
    message: string,
  ) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      logs: capList(
        [
          ...session.logs,
          {
            id: `run-${Date.now().toString(36)}-${session.logs.length}`,
            at: new Date().toLocaleTimeString(undefined, {
              hour: "2-digit",
              minute: "2-digit",
              second: "2-digit",
            }),
            tone,
            label,
            message,
          },
        ],
        MAX_AGENT_LOGS,
      ),
    }));
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

  function projectTaskSessionEvent(cardId: string, event: TaskSessionEvent) {
    const payload =
      typeof event.payload === "object" && event.payload !== null && !Array.isArray(event.payload)
        ? (event.payload as Record<string, unknown>)
        : {};
    const eventType = typeof payload.type === "string" ? payload.type : event.kind;
    if (event.progress) {
      const { completed, total } = event.progress;
      const progress = total && total > 0 ? 35 + Math.round((completed / total) * 35) : 55;
      setAgentProgressForCard(cardId, progress);
    }
    if (event.kind === "runtime" && eventType === "text_delta") return;

    let tone: AgentRunLog["tone"] = "info";
    let summary = `Task Session ${event.kind}: ${eventType}.`;
    if (event.kind === "lifecycle" && typeof payload.state === "string") {
      updateAgentSessionForCard(cardId, (session) => ({
        ...session,
        taskSessionState: payload.state as NonNullable<AgentRunSession["taskSessionState"]>,
      }));
      tone = payload.state === "failed" || payload.state === "blocked" ? "error" : "info";
      summary = `Task Session entered ${payload.state}.`;
    } else if (event.kind === "tool") {
      const context =
        typeof payload.display_context === "object" && payload.display_context !== null
          ? (payload.display_context as Record<string, unknown>)
          : {};
      const label = typeof context.label === "string" ? context.label : payload.tool_name;
      const failed = payload.type === "tool_completed" && payload.success === false;
      tone = failed ? "error" : "info";
      summary = `${payload.type === "tool_completed" ? (failed ? "Failed" : "Completed") : "Started"}: ${String(label ?? "Agent tool")}.`;
    } else if (event.kind === "runtime" && eventType === "agent_result_candidate") {
      summary = "Agent result staged for authoritative Task Session commit.";
    }
    appendStructuredAgentLogForCard(
      cardId,
      tone,
      event.kind,
      summary,
      [
        `Task Session event sequence: ${event.sequence}`,
        `Assignment attempt: ${event.attempt_id ?? "unassigned"}`,
      ],
      event.progress ? [`Progress phase: ${event.progress.phase}`] : [],
      [],
    );
  }

  function buildAgentContextExport(config: AiWorkerConfig, contract: ExecutionContract): string {
    const runtimeLabel =
      config.runtime === "opencode"
        ? `OpenCode ${config.opencode_model}`
        : `${config.provider_name} ${config.model}`;
    const description = contract.task_context.description.trim();
    const clippedDescription =
      description.length > 320 ? `${description.slice(0, 320)}…` : description;
    const operatorNotes = contract.runtime_inputs.operator_notes;
    const issueKey = contract.ticket.key;
    const previousOutput = contract.runtime_inputs.previous_output;
    const transcript = previousOutput?.trim() ? previousOutput : null;
    const clippedPreviousOutput = transcript
      ? transcript.length > 1200
        ? `${transcript.slice(-1200)}…`
        : transcript
      : "None";

    return [
      "STATUS: Running",
      `SUMMARY: ${contract.ticket.title} is prepared for Agent execution.`,
      "EVIDENCE:",
      `- Work item: ${contract.ticket.key ?? contract.task_id} · ${contract.ticket.title}`,
      `- Runtime: ${runtimeLabel}`,
      `- Jira link: ${issueKey ? `Issue ${issueKey}` : "Not linked"}`,
      `- Labels: ${contract.ticket.labels.length > 0 ? contract.ticket.labels.join(", ") : "None"}`,
      `- Task description: ${clippedDescription || "None"}`,
      "DETAILS:",
      `- Current board state: ${contract.task_context.execution_detail}`,
      `- Operator notes: ${operatorNotes ? operatorNotes : "None"}`,
      `- Session transcript replay: ${clippedPreviousOutput}`,
      "- Verification target: return evidence before marking complete.",
      "NEXT:",
      "- Pass the exported context to the runtime and wait for evidence.",
    ].join("\n");
  }

  function buildExecutionContract(
    runId: string,
    card: CardProjection,
    issueKey: string | null,
    operatorNotes: string | null,
    previousOutput: string | null,
    jiraTransitionCompleted: boolean,
  ): ExecutionContract {
    const completedSteps = jiraTransitionCompleted ? ["jira.transition.in_progress"] : [];
    const workflow: ExecutionContract["workflow"] = [
      {
        step_id: "jira.transition.in_progress",
        title: "Move linked Jira issue to In Progress if needed",
        type: "jira.transition",
        status: jiraTransitionCompleted ? "completed" : issueKey ? "remaining" : "completed",
      },
      {
        step_id: "worker.execute",
        title: "Execute the already-planned task",
        type: "worker.execute",
        status: "current",
      },
      {
        step_id: "worker.verify",
        title: "Verify execution evidence before reporting completion",
        type: "worker.verify",
        status: "remaining",
      },
      {
        step_id: "jira.comment.result",
        title: "Spacesly records final result on Jira after worker returns",
        type: "jira.comment",
        status: issueKey ? "remaining" : "completed",
      },
    ];

    return {
      contract_id: `contract-${runId}`,
      version: 1,
      task_id: card.id,
      workspace_id: workspace?.id ?? "workspace-personal",
      created_at: new Date().toISOString(),
      objective: {
        summary: card.title,
        success_criteria: [
          "Execute only the current worker step from this contract.",
          "Return concrete evidence for any claimed completion.",
          "Return blocked if required tools, permissions, or context are unavailable.",
        ],
      },
      task_context: {
        description: card.description,
        execution_detail: executionDetail(card.execution),
      },
      ticket: {
        provider: issueKey ? "jira" : "local",
        key: issueKey,
        url: card.url,
        title: card.title,
        labels: card.labels,
        status: card.jira_snapshot?.status ?? null,
        updated_at: card.jira_snapshot?.updated_at ?? null,
        fetched_at: card.jira_snapshot?.fetched_at ?? null,
      },
      workflow,
      completed_steps: completedSteps,
      current_step: "worker.execute",
      remaining_steps: workflow
        .filter((step) => step.status === "remaining")
        .map((step) => step.step_id),
      repository: {
        root_path: workspaceGitInfo?.repo_root ?? workspaceRoot,
        branch: workspaceGitInfo?.current_branch ?? null,
        head_commit: workspaceGitInfo?.head_commit ?? null,
      },
      constraints: {
        execution_only: true,
        planning_completed: true,
        must_not_read_jira_for_planning: true,
        must_not_classify_ticket: true,
        must_not_regenerate_workflow: true,
        must_not_rediscover_repository: true,
        may_modify_files: true,
        may_update_jira: false,
      },
      runtime_inputs: {
        operator_notes: operatorNotes,
        previous_output: previousOutput,
      },
    };
  }

  function createExecutionRun(runId: string, contract: ExecutionContract): ExecutionRun {
    const startedAt = new Date().toISOString();
    const step_runs = Object.fromEntries(
      contract.workflow.map((step) => [
        step.step_id,
        {
          step_id: step.step_id,
          status:
            step.status === "completed"
              ? "completed"
              : step.status === "current"
                ? "ready"
                : "pending",
          attempt: 0,
          started_at: step.status === "completed" ? startedAt : null,
          completed_at: step.status === "completed" ? startedAt : null,
          summary: step.status === "completed" ? step.title : null,
        },
      ]),
    ) as ExecutionRun["step_runs"];

    return {
      run_id: runId,
      contract,
      status: "pending",
      current_step_ids: [contract.current_step],
      step_runs,
      started_at: startedAt,
      completed_at: null,
    };
  }

  function resumeExecutionRun(
    runId: string,
    previousRun: ExecutionRun,
    operatorNotes: string | null,
    previousOutput: string | null,
  ): ExecutionRun {
    const workerStep = previousRun.step_runs["worker.execute"];
    const verifyStep = previousRun.step_runs["worker.verify"];
    return {
      ...previousRun,
      run_id: runId,
      contract: {
        ...previousRun.contract,
        contract_id: `contract-${runId}`,
        version: previousRun.contract.version + 1,
        runtime_inputs: {
          operator_notes: operatorNotes,
          previous_output: previousOutput,
        },
      },
      status: "pending",
      current_step_ids: ["worker.execute"],
      completed_at: null,
      step_runs: {
        ...previousRun.step_runs,
        ...(workerStep
          ? {
              "worker.execute": {
                ...workerStep,
                status: "ready" as const,
                completed_at: null,
                summary: null,
                lease_owner: null,
                lease_expires_at: null,
              },
            }
          : {}),
        ...(verifyStep
          ? {
              "worker.verify": {
                ...verifyStep,
                status: "pending" as const,
                started_at: null,
                completed_at: null,
                summary: null,
                lease_owner: null,
                lease_expires_at: null,
              },
            }
          : {}),
      },
    };
  }

  function updateExecutionStep(
    run: ExecutionRun,
    stepId: string,
    status: ExecutionRun["step_runs"][string]["status"],
    summary: string | null = null,
  ): ExecutionRun {
    const now = new Date().toISOString();
    const current = run.step_runs[stepId];
    if (!current) return run;

    return {
      ...run,
      status:
        status === "blocked"
          ? "blocked"
          : status === "failed"
            ? "failed"
            : status === "running"
              ? "running"
              : run.status,
      step_runs: {
        ...run.step_runs,
        [stepId]: {
          ...current,
          status,
          attempt: status === "running" ? current.attempt + 1 : current.attempt,
          started_at: status === "running" ? (current.started_at ?? now) : current.started_at,
          completed_at: ["completed", "blocked", "failed", "skipped"].includes(status)
            ? now
            : current.completed_at,
          summary: summary ?? current.summary,
        },
      },
    };
  }

  function completeExecutionRun(
    run: ExecutionRun,
    blocked: boolean,
    summary: string,
  ): ExecutionRun {
    const now = new Date().toISOString();
    return {
      ...run,
      status: blocked ? "blocked" : "completed",
      current_step_ids: [],
      completed_at: now,
      step_runs: {
        ...run.step_runs,
        "worker.verify": {
          ...(run.step_runs["worker.verify"] ?? {
            step_id: "worker.verify",
            status: "pending",
            attempt: 0,
            started_at: null,
            completed_at: null,
            summary: null,
          }),
          status: blocked ? "blocked" : "completed",
          completed_at: now,
          summary,
        },
      },
    };
  }

  function appendTerminalLineForCard(cardId: string, prompt: string, text: string) {
    updateAgentSessionForCard(cardId, (session) => ({
      ...session,
      terminalLines: capList(
        [
          ...session.terminalLines,
          {
            id: `term-${Date.now().toString(36)}-${session.terminalLines.length}`,
            prompt,
            text,
          },
        ],
        MAX_AGENT_TERMINAL_LINES,
      ),
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
    appendAgentSessionTranscriptForCard(
      cardId,
      isApprovalText(input) ? "approval" : "operator_note",
      input,
    );
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
      appNotice = {
        tone: "info",
        message: "Approval recorded. Continue the Agent on this card when ready.",
      };
    }
    agentTerminalInput = "";
  }

  async function cancelAgentRunForCard(cardId: string) {
    const session = agentSessionForCard(cardId);
    if (!session || session.status !== "running") return;
    const legacyRunId = runningWorkerRunIds[cardId];
    const cancelled =
      session.taskSessionId != null
        ? await cancelTaskSession(session.taskSessionId).catch(() => false)
        : legacyRunId
          ? await cancelAiWorkerTask(legacyRunId).catch(() => false)
          : false;
    if (cancelled && session.taskSessionId != null) {
      updateAgentSessionForCard(cardId, (current) => ({
        ...current,
        taskSessionState: "cancelling",
      }));
    }
    appendStructuredAgentLogForCard(
      cardId,
      cancelled ? "info" : "error",
      "cancel",
      cancelled ? "Operator cancellation requested." : "Cancellation could not be confirmed.",
      [
        session.taskSessionId
          ? `Task Session: ${session.taskSessionId}`
          : `Legacy run: ${legacyRunId ?? "unknown"}`,
      ],
      [],
      [
        cancelled
          ? "Wait for runtime cleanup before continuing."
          : "Review runtime state before retrying.",
      ],
    );
    if (cancelled) {
      appNotice = { tone: "info", message: `Cancellation requested for ${session.title}.` };
    } else {
      appNotice = {
        tone: "error",
        message: `Could not confirm cancellation for ${session.title}.`,
      };
    }
  }

  function isApprovalText(value: string): boolean {
    const text = value.toLowerCase();
    return (
      text.includes("approve") || text.includes("approved") || text.includes("approval granted")
    );
  }

  function operatorNotesForCard(cardId: string): string | null {
    const session = agentRunSessions[cardId];
    const notes = session?.terminalLines
      .filter((line) => line.prompt === "operator")
      .map((line) => line.text.trim())
      .filter(Boolean)
      .join("\n")
      .trim();

    return notes || null;
  }

  function previousOutputForCard(cardId: string): string | null {
    const session = agentRunSessions[cardId];
    const transcript = agentSessionReplay(
      session?.transcript ?? [],
      MAX_AGENT_SESSION_REPLAY_CHARS,
    );
    if (transcript) return transcript;

    const output = session?.output?.trim();
    return output &&
      output !== "Waiting for Agent output..." &&
      output !== "Agent is processing the task context..."
      ? output
      : null;
  }

  function resolveSessionCardId(sessionId = workspaceChatActiveSessionId): string | null {
    const session =
      workspaceChatSessions.find((entry) => entry.id === sessionId) ?? workspaceChatSession;
    const candidates = [
      session.lastCreatedCardId,
      session.lastCardId,
      ...session.recentCardIds,
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

  function agentJiraComment(
    result: AiWorkerTaskResult,
    config: AiWorkerConfig,
    request: string,
    gitInfo: GitWorkspaceInfo | null = null,
  ): string {
    const runtime = config.runtime === "opencode" ? "OpenCode" : config.provider_name;
    const model = config.runtime === "opencode" ? config.opencode_model : config.model;
    return formatJiraExecutionComment({
      request,
      result,
      runtime,
      model,
      environment:
        config.runtime === "opencode"
          ? config.opencode_workdir || "OpenCode workspace"
          : "API Agent",
      branch: gitInfo?.current_branch,
      revision: gitInfo?.head_commit,
      upstream: gitInfo?.upstream_branch,
    });
  }

  function agentWritebackRequiresPushedCommit(card: CardProjection): boolean {
    const text = [card.title, card.description, card.labels.join(" ")].join("\n").toLowerCase();

    const hasUpdateVerb = ["update", "change", "modify", "edit", "add", "remove", "set"].some(
      (needle) => text.includes(needle),
    );
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

  async function verifyAgentJiraDoneGate(
    card: CardProjection,
    config: AiWorkerConfig,
  ): Promise<GitWorkspaceInfo | null> {
    if (!agentWritebackRequiresPushedCommit(card)) return null;

    const gitPath =
      config.opencode_workdir?.trim() || workspaceRoot || workspaceGitInfo?.repo_root || null;
    if (!gitPath) {
      throw new Error(
        "Jira Done blocked: Spacesly cannot determine the git workdir for this Helm/env/template change.",
      );
    }

    const info = config.opencode_workdir?.trim()
      ? await getPathGitInfo(gitPath)
      : await getWorkspaceGitInfo(workspace?.id);
    workspaceGitInfo = info.is_git_repo ? info : null;
    selectedWorkspaceBranch = info.current_branch ?? "";

    if (!info.is_git_repo) {
      throw new Error(
        "Jira Done blocked: Spacesly cannot verify a git repository for this Helm/env/template change.",
      );
    }

    const runSnapshot = agentSessionForCard(card.id)?.gitSnapshot ?? null;
    if (!runSnapshot?.head_commit) {
      throw new Error(
        "Jira Done blocked: Spacesly did not capture the starting commit for this Agent run.",
      );
    }

    if (!info.head_commit) {
      throw new Error("Jira Done blocked: Spacesly cannot read the current git HEAD commit.");
    }

    if (info.head_commit === runSnapshot.head_commit) {
      throw new Error(
        "Jira Done blocked: no new commit was created for this Helm/env/template change.",
      );
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
    const cleaned = value
      .replace(/```[\s\S]*?```/g, "")
      .replace(/\s+/g, " ")
      .trim();
    if (cleaned.length <= maxChars) return cleaned;
    return `${cleaned.slice(0, maxChars - 3).trim()}...`;
  }

  function updateCardExecution(cardId: string, execution: ExecutionState) {
    if (!workspace) return;
    const completedAt =
      typeof execution === "object" && "completed" in execution ? Date.now() : null;

    if (
      !updateActiveBoard((board) => ({
        ...board,
        columns: board.columns.map((column) => ({
          ...column,
          cards: column.cards.map((card) =>
            card.id === cardId ? { ...card, execution, completedAt } : card,
          ),
        })),
      }))
    )
      return;
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace!);
  }

  function resolveActionCard(
    action: {
      card_id?: string;
      ticket?: string;
      title?: string;
    },
    sessionId = workspaceChatActiveSessionId,
  ): CardProjection | null {
    if (action.card_id && activeCardById.has(action.card_id))
      return activeCardById.get(action.card_id) ?? null;

    const ticket = action.ticket?.toLowerCase();
    if (ticket) {
      const byTicket = activeCards.find((card) => ticketLabel(card).toLowerCase() === ticket);
      if (byTicket) return byTicket;
    }

    const title = action.title?.toLowerCase();
    if (title) {
      return activeCards.find((card) => card.title.toLowerCase().includes(title)) ?? null;
    }

    const sessionCardId = resolveSessionCardId(sessionId);
    return sessionCardId ? (activeCardById.get(sessionCardId) ?? null) : null;
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
    if (!isBlocked(card.execution) && agentSessionForCard(cardId)?.status !== "blocked") {
      appNotice = {
        tone: "error",
        message: "Manual Done is only available for blocked Agent sessions.",
      };
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
    if (!isBlocked(card.execution) && agentSessionForCard(cardId)?.status !== "blocked") {
      appNotice = {
        tone: "error",
        message: "Manual Done is only available for blocked Agent sessions.",
      };
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
    const sourceIntent = cardColumnIntentById.get(cardId);
    const sourceColumn = sourceIntent ? activeColumnByIntent.get(sourceIntent) : undefined;
    const sourceExecution = card?.execution;
    const rollback = () => {
      if (sourceColumn && sourceExecution !== undefined) {
        moveCard(cardId, sourceColumn.id, sourceExecution, false);
      }
    };

    const localExecution = executionForColumn(targetColumnId);
    moveCard(cardId, targetColumnId, localExecution ?? undefined);

    if (!issueKey || !targetStatus) return;

    const config = buildJiraConfig();
    if (!config) {
      rollback();
      appNotice = {
        tone: "error",
        message: `Could not sync ${issueKey} to Jira. Configure Jira before moving this issue.`,
      };
      return;
    }

    try {
      if (targetStatus === "In Progress") {
        await assignJiraIssue(config, issueKey);
      }
      await transitionJiraIssue(config, issueKey, targetStatus);
      appNotice = {
        tone: "success",
        message:
          targetStatus === "In Progress"
            ? `${issueKey} assigned to you and moved to ${targetStatus} in Jira.`
            : `${issueKey} moved to ${targetStatus} in Jira.`,
      };
    } catch (reason) {
      rollback();
      appNotice = {
        tone: "error",
        message: reason instanceof Error ? reason.message : String(reason),
      };
    }
  }

  async function startWorkerForCard(cardId: string, backlogAlreadyApproved = false) {
    const retainedTaskState = agentSessionForCard(cardId)?.taskSessionState;
    if (
      runningWorkerCardIds[cardId] ||
      agentSessionForCard(cardId)?.status === "running" ||
      (retainedTaskState &&
        ["queued", "running", "cancelling", "committing"].includes(retainedTaskState))
    ) {
      appNotice = { tone: "info", message: "Agent is already running this card." };
      return;
    }

    const card = activeCardById.get(cardId);
    if (!card || card.execution === "running") return;
    const inProgressColumnId = columnIdByIntent("in_progress");
    const doneColumnId = columnIdByIntent("done");
    if (!inProgressColumnId || !doneColumnId) {
      appNotice = {
        tone: "error",
        message: "Agent cannot start because this board is missing an In Progress or Done column.",
      };
      return;
    }
    const runId = `agent-${cardId}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    runningWorkerCardIds = { ...runningWorkerCardIds, [cardId]: true };
    runningWorkerRunIds = { ...runningWorkerRunIds, [cardId]: runId };
    if (!backlogAlreadyApproved && !(await requestBacklogStartConfirmation(card))) {
      finishWorkerRun(cardId, runId);
      return;
    }

    const config = buildAiWorkerConfig();
    if (!config) {
      finishWorkerRun(cardId, runId);
      return;
    }
    if (!(await ensureAiWorkspaceTrusted(config))) {
      finishWorkerRun(cardId, runId);
      return;
    }
    const useTaskSession =
      config.runtime === "opencode" &&
      typeof window !== "undefined" &&
      "__TAURI_INTERNALS__" in window;

    if (!useTaskSession) {
      try {
        await reserveAiWorkerRun(runId, config);
        const grantedCapabilities = [
          "workspace_read",
          "workspace_write",
          "shell",
          "git",
          ...config.mcp_servers.map((server) => `external_tools:${server.secret_id}`),
        ] as ("workspace_read" | "workspace_write" | "shell" | "git" | "external_tools")[];
        await grantAiRunCapabilities(runId, grantedCapabilities);
      } catch (reason) {
        await releaseAiWorkerRun(runId).catch(() => false);
        finishWorkerRun(cardId, runId);
        const message = reason instanceof Error ? reason.message : String(reason);
        appNotice = { tone: "error", message };
        return;
      }
      markActiveAgentRun(
        typeof localStorage === "undefined" ? undefined : localStorage,
        cardId,
        runId,
      );
    }

    const issueKey = jiraKey(card);
    const existingSession = agentRunSessions[cardId];
    const isContinuation =
      existingSession?.status === "blocked" ||
      existingSession?.status === "blocked_for_resume" ||
      existingSession?.status === "timeout";
    const operatorNotes = operatorNotesForCard(cardId);
    const previousOutput = previousOutputForCard(cardId);
    const resumeAuthoritativeResult =
      existingSession?.status === "blocked_for_resume" && existingSession.result
        ? existingSession.result
        : null;
    let backendExecutionStarted = false;
    let jiraTransitionCompleted = !issueKey;

    try {
      beginAgentRun(card, isContinuation, null);
      const runtimeLabel =
        config.runtime === "opencode"
          ? `OpenCode ${config.opencode_model}`
          : `${config.provider_name} ${config.model}`;
      appNotice = {
        tone: "info",
        message: `${isContinuation ? "Agent continuing" : "Agent started"} ${ticketLabel(card)} with ${runtimeLabel}.`,
      };

      if (config.runtime === "opencode") {
        const runGitInfo = config.opencode_workdir?.trim()
          ? await getPathGitInfo(config.opencode_workdir.trim())
          : await getWorkspaceGitInfo(workspace?.id);
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
        config.runtime === "opencode"
          ? `Using OpenCode / ${config.opencode_model}.`
          : `Using ${config.provider_name} / ${config.model}.`,
        [
          `Runtime selected: ${config.runtime === "opencode" ? `OpenCode ${config.opencode_model}` : `${config.provider_name} ${config.model}`}`,
        ],
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
          jiraTransitionCompleted = true;
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
        jiraTransitionCompleted = true;
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
      let executionRun =
        resumeAuthoritativeResult && existingSession?.executionRun
          ? existingSession.executionRun
          : isContinuation && existingSession?.executionRun
            ? resumeExecutionRun(runId, existingSession.executionRun, operatorNotes, previousOutput)
            : createExecutionRun(
                runId,
                buildExecutionContract(
                  runId,
                  card,
                  issueKey,
                  operatorNotes,
                  previousOutput,
                  jiraTransitionCompleted,
                ),
              );
      setAgentRunOutputForCard(cardId, buildAgentContextExport(config, executionRun.contract));
      setAgentProgressForCard(cardId, 55);
      if (issueKey && !jiraTransitionCompleted) {
        const message = `Execution prerequisite failed: ${issueKey} was not transitioned to In Progress.`;
        executionRun = updateExecutionStep(
          { ...executionRun, status: "blocked", completed_at: new Date().toISOString() },
          "jira.transition.in_progress",
          "blocked",
          message,
        );
        await setExecutionRunForCard(cardId, executionRun);
        throw new Error(message);
      }
      if (!resumeAuthoritativeResult) {
        executionRun = updateExecutionStep(
          executionRun,
          "worker.execute",
          "running",
          "Execution worker started.",
        );
        await setExecutionRunForCard(cardId, executionRun);
      }
      let result: AiWorkerTaskResult;
      let lastAgentEventSequence = 0;
      if (resumeAuthoritativeResult) {
        result = resumeAuthoritativeResult;
        appendStructuredAgentLogForCard(
          cardId,
          "info",
          "resume",
          `Continuing from ${existingSession?.workflowCheckpoint ?? "agent_result_committed"}.`,
          [`Task Session: ${existingSession?.taskSessionId ?? "retained"}`],
          ["The authoritative Agent result was reused; Agent execution was not repeated."],
          ["Continue with verification and explicitly approved writeback stages."],
        );
      } else
        try {
          backendExecutionStarted = true;
          if (useTaskSession) {
            const prepared = await prepareAgentTaskSession(
              config,
              cardId,
              card.title,
              executionRun.run_id,
              executionRun.contract,
              await workspaceRootRevision(config.workspace_id),
            );
            updateAgentSessionForCard(cardId, (session) => ({
              ...session,
              conversationId: prepared.conversationId,
            }));
            const execution = await executeAgentTaskSession(ticketLabel(card), prepared, {
              onSubmitted: (session) => {
                updateAgentSessionForCard(cardId, (current) => ({
                  ...current,
                  taskSessionId: session.id,
                  taskSessionState: session.state,
                  conversationId: prepared.conversationId,
                }));
              },
              onEvent: (event) => projectTaskSessionEvent(cardId, event),
            });
            result = execution.result;
          } else {
            result = await executeAiWorkerTask(
              runId,
              config,
              {
                execution_contract: executionRun.contract,
                session_key: `task:${cardId}`,
              },
              (event) => {
                if (event.run_id !== runId || event.sequence <= lastAgentEventSequence) return;
                lastAgentEventSequence = event.sequence;
                if (event.type === "run_started") {
                  appendStructuredAgentLogForCard(
                    cardId,
                    "info",
                    "runtime",
                    "Agent runtime started.",
                    ["Execution events are now being tracked by the backend runtime."],
                    [],
                    [],
                  );
                } else if (event.type === "run_blocked") {
                  appendStructuredAgentLogForCard(
                    cardId,
                    "error",
                    "runtime",
                    "Agent runtime blocked the execution.",
                    [],
                    [],
                    ["Review the blocked reason and resolve the missing requirement."],
                  );
                } else if (event.type === "tool_started") {
                  appendStructuredAgentLogForCard(
                    cardId,
                    "info",
                    event.display_context.category,
                    `${event.display_context.label}.`,
                    [
                      `Category: ${event.display_context.category}`,
                      ...(event.display_context.target
                        ? [`Target: ${event.display_context.target}`]
                        : []),
                    ],
                    [`Tool: ${event.tool_name}`, `Risk: ${event.risk}`],
                    ["Wait for the runtime to report tool completion."],
                  );
                } else if (event.type === "tool_completed") {
                  appendStructuredAgentLogForCard(
                    cardId,
                    event.success ? "info" : "error",
                    event.display_context.category,
                    `${event.success ? "Completed" : "Failed"}: ${event.display_context.label}.`,
                    [
                      `Category: ${event.display_context.category}`,
                      ...(event.display_context.target
                        ? [`Target: ${event.display_context.target}`]
                        : []),
                    ],
                    [`Tool: ${event.tool_name}`, `Risk: ${event.risk}`],
                    event.success ? [] : ["Review the tool failure evidence before continuing."],
                  );
                } else if (event.type === "approval_required") {
                  appendStructuredAgentLogForCard(
                    cardId,
                    "error",
                    "approval",
                    `Tool approval required: ${event.operation}.`,
                    [
                      `Capability: ${event.capability}`,
                      `Operation: ${event.operation_id}`,
                      `Risk: ${event.risk}`,
                    ],
                    ["The backend stopped the unapproved operation."],
                    ["Grant the required capability and start a new execution run."],
                  );
                }
              },
            );
          }
        } catch (reason) {
          if (
            reason instanceof AgentTaskSessionTimeoutError ||
            (reason instanceof IpcPolicyError && reason.category === "timeout")
          ) {
            const cancelled =
              reason instanceof AgentTaskSessionTimeoutError
                ? reason.cancelled
                : await cancelAiWorkerTask(runId).catch(() => false);
            if (reason instanceof AgentTaskSessionTimeoutError && reason.terminalState) {
              updateAgentSessionForCard(cardId, (session) => ({
                ...session,
                taskSessionState: reason.terminalState,
              }));
            }
            updateExecutionRunForCard(cardId, (run) =>
              updateExecutionStep(
                { ...run, status: cancelled ? "cancelled" : "blocked" },
                "worker.execute",
                cancelled ? "failed" : "blocked",
                reason.message,
              ),
            );
            if (cancelled) {
              updateCardExecution(cardId, {
                blocked: { reason: "Agent timed out and was cancelled." },
              });
            }
            setAgentRunStatusForCard(cardId, "timeout");
            appendStructuredAgentLogForCard(
              cardId,
              "error",
              "timeout",
              "Spacesly stopped waiting for the Agent response before a structured result arrived.",
              [reason.message],
              [
                cancelled
                  ? "The Agent process was cancelled."
                  : "Spacesly could not confirm process cancellation.",
              ],
              [
                cancelled
                  ? "Review the task, then retry when ready."
                  : "Do not retry until the Agent process is confirmed stopped.",
              ],
            );
            appNotice = {
              tone: cancelled ? "info" : "error",
              message: cancelled
                ? `${ticketLabel(card)} timed out and the Agent process was cancelled.`
                : `${ticketLabel(card)} timed out, but process cancellation could not be confirmed.`,
            };
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
        [`Evidence lines: ${result.evidence.length}`, `Detail lines: ${result.details.length}`],
        result.completion_status === "completed"
          ? ["Review the result, then write back to board and Jira."]
          : ["Inspect the blocker, add notes if needed, and continue."],
      );
      setAgentRunResultForCard(cardId, result);
      setAgentRunOutputForCard(cardId, agentResultText(result));
      appendTerminalLineForCard(cardId, "agent", agentResultText(result));
      appendAgentSessionTranscriptForCard(
        cardId,
        result.completion_status === "completed" ? "agent_output" : "blocker",
        agentResultText(result),
      );
      setAgentProgressForCard(cardId, 75);

      if (result.completion_status !== "completed") {
        const reason = result.blocked_reason ?? result.summary;
        updateExecutionRunForCard(cardId, (run) =>
          updateExecutionStep(run, "worker.execute", "blocked", reason),
        );
        updateCardExecution(cardId, { blocked: { reason } });
        setAgentRunStatusForCard(cardId, "blocked");
        appendAgentSessionTranscriptForCard(cardId, "blocker", reason);
        appendStructuredAgentLogForCard(
          cardId,
          "error",
          "blocked",
          "Agent did not complete and verify the requested work. Card will not move to Done.",
          [`Completion status: ${result.completion_status}`, `Blocked reason: ${reason}`],
          [
            `Card execution remains blocked until the issue is resolved.`,
            `No Done transition will occur for this run.`,
          ],
          ["Add operator notes or fix the blocker, then continue the Agent."],
        );
        appNotice = { tone: "error", message: `${ticketLabel(card)} blocked: ${reason}` };
        return;
      }

      await persistExecutionRunUpdateForCard(cardId, (run) => {
        const executed = updateExecutionStep(run, "worker.execute", "completed", result.summary);
        const verifying = updateExecutionStep(
          executed,
          "worker.verify",
          "running",
          "Verification started.",
        );
        return { ...verifying, current_step_ids: ["worker.verify"] };
      });

      let gitWritebackInfo: GitWorkspaceInfo | null = null;
      try {
        gitWritebackInfo = await verifyAgentJiraDoneGate(card, config);
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : String(reason);
        updateExecutionRunForCard(cardId, (run) =>
          updateExecutionStep(run, "worker.verify", "blocked", message),
        );
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

      await persistExecutionRunUpdateForCard(cardId, (run) =>
        updateExecutionStep(run, "worker.verify", "completed", "Verification passed."),
      );

      appendStructuredAgentLogForCard(
        cardId,
        "success",
        "board",
        "Stored Agent summary on card and moved card to Done locally.",
        [`Card: ${ticketLabel(card)}`, `Board target: Done`],
        [
          `Local workspace projection updated with the verified summary.`,
          `Completed timestamp stored for the card.`,
        ],
        issueKey
          ? ["Write Jira completion state and add the completion comment."]
          : ["No Jira issue linked; board write-back is complete."],
      );
      moveCard(cardId, doneColumnId, { completed: { summary: result.summary } });
      setAgentProgressForCard(cardId, 82);

      if (issueKey) {
        const jiraConfig = buildJiraConfig();
        const resumeCheckpoint = existingSession?.workflowCheckpoint;
        const recoveryDecision = resumeCheckpoint
          ? agentWorkflowRecoveryDecision(resumeCheckpoint)
          : { safe: true as const };
        if (!recoveryDecision.safe) {
          updateCardExecution(cardId, { blocked: { reason: recoveryDecision.reason } });
          setAgentRunStatusForCard(cardId, "blocked");
          appendAgentSessionTranscriptForCard(cardId, "blocker", recoveryDecision.reason);
          appendStructuredAgentLogForCard(
            cardId,
            "error",
            "recovery",
            recoveryDecision.reason,
            [`Checkpoint: ${resumeCheckpoint}`],
            ["No Jira transition or comment was replayed."],
            ["Reconcile Jira manually before starting a newly reviewed run."],
          );
          appNotice = { tone: "error", message: recoveryDecision.reason };
          return;
        }
        if (jiraConfig && resumeCheckpoint !== "jira_writeback_completed") {
          if (resumeCheckpoint !== "jira_transition_completed") {
            await persistExecutionRunUpdateForCard(cardId, (run) => ({
              ...updateExecutionStep(
                run,
                "jira.comment.result",
                "running",
                "Jira writeback started.",
              ),
              current_step_ids: ["jira.comment.result"],
            }));
          }
          appendStructuredAgentLogForCard(
            cardId,
            "info",
            "jira",
            `Moving ${issueKey} to Done.`,
            [`Issue: ${issueKey}`, `Transition target: Done`],
            ["Posting board completion back to Jira."],
            ["Wait for the Jira transition result before finalizing the run."],
          );
          setAgentProgressForCard(cardId, 88);
          if (resumeCheckpoint !== "jira_transition_completed") {
            await transitionJiraIssue(jiraConfig, issueKey, "Done");
            await persistExecutionRunUpdateForCard(cardId, (run) =>
              updateExecutionStep(
                run,
                "jira.comment.result",
                "running",
                "Jira Done transition completed; completion comment pending.",
              ),
            );
          }
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
          await persistExecutionRunUpdateForCard(cardId, (run) =>
            updateExecutionStep(
              run,
              "jira.comment.result",
              "running",
              "Jira completion comment started; confirmation pending.",
            ),
          );
          await addJiraComment(
            jiraConfig,
            issueKey,
            agentJiraComment(result, config, card.title, gitWritebackInfo),
          );
          await persistExecutionRunUpdateForCard(cardId, (run) =>
            updateExecutionStep(
              run,
              "jira.comment.result",
              "completed",
              "Jira transition and completion comment succeeded.",
            ),
          );
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

      await persistExecutionRunUpdateForCard(cardId, (run) =>
        completeExecutionRun(run, false, result.summary),
      );
      setAgentRunStatusForCard(cardId, "completed");
      setAgentProgressForCard(cardId, 100);
      appNotice = {
        tone: "success",
        message: `${ticketLabel(card)} completed by Agent and moved to Done.`,
      };
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      updateExecutionRunForCard(cardId, (run) => {
        const currentStep = run.current_step_ids[0] ?? run.contract.current_step;
        return updateExecutionStep(
          { ...run, status: "failed", completed_at: new Date().toISOString() },
          currentStep,
          "failed",
          message,
        );
      });
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
      clearActiveAgentRun(
        useTaskSession || typeof localStorage === "undefined" ? undefined : localStorage,
        cardId,
        runId,
      );
      if (!useTaskSession && !backendExecutionStarted) {
        await releaseAiWorkerRun(runId).catch(() => undefined);
      }
      finishWorkerRun(cardId, runId);
    }
  }

  function finishWorkerRun(cardId: string, runId: string) {
    if (runningWorkerRunIds[cardId] !== runId) return;
    const { [cardId]: _finishedCard, ...remainingCards } = runningWorkerCardIds;
    const { [cardId]: _finishedRun, ...remainingRunIds } = runningWorkerRunIds;
    runningWorkerCardIds = remainingCards;
    runningWorkerRunIds = remainingRunIds;
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

      <button class="icon-button" type="button" aria-label="Settings" onclick={() => openSettings()}
        >Settings</button
      >

      <nav class="mode-switch" aria-label="Workspace mode">
        <button
          class:active={workspaceMode === "board"}
          type="button"
          aria-label="Board view"
          onclick={() => setWorkspaceMode("board")}>Board</button
        >
        <button
          class:active={workspaceMode === "files"}
          type="button"
          aria-label="Files view"
          onclick={() => setWorkspaceMode("files")}>Files</button
        >
        <button
          class:active={workspaceMode === "term"}
          type="button"
          aria-label="Terminal view"
          onclick={openTermWorkspace}>Term</button
        >
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
      {#if deletedJiraCardCount > 0}
        <button
          class="sync-button restore-button"
          type="button"
          disabled={syncing}
          onclick={restoreDeletedJiraCards}
          title="Show Jira cards previously removed from Spacesly"
        >
          Restore {deletedJiraCardCount} deleted
        </button>
      {/if}
    </header>

    {#if settingsOpen}
      <section class="settings-backdrop" aria-label="Settings dialog">
        <div
          class="settings-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="settings-title"
        >
          <header>
            <div>
              <p>Settings</p>
              <h2 id="settings-title">{settingsTitle}</h2>
            </div>
            <button type="button" aria-label="Close settings" onclick={closeSettings}>×</button>
          </header>

          <div class:mcp={settingsTab === "mcp"} class="settings-grid">
            <aside class="settings-nav" aria-label="Settings navigation">
              <button
                class:active={settingsTab === "agent"}
                type="button"
                onclick={() => switchSettingsTab("agent")}
              >
                <strong>Agent</strong>
                <span>Model and worker runtime</span>
              </button>
              <button
                class:active={settingsTab === "rules"}
                type="button"
                onclick={() => switchSettingsTab("rules")}
              >
                <strong>Rules</strong>
                <span>Operating guardrails</span>
              </button>
              <button
                class:active={settingsTab === "skills"}
                type="button"
                onclick={() => switchSettingsTab("skills")}
              >
                <strong>Skills</strong>
                <span>Reusable playbooks</span>
              </button>
              <button
                class:active={settingsTab === "mcp"}
                type="button"
                onclick={() => switchSettingsTab("mcp")}
              >
                <strong>MCP</strong>
                <span>Tools and server connections</span>
              </button>
              <button
                class:active={settingsTab === "jira"}
                type="button"
                onclick={() => switchSettingsTab("jira")}
              >
                <strong>Jira</strong>
                <span>Board sync and credentials</span>
              </button>
              <button
                class:active={settingsTab === "environment"}
                type="button"
                onclick={() => switchSettingsTab("environment")}
              >
                <strong>Global Environment</strong>
                <span>Process environment variables</span>
              </button>
              <button
                class:active={settingsTab === "theme"}
                type="button"
                onclick={() => switchSettingsTab("theme")}
              >
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
                    aria-pressed={server.id === selectedServerId}
                    onclick={() => {
                      selectedServerId = server.id;
                      if (server.kind === "jira") {
                        settings = { ...settings, jira: { ...settings.jira, serverId: server.id } };
                      }
                    }}
                  >
                    <strong>{server.name || "Unnamed MCP"}</strong>
                    <span
                      >{server.kind.toUpperCase()} · {server.command ||
                        "No command configured"}</span
                    >
                    <small
                      class={`mcp-status ${mcpConnectionState(server.id)?.status ?? "unknown"}`}
                    >
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
                      configuredEnvKeys={mcpEnvironmentSecrets[selectedServer.id] ?? []}
                      onUpdate={updateSelectedServer}
                      onError={(message) => (settingsError = message)}
                    />
                  {:else}
                    <section class="settings-section">
                      <div>
                        <p class="section-kicker">MCP Connection</p>
                        <h3>Loading connection settings</h3>
                      </div>
                      <p class="field-help">
                        Preparing MCP controls only when this settings tab is opened.
                      </p>
                    </section>
                  {/if}
                {/if}

                {#if settingsTab === "agent"}
                  <div class="settings-section worker-section">
                    <div>
                      <p class="section-kicker">Agent</p>
                      <h3>Model runtime</h3>
                    </div>
                    <p class="field-help">
                      Choose a supported provider and model. Spacesly configures the endpoint
                      automatically and only asks for the credential that provider requires.
                    </p>

                    <div class="runtime-options" aria-label="Agent runtime">
                      <button
                        class:active={settings.aiWorker.runtime === "api"}
                        type="button"
                        onclick={() => {
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, runtime: "api" },
                          };
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
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, runtime: "opencode" },
                          };
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
                              const modelId =
                                settings.aiWorker.modelIds[providerId] ??
                                defaultModelForProvider(providerId);
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
                              settings = {
                                ...settings,
                                aiWorker: {
                                  ...settings.aiWorker,
                                  modelId,
                                  modelIds: {
                                    ...settings.aiWorker.modelIds,
                                    [selectedAiProvider.id]: modelId,
                                  },
                                },
                              };
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
                            placeholder={aiProviderSecrets[selectedAiProvider.id]
                              ? "Saved securely. Enter a new key to replace it."
                              : selectedAiProvider.apiKeyPlaceholder}
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
                                aiWorker: {
                                  ...settings.aiWorker,
                                  temperature: Number(event.currentTarget.value),
                                },
                              })}
                          />
                        </label>
                      </div>
                    {:else}
                      <div class="endpoint-card">
                        <span>Runtime</span>
                        <code
                          >{settings.aiWorker.opencodeCommand} run --model {settings.aiWorker
                            .opencodeModel}</code
                        >
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
                                aiWorker: {
                                  ...settings.aiWorker,
                                  opencodeCommand: event.currentTarget.value,
                                },
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
                              <span class={`model-badge ${model.badge.toLowerCase()}`}
                                >{model.badge}</span
                              >
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
                          placeholder="Optional. Defaults to the open workspace folder."
                          value={settings.aiWorker.opencodeWorkdir}
                          oninput={(event) => {
                            settings = {
                              ...settings,
                              aiWorker: {
                                ...settings.aiWorker,
                                opencodeWorkdir: event.currentTarget.value,
                              },
                            };
                          }}
                        />
                      </label>
                      <label class="check-row">
                        <input type="checkbox" checked={false} disabled />
                        <span>Unrestricted auto-approval is disabled by the AI Runtime.</span>
                      </label>
                      <p class="field-help">
                        Run <code>opencode auth login</code> in your terminal first. Spacesly uses the
                        same local OpenCode credential store and does not need your OpenAI API key for
                        this runtime.
                      </p>
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
                        <span
                          >Keep these short, explicit, and enforceable. They are injected into Agent
                          tasks and chat before user instructions.</span
                        >
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
                        onblur={commitAgentRulesDraft}></textarea>
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
                        <span
                          >Describe repeatable work patterns for your domain. The Agent applies them
                          only when relevant to a task.</span
                        >
                      </div>
                      <strong>Contextual</strong>
                    </div>

                    <div class="skill-template-grid" aria-label="Skill examples">
                      <div>
                        <strong>Bamboo diagnostics</strong>
                        <span
                          >Check latest build, fetch logs, identify failing job, summarize evidence.</span
                        >
                      </div>
                      <div>
                        <strong>OCP troubleshooting</strong>
                        <span
                          >Inspect pods, events, logs, and resource usage before proposing fixes.</span
                        >
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
                        onblur={commitAgentSkillsDraft}></textarea>
                    </label>
                  </div>
                {/if}

                {#if settingsTab === "environment"}
                  <div class="settings-section">
                    <div>
                      <p class="section-kicker">Global Environment</p>
                      <h3>Process environment variables</h3>
                    </div>
                    <p class="field-help">
                      Variables defined here are automatically injected into every process that
                      Spacesly launches — terminals, shell commands, MCP servers, formatters, git
                      operations, and Agent workers.
                    </p>

                    <div class="field-row">
                      <label>
                        <span>Search</span>
                        <input
                          type="search"
                          placeholder="Filter by key…"
                          value={globalEnvironmentSearch}
                          oninput={(event) => (globalEnvironmentSearch = event.currentTarget.value)}
                        />
                      </label>
                      <button
                        type="button"
                        class="add-env-btn"
                        onclick={addGlobalEnvironmentVariable}
                        disabled={globalEnvironmentLoading}
                      >
                        ＋ Add Variable
                      </button>
                    </div>

                    {#if globalEnvironmentLoading && globalEnvironmentVariables.length === 0}
                      <p class="field-help">Loading environment variables…</p>
                    {:else if globalEnvironmentVariables.length === 0}
                      <p class="empty-state">No environment variables defined.</p>
                    {:else}
                      <div class="global-env-list">
                        {#each globalEnvironmentVariables.filter((env) => !globalEnvironmentSearch.trim() || env.key
                              .toLowerCase()
                              .includes(globalEnvironmentSearch
                                  .trim()
                                  .toLowerCase())) as variable (variable.id)}
                          <div
                            class="env-row"
                            class:env-draft={variable.draft}
                            class:env-secret={variable.secret}
                          >
                            <div class="env-row-fields">
                              <input
                                class="env-key-input"
                                type="text"
                                placeholder="KEY_NAME"
                                value={variable.key}
                                disabled={!variable.editing && !variable.draft}
                                oninput={(event) =>
                                  updateGlobalEnvironmentDraft(variable.id, {
                                    key: event.currentTarget.value,
                                  })}
                              />
                              {#if variable.revealed || !variable.secret || !variable.value_set}
                                <input
                                  class="env-value-input"
                                  type={variable.secret ? "password" : "text"}
                                  placeholder="value"
                                  value={variable.revealed || !variable.secret
                                    ? variable.value
                                    : ""}
                                  disabled={!variable.editing && !variable.draft}
                                  oninput={(event) =>
                                    updateGlobalEnvironmentDraft(variable.id, {
                                      value: event.currentTarget.value,
                                    })}
                                />
                              {:else}
                                <input
                                  class="env-value-input"
                                  type="password"
                                  placeholder="••••••••"
                                  disabled
                                />
                              {/if}
                              <label class="env-toggle" title="Secret">
                                <input
                                  type="checkbox"
                                  checked={variable.secret}
                                  disabled={!variable.editing && !variable.draft}
                                  onchange={(event) =>
                                    updateGlobalEnvironmentDraft(variable.id, {
                                      secret: event.currentTarget.checked,
                                    })}
                                />
                                <span>Secret</span>
                              </label>
                              <label class="env-toggle" title="Enabled">
                                <input
                                  type="checkbox"
                                  checked={variable.enabled}
                                  onchange={(event) =>
                                    updateGlobalEnvironmentDraft(variable.id, {
                                      enabled: event.currentTarget.checked,
                                    })}
                                />
                                <span>On</span>
                              </label>
                            </div>
                            <div class="env-row-actions">
                              {#if variable.draft || variable.editing}
                                <button
                                  type="button"
                                  class="env-save-btn"
                                  onclick={() => saveGlobalEnvironmentEntry(variable)}
                                  disabled={globalEnvironmentLoading}
                                >
                                  Save
                                </button>
                              {:else if variable.secret && variable.value_set && !variable.revealed}
                                <button
                                  type="button"
                                  class="env-reveal-btn"
                                  onclick={() => revealGlobalEnvironment(variable.id)}
                                >
                                  Reveal
                                </button>
                              {:else if variable.secret && variable.revealed}
                                <button
                                  type="button"
                                  class="env-hide-btn"
                                  onclick={() => hideGlobalEnvironment(variable.id)}
                                >
                                  Hide
                                </button>
                              {/if}
                              <button
                                type="button"
                                class="env-edit-btn"
                                onclick={() =>
                                  updateGlobalEnvironmentDraft(variable.id, { editing: true })}
                                disabled={variable.draft || variable.editing}
                              >
                                Edit
                              </button>
                              <button
                                type="button"
                                class="env-delete-btn"
                                onclick={() => removeGlobalEnvironment(variable.id)}
                                disabled={globalEnvironmentLoading}
                              >
                                Delete
                              </button>
                            </div>
                          </div>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/if}
                {#if settingsTab === "jira"}
                  <div class="jira-section settings-section">
                    <div>
                      <p class="section-kicker">Jira</p>
                      <h3>Jira Board Sync</h3>
                    </div>
                    <p class="field-help">
                      Configure Jira once. These credentials power board sync, card transitions,
                      Jira comments, and the selected Jira MCP connection.
                    </p>

                    <label>
                      <span>Jira MCP Runtime</span>
                      <select
                        value={settings.jira.serverId}
                        oninput={(event) => {
                          selectedServerId = event.currentTarget.value;
                          settings = {
                            ...settings,
                            jira: { ...settings.jira, serverId: event.currentTarget.value },
                          };
                        }}
                      >
                        {#each settings.mcpServers as server (server.id)}
                          <option value={server.id}
                            >{server.name || "Unnamed MCP"} ({server.kind})</option
                          >
                        {/each}
                      </select>
                    </label>
                    <p class="field-help">
                      Only the MCP command lives in the MCP tab. Its Jira identity is inherited from
                      this page so credentials do not drift.
                    </p>

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
                              authMode: event.currentTarget
                                .value as AppSettings["jira"]["authMode"],
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
                          <span>{settings.jira.authMode === "password" ? "Username" : "Email"}</span
                          >
                          <input
                            placeholder={settings.jira.authMode === "password"
                              ? "jira-user"
                              : "you@company.com"}
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
                          placeholder={jiraSecrets.api_token
                            ? "Saved securely. Enter a new token to replace it."
                            : "Paste API token here"}
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
                          placeholder={jiraSecrets.personal_access_token
                            ? "Saved securely. Enter a new token to replace it."
                            : "Paste PAT here"}
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
                          placeholder={jiraSecrets.password
                            ? "Saved securely. Enter a new password to replace it."
                            : "Jira password"}
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
                              jira: {
                                ...settings.jira,
                                boardNameFilter: event.currentTarget.value,
                              },
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
                              jira: {
                                ...settings.jira,
                                pageSize: Number(event.currentTarget.value),
                              },
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
                              jira: {
                                ...settings.jira,
                                maxPages: Number(event.currentTarget.value),
                              },
                            })}
                        />
                      </label>
                    </div>
                    <p class="field-help">
                      {syncBudgetLabel} Keep this small for daily use; use a narrower JQL instead of fetching
                      many pages.
                    </p>
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
                              jira: {
                                ...settings.jira,
                                boardIssuesToolName: event.currentTarget.value,
                              },
                            })}
                        />
                      </label>
                    </div>
                    <label>
                      <span>JQL</span>
                      <div class="jql-presets">
                        <button type="button" onclick={() => applyJqlPreset("assigned")}
                          >Assigned to me</button
                        >
                        <button type="button" onclick={() => applyJqlPreset("unassigned_todo")}
                          >Todo + unassigned</button
                        >
                        <button type="button" onclick={() => applyJqlPreset("unresolved")}
                          >All unresolved</button
                        >
                      </div>
                      <textarea
                        value={settings.jira.jql}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, jql: event.currentTarget.value },
                          })}></textarea>
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
                      <span
                        >Current Spacesly theme. Future color, density, and typography controls will
                        live here instead of crowding integration settings.</span
                      >
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
                      {#each selectedMcpTools as tool (tool)}
                        <code>{tool}</code>
                      {/each}
                    </div>
                  </details>
                {/if}

                <footer>
                  {#if settingsTab === "mcp"}
                    <button type="button" onclick={removeSelectedServer}> Remove </button>
                    <button type="button" onclick={disconnectSelectedMcpServer}>
                      Disconnect
                    </button>
                    <button
                      type="button"
                      onclick={testSelectedMcpConnection}
                      disabled={testingConnection}
                    >
                      {testingConnection ? "Testing..." : "Test connection"}
                    </button>
                  {/if}
                  {#if settingsTab === "jira"}
                    <button
                      class="connect-jira"
                      type="button"
                      onclick={connectJira}
                      disabled={connectingJira}
                    >
                      {connectingJira ? "Connecting..." : "Connect Jira"}
                    </button>
                    <button type="button" onclick={testJiraConnection} disabled={testingConnection}>
                      {testingConnection ? "Testing..." : "Test connection"}
                    </button>
                  {/if}
                  {#if settingsTab === "agent"}
                    <button
                      class="connect-jira"
                      type="button"
                      onclick={testWorkerConnection}
                      disabled={testingWorker}
                    >
                      {testingWorker ? "Testing Agent..." : "Test Agent"}
                    </button>
                  {/if}
                  <button class="save-settings" type="button" onclick={persistSettings}
                    >Save settings</button
                  >
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
          hidden={workspaceMode !== "board"}
          class:with-console={agentConsoleOpen && hasAgentConsoleSession}
          class="workspace-body"
          style={agentConsoleOpen && hasAgentConsoleSession
            ? `--agent-console-width: ${layoutPrefs.agentConsoleWidth}px; --lane-width: ${layoutPrefs.laneWidth}px;`
            : `--lane-width: ${layoutPrefs.laneWidth}px;`}
        >
          <BoardWorkspace
            {displayColumns}
            {selectedCardId}
            {draggedCardId}
            {runningWorkerCardIds}
            agentTaskProjections={agentTaskCardProjections}
            runningAgentSessions={runningAgentTaskSessions}
            cardMinHeight={layoutPrefs.cardMinHeight}
            {doneVisibleLimit}
            {hasAgentConsoleSession}
            {agentConsoleOpen}
            agentRunStatus={visibleAgentRunStatus}
            agentRunProgress={visibleAgentRunProgress}
            onResizeLane={(event) => beginLayoutResize(event, "laneWidth", 260, 460, "x")}
            onResizeCard={(event) => beginLayoutResize(event, "cardMinHeight", 170, 360, "y")}
            onOpenAgentConsole={openAgentConsole}
            onOpenAgentSession={(cardId) => {
              const card = activeCardById.get(cardId);
              if (card) openAgentRunForCard(card);
            }}
            onDropCard={(cardId, columnId) => void moveCardAndSync(cardId, columnId)}
            onSelectCard={selectCard}
            onQueueCard={queueCard}
            onStartAgent={(cardId) => void startWorkerForCard(cardId)}
            onMarkDone={requestManualDoneConfirmation}
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
            {canStartAgent}
            {agentActionLabel}
            {executionLabel}
            {ticketLabel}
            {isBlocked}
            {operatorNotesForCard}
          />

          {#if agentConsoleOpen && hasAgentConsoleSession}
            <div class="grid-resize-handle">
              <span
                class="drag-handle horizontal"
                role="separator"
                aria-orientation="horizontal"
                onpointerdown={(event) =>
                  beginLayoutResize(event, "agentConsoleWidth", 360, 720, "x", true)}
                onpointermove={moveLayoutResize}
                onpointerup={endLayoutResize}
                onpointercancel={endLayoutResize}
              ></span>
            </div>
            {#if agentConsoleModule}
              {@const AgentConsolePanel = agentConsoleModule.default}
              <AgentConsolePanel
                style=""
                title={visibleAgentRunTitle}
                status={visibleAgentRunStatus}
                progress={visibleAgentRunProgress}
                logs={visibleAgentRunLogs}
                transcript={visibleAgentRunTranscript}
                output={visibleAgentRunOutput}
                result={visibleAgentRunResult}
                executionRun={visibleExecutionRun}
                runStatus={visibleAgentRunStatus}
                terminalLines={visibleAgentTerminalLines}
                terminalInput={agentTerminalInput}
                runCardId={agentConsoleCardId}
                onClose={() => (agentConsoleOpen = false)}
                onCancel={cancelAgentRunForCard}
                onTerminalInputChange={(value) => (agentTerminalInput = value)}
                onSubmitTerminalInput={submitAgentTerminalInput}
                onOpenCard={(cardId) => (selectedCardId = cardId)}
                onMarkBlockedDone={requestManualDoneConfirmation}
              />
            {:else}
              <aside class="agent-console" aria-label="Agent run console loading">
                <header>
                  <div>
                    <p>Agent Console</p>
                    <h3>{visibleAgentRunTitle}</h3>
                  </div>
                  <div class={`run-state ${visibleAgentRunStatus}`}>{visibleAgentRunStatus}</div>
                  <button
                    type="button"
                    aria-label="Close Agent console"
                    onclick={() => (agentConsoleOpen = false)}>×</button
                  >
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
            <button
              class="close-detail"
              type="button"
              aria-label="Close"
              onclick={() => (selectedCardId = null)}>×</button
            >
            <div class="task-status waiting">
              <span></span>
              <strong>{executionLabel(selectedCard.execution)}</strong>
            </div>
            <h3>{selectedCard.title}</h3>
            <p>
              {#each descriptionParts(selectedCard.description) as part (part.url || part.text)}
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
                    <a href={selectedCard.url} target="_blank" rel="noreferrer"
                      >{ticketLabel(selectedCard)}</a
                    >
                  {:else}
                    {ticketLabel(selectedCard)}
                  {/if}
                </dd>
              </div>
              <div>
                <dt>Status</dt>
                <dd>
                  {selectedCardAgentSession?.taskSessionState ??
                    executionDetail(selectedCard.execution)}
                  {#if selectedCardAgentSession}
                    · {selectedCardAgentSession.progress}%
                  {/if}
                </dd>
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
                    disabled={!canStartAgent(
                      selectedCard,
                      Boolean(runningWorkerCardIds[selectedCard.id]),
                    )}
                    onclick={() => void startWorkerForCard(selectedCard.id)}
                  >
                    {agentActionLabel(
                      selectedCard,
                      Boolean(runningWorkerCardIds[selectedCard.id]),
                      Boolean(operatorNotesForCard(selectedCard.id)),
                    )}
                  </button>
                {/if}
                {#if selectedCardAgentSession}
                  <button
                    type="button"
                    class="open-console-action"
                    onclick={() => openAgentConsole(selectedCard)}
                  >
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
          hidden={workspaceMode !== "files"}
          class="files-workspace"
          style={`--file-sidebar-width: ${layoutPrefs.fileSidebarWidth}px;`}
        >
          <div class="files-sidebar">
            <SegmentedControl
              ariaLabel="Workspace sidebar tabs"
              activeValue={workspaceSidebarTab}
              items={[
                { value: "explorer", label: "Explorer" },
                {
                  value: "search",
                  label: "Search",
                  badge: workspaceSearchResults.length || undefined,
                },
                {
                  value: "source-control",
                  label: "Source Control",
                  badge: sourceControlChangedCount > 0 ? sourceControlChangedCount : undefined,
                },
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
            {:else if workspaceSidebarTab === "search"}
              {#if workspaceSearchModule}
                {@const WorkspaceSearchPane = workspaceSearchModule.default}
                <WorkspaceSearchPane
                  query={workspaceSearchQuery}
                  caseSensitive={workspaceSearchCaseSensitive}
                  results={workspaceSearchResults}
                  loading={workspaceSearchLoading}
                  error={workspaceSearchError}
                  filesSearched={workspaceSearchFilesSearched}
                  truncated={workspaceSearchTruncated}
                  replaceOpen={workspaceReplaceOpen}
                  replacement={workspaceReplacement}
                  replacePreview={workspaceReplacePreview}
                  replaceLoading={workspaceReplaceLoading}
                  replaceApplying={workspaceReplaceApplying}
                  replaceError={workspaceReplaceError}
                  onQueryChange={updateWorkspaceSearchQuery}
                  onCaseSensitiveChange={updateWorkspaceSearchCaseSensitive}
                  onOpenResult={(result) => void openWorkspaceSearchResult(result)}
                  onToggleReplace={() => {
                    workspaceReplaceOpen = !workspaceReplaceOpen;
                    if (!workspaceReplaceOpen) invalidateWorkspaceReplacePreview();
                  }}
                  onReplacementChange={updateWorkspaceReplacement}
                  onPreviewReplace={() => void previewWorkspaceReplacement()}
                  onApplyReplace={() => void applyWorkspaceReplacement()}
                />
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
                  onOpenFile={(path) =>
                    void openFileEntry({ name: fileName(path), path, is_dir: false, size: 0 })}
                />
              {:else}
                <aside
                  class="git-actions-pane git-actions-loading"
                  aria-label="Git actions loading"
                >
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
            <button
              class="file-sidebar-rail"
              type="button"
              onclick={toggleFileSidebar}
              aria-label="Show file browser"
            >
              &gt;
            </button>
          {:else}
            <div class="grid-resize-handle file-resize-handle">
              <span
                class="drag-handle horizontal"
                role="separator"
                aria-orientation="horizontal"
                onpointerdown={(event) =>
                  beginLayoutResize(event, "fileSidebarWidth", 240, 560, "x")}
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
              onExecuteCommand={executeEditorCommand}
              onSelectEditorTab={selectEditorTab}
              onCloseEditorTab={closeEditorTab}
              onSetEditorDirty={setEditorDirty}
              {onEditorChange}
              {onEditorReady}
              vimMode={editorVimMode}
              onToggleVimMode={toggleEditorVimMode}
              onReloadActiveFile={() => void reloadActiveFileFromDisk()}
              lspDiagnostics={activeLspDiagnostics}
              lspStatus={activeLspStatus}
              onLspHover={requestLspHover}
              onLspCompletion={requestLspCompletion}
              lspCodeActions={activeLspCodeActions}
              {lspCodeActionsLoading}
              onRequestLspCodeActions={() => void requestLspCodeActions()}
              onApplyLspCodeAction={applyLspCodeAction}
              {aiEditProposal}
              {aiEditGenerating}
              {aiEditStale}
              {aiEditSelectedHunkIds}
              {aiEditError}
              onRequestAiEdit={(instruction) => void requestAiEdit(instruction)}
              onCancelAiEdit={cancelAiEdit}
              onToggleAiEditHunk={toggleAiEditHunk}
              onApplySelectedAiEdit={() => applyAiEdit(aiEditSelectedHunkIds)}
              onAcceptAllAiEdit={() =>
                applyAiEdit(aiEditProposal?.hunks.map((hunk) => hunk.id) ?? [])}
              onRejectAiEdit={rejectAiEdit}
              aiEditContextDocuments={aiEditContextOptions}
              {aiEditContextCharacters}
              onToggleAiEditContext={toggleAiEditContext}
              canNavigateBack={canNavigateEditorBack}
              canNavigateForward={canNavigateEditorForward}
              lspSymbols={activeLspSymbols}
              {lspSymbolsLoading}
              onRefreshLspSymbols={() => void refreshActiveLspSymbols(true)}
              onSelectLspSymbol={(symbol) => void navigateToDocumentSymbol(symbol)}
              lspReferences={activeLspReferences}
              {lspReferencesLoading}
              onSelectLspReference={(location) => void navigateToReference(location)}
              onCloseLspReferences={() => {
                activeLspReferences = [];
                lspReferenceRequestId += 1;
                lspReferencesLoading = false;
              }}
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

        <div
          hidden={workspaceMode !== "term"}
          class="term-workspace"
          style={`--terminal-width: ${layoutPrefs.terminalWidth}px;`}
        >
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
              title={settings.aiWorker.runtime === "opencode"
                ? settings.aiWorker.opencodeModel
                : selectedAiModel.label}
              onOpenRuntimeSettings={() => openSettings("agent")}
              sessions={workspaceChatSessions}
              sessionStatuses={workspaceChatSessionStatuses}
              activeSessionId={workspaceChatActiveSessionId}
              onNewSession={startWorkspaceChatSession}
              onSwitchSession={activateWorkspaceChatSession}
              messages={workspaceChatMessages}
              streamingText={activeWorkspaceChatRun.streamingText}
              running={activeWorkspaceChatRun.running}
              actionProposal={activeWorkspaceChatRun.actionProposal?.sessionId ===
              workspaceChatSession.id
                ? activeWorkspaceChatRun.actionProposal
                : null}
              onApplyActionProposal={() => void applyWorkspaceChatActionProposal()}
              onRejectActionProposal={rejectWorkspaceChatActionProposal}
              onCancel={cancelWorkspaceChat}
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
        <div
          class="confirm-backdrop"
          role="presentation"
          onclick={() => resolveBacklogStartConfirmation(false)}
        ></div>
        <div
          class="confirm-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="confirm-backlog-start-title"
        >
          <header>
            <div>
              <p>Confirm start</p>
              <h2 id="confirm-backlog-start-title">Start backlog task?</h2>
            </div>
            <button
              type="button"
              aria-label="Close confirmation"
              onclick={() => resolveBacklogStartConfirmation(false)}>×</button
            >
          </header>
          <div class="confirm-body">
            <p>
              <strong>{backlogStartConfirmation.title}</strong>
              will move from Backlog to In Progress and begin Agent execution.
            </p>
            <p>This will create a running task immediately. Continue?</p>
          </div>
          <footer>
            <button type="button" onclick={() => resolveBacklogStartConfirmation(false)}
              >Cancel</button
            >
            <button
              class="confirm-primary"
              type="button"
              onclick={() => resolveBacklogStartConfirmation(true)}>Start Agent</button
            >
          </footer>
        </div>
      {/if}
      {#if manualDoneConfirmation}
        <div
          class="confirm-backdrop"
          role="presentation"
          onclick={() => void confirmManualDone(false)}
        ></div>
        <div
          class="confirm-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="confirm-manual-done-title"
        >
          <header>
            <div>
              <p>Confirm manual completion</p>
              <h2 id="confirm-manual-done-title">Mark task Done?</h2>
            </div>
            <button
              type="button"
              aria-label="Close confirmation"
              onclick={() => void confirmManualDone(false)}>×</button
            >
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
            <button
              class="confirm-primary"
              type="button"
              onclick={() => void confirmManualDone(true)}>Done</button
            >
          </footer>
        </div>
      {/if}
    {:else if workspace}
      <section class="state-panel">
        <strong>Unable to open workspace board</strong>
        <p>
          The loaded workspace does not contain a board projection. Clear the saved workspace or
          sync Jira again.
        </p>
      </section>
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
