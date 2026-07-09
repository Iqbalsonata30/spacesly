<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import AgentOutput from "$lib/components/AgentOutput.svelte";
  import McpConnectionSettings from "$lib/components/McpConnectionSettings.svelte";
  import TaskCard from "$lib/components/TaskCard.svelte";
  import "@xterm/xterm/css/xterm.css";
  import "./page.css";
  import {
    addJiraComment,
    assignJiraIssue,
    chatAiWorker,
    executeAiWorkerTask,
    getJiraBoards,
    getWorkspace,
    openPtyTerminal,
    resizePtyTerminal,
    syncJiraWorkspace,
    testAiWorker,
    testMcpServerConnection,
    transitionJiraIssue,
    writePtyTerminal,
    type AiWorkerConfig,
    type AiWorkerStatus,
    type BoardProjection,
    type CardProjection,
    type CardSource,
    type ColumnIntent,
    type ExecutionState,
    type JiraMcpConfig,
    type JiraBoard,
    type WorkspaceProjection,
    testJiraMcpConnection,
  } from "$lib/ipc";
  import {
    createMcpServer,
    loadSettings,
    saveSettings,
    type AppSettings,
  } from "$lib/settings";
  import { aiProviders, defaultModelForProvider, modelById, providerById } from "$lib/aiModels";
  import { loadCachedWorkspace, saveCachedWorkspace } from "$lib/workspaceCache";

  const initialSettings = loadSettings();
  const cachedWorkspace = loadCachedWorkspace();
  const AGENT_STATUS_KEY = "spacesly.agent.status.v1";
  const MAX_AGENT_LOGS = 120;
  const MAX_AGENT_TERMINAL_LINES = 80;
  const MAX_WORKSPACE_CHAT_MESSAGES = 80;
  const MAX_AGENT_OUTPUT_CHARS = 32_000;
  const LAYOUT_PREFS_KEY = "spacesly.layout.v1";
  const UI_STATE_KEY = "spacesly.ui.v1";

  type AgentRunLog = {
    id: string;
    at: string;
    tone: "info" | "success" | "error";
    label: string;
    message: string;
  };

  type AgentTerminalLine = {
    id: string;
    prompt: string;
    text: string;
  };

  type WorkspaceChatMessage = {
    id: string;
    role: "user" | "agent" | "system";
    text: string;
  };

  type WorkspaceChatAction =
    | { type: "create_task"; title: string; description?: string }
    | { type: "move_card"; card_id?: string; ticket?: string; title?: string; target: "todo" | "ready" | "in_progress" | "done" }
    | { type: "start_agent"; card_id?: string; ticket?: string; title?: string }
    | { type: "select_card"; card_id?: string; ticket?: string; title?: string }
    | { type: "delete_card"; card_id?: string; ticket?: string; title?: string }
    | { type: "sync_jira" };

  type LayoutPrefs = {
    laneWidth: number;
    cardMinHeight: number;
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

  type UiState = {
    workspaceMode: "board" | "term";
    workspaceShellWorkdir: string;
    workspaceChatMessages: WorkspaceChatMessage[];
  };

  const defaultLayoutPrefs: LayoutPrefs = {
    laneWidth: 300,
    cardMinHeight: 220,
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
      text: "Ask the Agent about terminal output or tell it to create, move, open, sync, or start work on board tasks.",
    },
  ];
  const initialUiState = loadUiState();

  type AgentConnectionState = {
    connected: boolean;
    testedAt: number;
    message: string;
  };

  type AgentRunSession = {
    cardId: string;
    title: string;
    status: "idle" | "running" | "completed" | "blocked";
    progress: number;
    output: string;
    logs: AgentRunLog[];
    terminalLines: AgentTerminalLine[];
  };

  type AgentPhaseKey = "prepare" | "jira" | "model" | "execute" | "writeback" | "done";
  type AgentPhase = {
    key: AgentPhaseKey;
    label: string;
    state: "pending" | "active" | "done" | "blocked";
  };

  let workspace = $state<WorkspaceProjection | null>(cachedWorkspace?.workspace ?? null);
  let cacheSavedAt = $state<number | null>(cachedWorkspace?.savedAt ?? null);
  let error = $state<string | null>(null);
  let syncError = $state<string | null>(null);
  let syncing = $state(false);
  let testingConnection = $state(false);
  let loadingBoards = $state(false);
  let connectingJira = $state(false);
  let testingWorker = $state(false);
  let runningWorkerCardId = $state<string | null>(null);
  let connectionMessage = $state<string | null>(null);
  let workerStatus = $state<AiWorkerStatus | null>(null);
  let agentConnectionStates = $state<Record<string, AgentConnectionState>>(loadAgentConnectionStates());
  let appNotice = $state<{ tone: "info" | "success" | "error"; message: string } | null>(
    cachedWorkspace
      ? { tone: "info", message: "Loaded saved cards instantly. Sync Jira only when you need fresh updates." }
      : null,
  );
  let discoveredTools = $state<string[]>([]);
  let settingsOpen = $state(false);
  let settingsTab = $state<"agent" | "rules" | "skills" | "mcp" | "jira" | "theme">("agent");
  let settingsError = $state<string | null>(null);
  let settings = $state<AppSettings>(initialSettings);
  let selectedServerId = $state(initialSettings.jira.serverId);
  let workspaceMode = $state<"board" | "term">(initialUiState.workspaceMode);
  let now = $state(new Date());
  let selectedCardId = $state<string | null>(null);
  let draggedCardId = $state<string | null>(null);
  let newTaskOpen = $state(false);
  let newTaskTitle = $state("");
  let newTaskDescription = $state("");
  let agentConsoleOpen = $state(false);
  let agentRunCardId = $state<string | null>(null);
  let agentRunTitle = $state("No active run");
  let agentRunStatus = $state<"idle" | "running" | "completed" | "blocked">("idle");
  let agentRunProgress = $state(0);
  let agentRunOutput = $state("");
  let agentRunLogs = $state<AgentRunLog[]>([]);
  let agentTerminalLines = $state<AgentTerminalLine[]>([]);
  let agentTerminalInput = $state("");
  let agentRunSessions = $state<Record<string, AgentRunSession>>({});
  let workspaceShellWorkdir = $state(initialUiState.workspaceShellWorkdir);
  let workspaceTerminalContainer: HTMLDivElement | null = $state(null);
  let workspaceTerminal: Terminal | null = null;
  let workspaceFitAddon: FitAddon | null = null;
  let workspaceTerminalResizeObserver: ResizeObserver | null = null;
  let workspaceTerminalOpened = $state(false);
  const workspaceTerminalId = "main-workspace-terminal";
  let workspaceChatInput = $state("");
  let workspaceChatRunning = $state(false);
  let workspaceChatMessages = $state<WorkspaceChatMessage[]>(initialUiState.workspaceChatMessages);
  let layoutPrefs = $state<LayoutPrefs>(loadLayoutPrefs());
  let layoutResizeDrag: LayoutResizeDrag | null = null;

  onMount(() => {
    const timer = window.setInterval(() => {
      now = new Date();
    }, 60_000);

    return () => window.clearInterval(timer);
  });

  $effect(() => {
    if (workspace) return;

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
      void initWorkspaceTerminal();
    }
  });

  onDestroy(() => {
    workspaceTerminalResizeObserver?.disconnect();
    workspaceTerminal?.dispose();
  });

  let activeBoard = $derived<BoardProjection | null>(
    workspace?.projects[0]?.boards[0] ?? null,
  );
  let activeCards = $derived<CardProjection[]>(
    activeBoard?.columns.flatMap((column) => column.cards) ?? [],
  );
  let activeCardById = $derived(
    new Map(activeCards.map((card) => [card.id, card])),
  );
  let activeColumnById = $derived(
    new Map(activeBoard?.columns.map((column) => [column.id, column]) ?? []),
  );
  let activeColumnByIntent = $derived(
    new Map(activeBoard?.columns.map((column) => [column.intent, column]) ?? []),
  );
  let cardColumnIntentById = $derived(
    new Map(
      activeBoard?.columns.flatMap((column) => column.cards.map((card) => [card.id, column.intent] as const)) ?? [],
    ),
  );
  let selectedCard = $derived<CardProjection | null>(
    selectedCardId ? activeCardById.get(selectedCardId) ?? null : null,
  );
  let selectedCardAgentSession = $derived<AgentRunSession | null>(
    selectedCardId ? agentRunSessions[selectedCardId] ?? null : null,
  );
  let selectedServer = $derived(
    settings.mcpServers.find((server) => server.id === selectedServerId) ??
      settings.mcpServers[0],
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
  let cacheStatusLabel = $derived(cacheSavedAt ? `Cached ${relativeTime(cacheSavedAt)}` : "No cached board");
  let syncBudgetLabel = $derived(
    `Fast sync: up to ${settings.jira.pageSize * settings.jira.maxPages} Jira cards (${settings.jira.pageSize}/page × ${settings.jira.maxPages} page${settings.jira.maxPages === 1 ? "" : "s"}).`,
  );
  let selectedAiProvider = $derived(providerById(settings.aiWorker.providerId));
  let selectedAiModel = $derived(modelById(selectedAiProvider, settings.aiWorker.modelId));
  let selectedAiApiKey = $derived(settings.aiWorker.apiKeys[selectedAiProvider.id] ?? "");
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
  let currentAgentLog = $derived(agentRunLogs.at(-1) ?? null);
  let agentActivityView = $derived(agentActivity(agentRunStatus, agentRunProgress, currentAgentLog));
  let agentActivityTitle = $derived(agentActivityView.title);
  let agentActivityDetail = $derived(agentActivityView.detail);
  let agentNextStep = $derived(agentActivityView.next);
  let agentPhases = $derived(agentPhaseTimeline(agentRunStatus, agentRunProgress));
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

  function columnTitle(name: string): string {
    if (name.toLowerCase() === "backlog") return "Todo";
    return name;
  }

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

  function loadLayoutPrefs(): LayoutPrefs {
    if (typeof localStorage === "undefined") return { ...defaultLayoutPrefs };
    try {
      const parsed = JSON.parse(localStorage.getItem(LAYOUT_PREFS_KEY) ?? "{}");
      return normalizeLayoutPrefs(parsed);
    } catch {
      return { ...defaultLayoutPrefs };
    }
  }

  function loadUiState(): UiState {
    if (typeof localStorage === "undefined") {
      return { workspaceMode: "board", workspaceShellWorkdir: "", workspaceChatMessages: defaultWorkspaceChatMessages };
    }

    try {
      const parsed = JSON.parse(localStorage.getItem(UI_STATE_KEY) ?? "{}") as Partial<UiState>;
      const messages = Array.isArray(parsed.workspaceChatMessages)
        ? parsed.workspaceChatMessages.filter(isWorkspaceChatMessage).slice(-MAX_WORKSPACE_CHAT_MESSAGES)
        : defaultWorkspaceChatMessages;
      return {
        workspaceMode: parsed.workspaceMode === "term" ? "term" : "board",
        workspaceShellWorkdir: typeof parsed.workspaceShellWorkdir === "string" ? parsed.workspaceShellWorkdir : "",
        workspaceChatMessages: messages.length > 0 ? messages : defaultWorkspaceChatMessages,
      };
    } catch {
      return { workspaceMode: "board", workspaceShellWorkdir: "", workspaceChatMessages: defaultWorkspaceChatMessages };
    }
  }

  function isWorkspaceChatMessage(value: unknown): value is WorkspaceChatMessage {
    if (!value || typeof value !== "object") return false;
    const candidate = value as Partial<WorkspaceChatMessage>;
    return typeof candidate.id === "string"
      && (candidate.role === "user" || candidate.role === "agent" || candidate.role === "system")
      && typeof candidate.text === "string";
  }

  function saveUiState() {
    localStorage.setItem(
      UI_STATE_KEY,
      JSON.stringify({
        workspaceMode,
        workspaceShellWorkdir,
        workspaceChatMessages: workspaceChatMessages.slice(-MAX_WORKSPACE_CHAT_MESSAGES),
      } satisfies UiState),
    );
  }

  function setWorkspaceMode(mode: "board" | "term") {
    workspaceMode = mode;
    saveUiState();
    if (mode === "term") void initWorkspaceTerminal();
  }

  function normalizeLayoutPrefs(value: Partial<LayoutPrefs>): LayoutPrefs {
    return {
      laneWidth: clampNumber(value.laneWidth, 260, 460, defaultLayoutPrefs.laneWidth),
      cardMinHeight: clampNumber(value.cardMinHeight, 170, 360, defaultLayoutPrefs.cardMinHeight),
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

  function sourceLabel(source: CardSource): string {
    if (source === "local") return "spacesly/local";
    return source.jira.key;
  }

  function ticketLabel(card: CardProjection): string {
    return sourceLabel(card.source);
  }

  function descriptionParts(description: string): Array<{ text: string; url?: string }> {
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

  function executionLabel(execution: ExecutionState): string {
    if (typeof execution === "string") return execution.replace("_", " ");
    if ("blocked" in execution) return "blocked";
    return "merged";
  }

  function executionDetail(execution: ExecutionState): string {
    if (typeof execution === "string") return execution.replace("_", " ");
    if ("blocked" in execution) return execution.blocked.reason;
    return execution.completed.summary;
  }

  function agentActivity(
    status: AgentRunSession["status"],
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

  function agentPhaseTimeline(status: AgentRunSession["status"], progress: number): AgentPhase[] {
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

  function isBlocked(execution: ExecutionState): boolean {
    return typeof execution === "object" && "blocked" in execution;
  }

  function canStartAgent(card: CardProjection): boolean {
    return runningWorkerCardId !== card.id && card.execution !== "running";
  }

  function agentActionLabel(card: CardProjection): string {
    if (runningWorkerCardId === card.id || card.execution === "running") return "Running";
    if (isBlocked(card.execution)) return "↻ Retry Agent";
    return "▷ Start";
  }

  function buildJiraConfig(): JiraMcpConfig | null {
    const server = settings.mcpServers.find((entry) => entry.id === settings.jira.serverId);

    const credential = credentialValue();

    if (!settings.jira.baseUrl.trim() || !credential) {
      syncError = "Open Settings and fill Jira URL plus the selected credential before syncing.";
      settingsOpen = true;
      return null;
    }

    if (settings.jira.authMode !== "pat" && !settings.jira.username.trim()) {
      syncError = "Open Settings and fill the Jira email/username required by the selected auth method.";
      settingsOpen = true;
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
        api_token: settings.jira.apiToken,
        personal_access_token: settings.jira.personalAccessToken,
        password: settings.jira.password,
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
    if (settings.aiWorker.runtime === "api" && !selectedAiApiKey.trim()) {
      settingsOpen = true;
      settingsTab = "agent";
      appNotice = { tone: "error", message: `Open Settings and fill ${selectedAiProvider.apiKeyLabel}.` };
      return null;
    }

    if (settings.aiWorker.runtime === "opencode" && !settings.aiWorker.opencodeCommand.trim()) {
      settingsOpen = true;
      settingsTab = "agent";
      appNotice = { tone: "error", message: "Open Settings and fill the OpenCode command." };
      return null;
    }

    return {
      runtime: settings.aiWorker.runtime,
      provider_name: selectedAiProvider.label,
      base_url: selectedAiProvider.baseUrl,
      api_style: selectedAiProvider.apiStyle,
      api_key: selectedAiApiKey,
      model: settings.aiWorker.runtime === "opencode" ? settings.aiWorker.opencodeModel : selectedAiModel.id,
      opencode_command: settings.aiWorker.opencodeCommand,
      opencode_model: settings.aiWorker.opencodeModel,
      opencode_workdir: settings.aiWorker.opencodeWorkdir.trim() || null,
      opencode_auto_approve: settings.aiWorker.opencodeAutoApprove,
      agent_rules: settings.aiWorker.agentRules,
      agent_skills: settings.aiWorker.agentSkills,
      temperature: settings.aiWorker.temperature,
    };
  }

  function appendWorkspaceChat(message: Omit<WorkspaceChatMessage, "id">) {
    workspaceChatMessages = capList([
      ...workspaceChatMessages,
      { ...message, id: `chat-${Date.now().toString(36)}-${workspaceChatMessages.length}` },
    ], MAX_WORKSPACE_CHAT_MESSAGES);
    saveUiState();
  }

  function capList<T>(items: T[], maxItems: number): T[] {
    return items.length > maxItems ? items.slice(items.length - maxItems) : items;
  }

  function capText(value: string, maxChars: number): string {
    return value.length > maxChars ? value.slice(value.length - maxChars) : value;
  }

  function terminalContext(): string {
    return "PTY terminal is active. Ask about visible terminal output or paste relevant output into chat when needed.";
  }

  async function initWorkspaceTerminal() {
    await tick();
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
    setWorkspaceMode("term");
  }

  async function sendWorkspaceChat() {
    const message = workspaceChatInput.trim();
    if (!message || workspaceChatRunning) return;

    workspaceChatInput = "";
    appendWorkspaceChat({ role: "user", text: message });

    const localActions = fastWorkspaceChatActions(message);
    if (localActions.length > 0) {
      const actionSummary = await applyWorkspaceChatActions(localActions);
      appendWorkspaceChat({ role: "system", text: actionSummary });
      return;
    }

    const config = buildAiWorkerConfig();
    if (!config) return;

    workspaceChatRunning = true;

    try {
      const result = await chatAiWorker(config, {
        message,
        terminal_context: workspaceAgentContext(),
      });
      const actions = extractWorkspaceActions(result.raw_response);
      appendWorkspaceChat({ role: "agent", text: stripWorkspaceActions(result.raw_response) });
      if (actions.length > 0) {
        const actionSummary = await applyWorkspaceChatActions(actions);
        appendWorkspaceChat({ role: "system", text: actionSummary });
      }
    } catch (reason) {
      appendWorkspaceChat({ role: "system", text: reason instanceof Error ? reason.message : String(reason) });
    } finally {
      workspaceChatRunning = false;
    }
  }

  function workspaceAgentContext(): string {
    return [terminalContext(), boardContext()].filter(Boolean).join("\n\n");
  }

  function boardContext(): string {
    if (!activeBoard) return "Board context: no active board loaded.";

    const cards = activeBoard.columns.flatMap((column) =>
      column.cards.slice(0, 20).map((card) => ({
        id: card.id,
        ticket: ticketLabel(card),
        title: card.title,
        column: columnTitle(column.name),
        intent: column.intent,
        execution: executionLabel(card.execution),
        labels: card.labels.slice(0, 4),
      })),
    ).slice(0, 80);

    return [
      `Board context: ${activeBoard.name}`,
      `Columns: ${activeBoard.columns.map((column) => `${column.intent}:${column.name}`).join(" | ")}`,
      "Cards JSON:",
      JSON.stringify(cards),
      "Allowed board actions: create_task, move_card, start_agent, select_card, delete_card, sync_jira.",
      "If you want Spacesly to mutate the board, append a final line starting with SPACESLY_ACTIONS: followed by a JSON array of actions. Use card_id when possible. Use target todo/ready/in_progress/done for move_card.",
    ].join("\n");
  }

  function extractWorkspaceActions(response: string): WorkspaceChatAction[] {
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

  function stripWorkspaceActions(response: string): string {
    return response.replace(/\n?SPACESLY_ACTIONS:\s*\[[\s\S]*\]\s*$/i, "").trim() || response.trim();
  }

  function normalizeWorkspaceAction(value: unknown): WorkspaceChatAction[] {
    if (!value || typeof value !== "object") return [];
    const action = value as Record<string, unknown>;
    const type = typeof action.type === "string" ? action.type : "";

    if (type === "create_task" && typeof action.title === "string" && action.title.trim()) {
      return [{ type, title: action.title.trim(), description: typeof action.description === "string" ? action.description : undefined }];
    }

    if (type === "move_card" && typeof action.target === "string" && isBoardTarget(action.target)) {
      return [{
        type,
        card_id: stringField(action.card_id),
        ticket: stringField(action.ticket),
        title: stringField(action.title),
        target: action.target,
      }];
    }

    if ((type === "start_agent" || type === "select_card")) {
      return [{ type, card_id: stringField(action.card_id), ticket: stringField(action.ticket), title: stringField(action.title) }];
    }

    if (type === "delete_card") {
      return [{ type, card_id: stringField(action.card_id), ticket: stringField(action.ticket), title: stringField(action.title) }];
    }

    if (type === "sync_jira") return [{ type }];
    return [];
  }

  function stringField(value: unknown): string | undefined {
    return typeof value === "string" && value.trim() ? value.trim() : undefined;
  }

  function isBoardTarget(value: string): value is "todo" | "ready" | "in_progress" | "done" {
    return value === "todo" || value === "ready" || value === "in_progress" || value === "done";
  }

  function fastWorkspaceChatActions(message: string): WorkspaceChatAction[] {
    const text = message.trim();
    const lower = text.toLowerCase();

    if (/^(sync|refresh)\s+jira\b/.test(lower)) return [{ type: "sync_jira" }];

    const createMatch = text.match(/^(?:create|add)(?:\s+a)?\s+task\s*:?\s+(.+)$/i);
    if (createMatch?.[1]) return [{ type: "create_task", title: createMatch[1].trim() }];

    const moveMatch = text.match(/^move\s+(.+?)\s+to\s+(todo|ready|in[\s_-]?progress|done)$/i);
    if (moveMatch?.[1] && moveMatch[2]) {
      return [{ type: "move_card", ...cardReference(moveMatch[1]), target: normalizeBoardTarget(moveMatch[2]) }];
    }

    const startMatch = text.match(/^start(?:\s+agent)?(?:\s+(?:on|for))?\s+(.+)$/i);
    if (startMatch?.[1]) return [{ type: "start_agent", ...cardReference(startMatch[1]) }];

    const openMatch = text.match(/^(?:open|show|select)\s+(.+)$/i);
    if (openMatch?.[1]) return [{ type: "select_card", ...cardReference(openMatch[1]) }];

    const deleteMatch = text.match(/^(?:delete|remove)\s+(.+)$/i);
    if (deleteMatch?.[1]) return [{ type: "delete_card", ...cardReference(deleteMatch[1]) }];

    return [];
  }

  function cardReference(value: string): { card_id?: string; ticket?: string; title?: string } {
    const text = value.trim().replace(/^task\s+/i, "");
    if (activeCardById.has(text)) return { card_id: text };
    if (/^[A-Z][A-Z0-9]+-\d+$/i.test(text)) return { ticket: text.toUpperCase() };
    return { title: text };
  }

  function normalizeBoardTarget(value: string): "todo" | "ready" | "in_progress" | "done" {
    return value.toLowerCase().replace(/[\s-]+/g, "_") === "in_progress" ? "in_progress" : value.toLowerCase() as "todo" | "ready" | "done";
  }

  async function applyWorkspaceChatActions(actions: WorkspaceChatAction[]): Promise<string> {
    const results: string[] = [];

    for (const action of actions.slice(0, 5)) {
      if (action.type === "create_task") {
        const card = createBoardTask(action.title, action.description ?? "Created by Spacesly Agent chat.");
        results.push(card ? `Created task "${card.title}".` : `Could not create task "${action.title}".`);
        continue;
      }

      if (action.type === "sync_jira") {
        await syncJira();
        results.push("Started Jira sync.");
        continue;
      }

      const card = resolveActionCard(action);
      if (!card) {
        results.push(`Could not find the requested card for ${action.type}.`);
        continue;
      }

      if (action.type === "select_card") {
        selectCard(card);
        setWorkspaceMode("board");
        results.push(`Opened ${ticketLabel(card)}.`);
        continue;
      }

      if (action.type === "delete_card") {
        const removed = removeCard(card.id);
        results.push(removed ? `Removed ${ticketLabel(card)} from Spacesly.` : `Could not remove ${ticketLabel(card)}.`);
        continue;
      }

      if (action.type === "start_agent") {
        await startWorkerForCard(card.id);
        results.push(`Started Agent for ${ticketLabel(card)}.`);
        continue;
      }

      if (action.type === "move_card") {
        const columnId = columnIdForChatTarget(action.target);
        if (!columnId) {
          results.push(`Could not find ${action.target} column.`);
          continue;
        }
        await moveCardAndSync(card.id, columnId);
        results.push(`Moved ${ticketLabel(card)} to ${action.target}.`);
      }
    }

    return results.length > 0 ? `Board actions: ${results.join(" ")}` : "No board actions were applied.";
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

    return null;
  }

  function columnIdForChatTarget(target: "todo" | "ready" | "in_progress" | "done"): string | null {
    const intent: ColumnIntent = target === "todo" ? "backlog" : target;
    return activeColumnByIntent.get(intent)?.id ?? null;
  }

  function credentialValue(): string {
    if (settings.jira.authMode === "pat") return settings.jira.personalAccessToken.trim();
    if (settings.jira.authMode === "password") return settings.jira.password.trim();
    return settings.jira.apiToken.trim();
  }

  function authEnv(): Record<string, string> {
    if (settings.jira.authMode === "pat") {
      return {
        JIRA_PAT: settings.jira.personalAccessToken,
        JIRA_PERSONAL_ACCESS_TOKEN: settings.jira.personalAccessToken,
        ATLASSIAN_PAT: settings.jira.personalAccessToken,
        ATLASSIAN_PERSONAL_ACCESS_TOKEN: settings.jira.personalAccessToken,
      };
    }

    if (settings.jira.authMode === "password") {
      return {
        JIRA_PASSWORD: settings.jira.password,
        ATLASSIAN_PASSWORD: settings.jira.password,
      };
    }

    return {
      JIRA_API_TOKEN: settings.jira.apiToken,
      ATLASSIAN_API_TOKEN: settings.jira.apiToken,
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
      settingsOpen = true;
      return;
    }

    syncing = true;
    syncError = null;
      appNotice = { tone: "info", message: "Refreshing from Jira. Saved cards stay visible if Jira is slow." };
    await tick();

    try {
      const projection = applyConfiguredBoardName(await syncJiraWorkspace(config));
      workspace = projection;
      cacheSavedAt = Date.now();
      saveCachedWorkspace(projection);
      const syncedCards = projection.projects[0]?.boards[0]?.columns.reduce(
        (count, column) => count + column.cards.filter((card) => card.source !== "local").length,
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

    testingConnection = true;
    connectionMessage = null;
    settingsError = null;

    try {
      const status = await testJiraMcpConnection(config);
      discoveredTools = status.tools;
      connectionMessage = status.board_count > 0 || status.issue_count > 0
        ? `Jira connected. Found ${status.board_count} board${status.board_count === 1 ? "" : "s"} and ${status.issue_count} sample ticket${status.issue_count === 1 ? "" : "s"}.`
        : `MCP connected with ${status.tool_count} tools, but Jira returned no boards or tickets yet. Try Connect Jira, a project key, or a board name.`;
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      testingConnection = false;
    }
  }

  async function testSelectedMcpConnection() {
    if (!selectedServer) return;

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
      discoveredTools = status.tools;
      connectionMessage = `Connected. ${status.tool_count} MCP tool${status.tool_count === 1 ? "" : "s"} available.`;
    } catch (reason) {
      settingsError = reason instanceof Error ? reason.message : String(reason);
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
      saveSettings(nextSettings);
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

  function persistSettings() {
    if (selectedServer?.kind === "jira" && !settings.jira.toolName.trim()) {
      settingsError = "Jira tool name is required.";
      return;
    }

    saveSettings(settings);
    settingsOpen = false;
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

  function moveCard(cardId: string, targetColumnId: string) {
    if (!workspace) return;

    let movedCard: CardProjection | null = null;
    const nextWorkspace = {
      ...workspace,
      projects: workspace.projects.map((project) => ({
        ...project,
        boards: project.boards.map((board) => ({
          ...board,
          columns: board.columns.map((column) => {
            const cards = column.cards.filter((card) => {
              if (card.id === cardId) {
                movedCard = card;
                return false;
              }
              return true;
            });

            return { ...column, cards };
          }),
        })),
      })),
    };

    if (!movedCard) return;

    workspace = {
      ...nextWorkspace,
      projects: nextWorkspace.projects.map((project) => ({
        ...project,
        boards: project.boards.map((board) => ({
          ...board,
          columns: board.columns.map((column) =>
            column.id === targetColumnId
              ? { ...column, cards: [...column.cards, movedCard as CardProjection] }
              : column,
          ),
        })),
      })),
    };
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace);
    appNotice = { tone: "info", message: "Card moved in Spacesly." };
  }

  function removeCard(cardId: string): boolean {
    if (!workspace) return false;

    const card = activeCardById.get(cardId);
    if (!card || card.execution === "running") {
      appNotice = { tone: "error", message: "Running cards cannot be removed." };
      return false;
    }

    workspace = {
      ...workspace,
      projects: workspace.projects.map((project) => ({
        ...project,
        boards: project.boards.map((board) => ({
          ...board,
          columns: board.columns.map((column) => ({
            ...column,
            cards: column.cards.filter((entry) => entry.id !== cardId),
          })),
        })),
      })),
    };

    if (selectedCardId === cardId) selectedCardId = null;
    if (agentRunCardId === cardId) agentConsoleOpen = false;
    const { [cardId]: _removed, ...remainingSessions } = agentRunSessions;
    agentRunSessions = remainingSessions;
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace);
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

    workspace = {
      ...workspace,
      projects: workspace.projects.map((project) => ({
        ...project,
        boards: project.boards.map((board) => ({
          ...board,
          columns: board.columns.map((column) =>
            column.intent === "backlog" ? { ...column, cards: [...column.cards, card] } : column,
          ),
        })),
      })),
    };
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace);
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
    appNotice = { tone: "success", message: "Task created. Click Start or drag it to Ready when you want the Agent to run it." };
  }

  function beginAgentRun(card: CardProjection) {
    agentConsoleOpen = true;
    agentRunCardId = card.id;
    agentRunTitle = card.title;
    agentRunStatus = "running";
    agentRunProgress = 5;
    agentRunOutput = "Waiting for Agent output...";
    agentRunLogs = [];
    agentTerminalLines = [
      {
        id: `term-${Date.now().toString(36)}`,
        prompt: "system",
        text: "Agent execution session opened. Use the input below for approvals, constraints, or operator notes.",
      },
    ];
    appendAgentLog("info", "start", `Agent run started for ${ticketLabel(card)}.`);
  }

  function activeAgentSession(): AgentRunSession | null {
    if (!agentRunCardId) return null;

    return {
      cardId: agentRunCardId,
      title: agentRunTitle,
      status: agentRunStatus,
      progress: agentRunProgress,
      output: agentRunOutput,
      logs: agentRunLogs,
      terminalLines: agentTerminalLines,
    };
  }

  function persistActiveAgentRun() {
    const session = activeAgentSession();
    if (!session) return;

    agentRunSessions = {
      ...agentRunSessions,
      [session.cardId]: session,
    };
  }

  function openAgentRunForCard(card: CardProjection) {
    const session = agentRunSessions[card.id];
    if (!session) {
      appNotice = { tone: "info", message: "This card does not have an Agent terminal session yet." };
      return;
    }

    agentConsoleOpen = true;
    agentRunCardId = session.cardId;
    agentRunTitle = session.title;
    agentRunStatus = session.status;
    agentRunProgress = session.progress;
    agentRunOutput = session.output;
    agentRunLogs = session.logs;
    agentTerminalLines = session.terminalLines;
    agentTerminalInput = "";
  }

  function hasTerminalState(card: CardProjection): boolean {
    return card.execution === "running" || (typeof card.execution === "object" && "completed" in card.execution);
  }

  function selectCard(card: CardProjection) {
    selectedCardId = card.id;
    if (hasTerminalState(card) && agentRunSessions[card.id]) {
      openAgentRunForCard(card);
    }
  }

  function setAgentRunStatus(status: AgentRunSession["status"]) {
    agentRunStatus = status;
    persistActiveAgentRun();
  }

  function setAgentRunOutput(output: string) {
    agentRunOutput = capText(output, MAX_AGENT_OUTPUT_CHARS);
    persistActiveAgentRun();
  }

  function setAgentProgress(value: number) {
    agentRunProgress = Math.max(agentRunProgress, Math.min(100, value));
    persistActiveAgentRun();
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

  function submitAgentTerminalInput() {
    const input = agentTerminalInput.trim();
    if (!input) return;

    appendTerminalLine("operator", input);
    appendAgentLog("info", "operator", input);
    agentTerminalInput = "";
  }

  function agentJiraComment(resultSummary: string, resultBody: string): string {
    const details = resultBody.trim().slice(0, 3500);
    return [
      "Spacesly Agent completed this task.",
      "",
      `Provider: ${selectedAiProvider.label}`,
      `Model: ${selectedAiModel.label}`,
      "",
      "Summary:",
      resultSummary,
      "",
      "Details:",
      details || resultSummary,
    ].join("\n");
  }

  function updateCardExecution(cardId: string, execution: ExecutionState) {
    if (!workspace) return;

    workspace = {
      ...workspace,
      projects: workspace.projects.map((project) => ({
        ...project,
        boards: project.boards.map((board) => ({
          ...board,
          columns: board.columns.map((column) => ({
            ...column,
            cards: column.cards.map((card) => card.id === cardId ? { ...card, execution } : card),
          })),
        })),
      })),
    };
    cacheSavedAt = Date.now();
    saveCachedWorkspace(workspace);
  }

  function columnIdByIntent(intent: "ready" | "in_progress" | "done"): string | null {
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

    if (column.intent === "ready" || column.intent === "in_progress") return "In Progress";
    if (column.intent === "done") return "Done";
    return null;
  }

  function shouldStartWorkerForColumn(columnId: string): boolean {
    const column = activeColumnById.get(columnId);
    return column?.intent === "ready" || column?.intent === "in_progress";
  }

  async function moveCardAndSync(cardId: string, targetColumnId: string) {
    if (shouldStartWorkerForColumn(targetColumnId)) {
      await startWorkerForCard(cardId);
      return;
    }

    const card = activeCardById.get(cardId);
    const issueKey = card ? jiraKey(card) : null;
    const targetStatus = jiraTargetStatus(targetColumnId);

    moveCard(cardId, targetColumnId);

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
    const card = activeCardById.get(cardId);
    if (!card) return;

    const config = buildAiWorkerConfig();
    if (!config) return;

    const issueKey = jiraKey(card);
    const inProgressColumnId = columnIdByIntent("in_progress");
    const doneColumnId = columnIdByIntent("done");
    if (!inProgressColumnId || !doneColumnId) return;

    runningWorkerCardId = cardId;
    beginAgentRun(card);
    const runtimeLabel = config.runtime === "opencode" ? `OpenCode ${config.opencode_model}` : selectedAiModel.label;
    appNotice = { tone: "info", message: `Agent started ${ticketLabel(card)} with ${runtimeLabel}.` };

    try {
      if (cardColumnIntent(cardId) !== "in_progress") {
        appendAgentLog("info", "board", "Moved card to In Progress locally.");
        moveCard(cardId, inProgressColumnId);
      }
      updateCardExecution(cardId, "running");
      setAgentProgress(15);
      appendAgentLog("info", "model", config.runtime === "opencode" ? `Using OpenCode / ${config.opencode_model}.` : `Using ${selectedAiProvider.label} / ${selectedAiModel.label}.`);

      if (issueKey) {
        const jiraConfig = buildJiraConfig();
        if (jiraConfig) {
          appendAgentLog("info", "jira", `Assigning ${issueKey} and moving Jira to In Progress.`);
          setAgentProgress(25);
          await assignJiraIssue(jiraConfig, issueKey);
          await transitionJiraIssue(jiraConfig, issueKey, "In Progress");
          appendAgentLog("success", "jira", `${issueKey} is In Progress in Jira.`);
          setAgentProgress(35);
        }
      } else {
        appendAgentLog("info", "local", "Local Spacesly task. Jira sync is not required.");
        setAgentProgress(35);
      }

      appendAgentLog("info", "agent", "Sending task context to Agent.");
      setAgentRunOutput("Agent is processing the task context...");
      setAgentProgress(55);
      const result = await executeAiWorkerTask(config, {
        key: issueKey,
        title: card.title,
        description: card.description,
        labels: card.labels,
        url: card.url,
      });
      appendAgentLog(result.completion_status === "completed" ? "success" : "error", "agent", result.summary);
      setAgentRunOutput(result.raw_response);
      appendTerminalLine("agent", result.raw_response);
      setAgentProgress(75);

      if (result.completion_status !== "completed") {
        const reason = result.blocked_reason ?? result.summary;
        updateCardExecution(cardId, { blocked: { reason } });
        setAgentRunStatus("blocked");
        appendAgentLog("error", "blocked", "Agent did not complete and verify the requested work. Card will not move to Done.");
        appNotice = { tone: "error", message: `${ticketLabel(card)} blocked: ${reason}` };
        return;
      }

      updateCardExecution(cardId, { completed: { summary: result.summary } });
      appendAgentLog("info", "board", "Stored Agent summary on card and moved card to Done locally.");
      moveCard(cardId, doneColumnId);
      setAgentProgress(82);

      if (issueKey) {
        const jiraConfig = buildJiraConfig();
        if (jiraConfig) {
          appendAgentLog("info", "jira", `Posting Agent comment to ${issueKey}.`);
          setAgentProgress(88);
          await addJiraComment(jiraConfig, issueKey, agentJiraComment(result.summary, result.raw_response));
          appendAgentLog("success", "jira", `Agent comment posted to ${issueKey}.`);
          appendAgentLog("info", "jira", `Moving ${issueKey} to Done.`);
          setAgentProgress(94);
          await transitionJiraIssue(jiraConfig, issueKey, "Done");
          appendAgentLog("success", "jira", `${issueKey} is Done in Jira.`);
        }
      }

      setAgentRunStatus("completed");
      setAgentProgress(100);
      appNotice = { tone: "success", message: `${ticketLabel(card)} completed by Agent and moved to Done.` };
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      updateCardExecution(cardId, { blocked: { reason: message } });
      setAgentRunStatus("blocked");
      setAgentRunOutput(message);
      appendTerminalLine("error", message);
      appendAgentLog("error", "blocked", message);
      appNotice = { tone: "error", message };
    } finally {
      runningWorkerCardId = null;
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

      <button class="icon-button" type="button" aria-label="Settings" onclick={() => (settingsOpen = true)}>Settings</button>

      <nav class="mode-switch" aria-label="Workspace mode">
        <button class:active={workspaceMode === "board"} type="button" aria-label="Board view" onclick={() => setWorkspaceMode("board")}>Board</button>
        <button type="button" aria-label="Files view" disabled>Files</button>
        <button class:active={workspaceMode === "term"} type="button" aria-label="Terminal view" onclick={openTermWorkspace}>Term</button>
      </nav>

      <button
        class:connected={workerConnected}
        class="worker-pill"
        type="button"
        title={workerStatusLabel}
        onclick={() => (settingsOpen = true)}
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
            <button type="button" onclick={() => (settingsOpen = false)}>×</button>
          </header>

          <div class:mcp={settingsTab === "mcp"} class="settings-grid">
            <aside class="settings-nav" aria-label="Settings navigation">
              <button class:active={settingsTab === "agent"} type="button" onclick={() => (settingsTab = "agent")}>
                <strong>Agent</strong>
                <span>Model and worker runtime</span>
              </button>
              <button class:active={settingsTab === "rules"} type="button" onclick={() => (settingsTab = "rules")}>
                <strong>Rules</strong>
                <span>Operating guardrails</span>
              </button>
              <button class:active={settingsTab === "skills"} type="button" onclick={() => (settingsTab = "skills")}>
                <strong>Skills</strong>
                <span>Reusable playbooks</span>
              </button>
              <button class:active={settingsTab === "mcp"} type="button" onclick={() => (settingsTab = "mcp")}>
                <strong>MCP</strong>
                <span>Tools and server connections</span>
              </button>
              <button class:active={settingsTab === "jira"} type="button" onclick={() => (settingsTab = "jira")}>
                <strong>Jira</strong>
                <span>Board sync and credentials</span>
              </button>
              <button class:active={settingsTab === "theme"} type="button" onclick={() => (settingsTab = "theme")}>
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
                </button>
              {/each}
              <button class="add-server" type="button" onclick={addMcpServer}>＋ Add MCP</button>
            </aside>
            {/if}

            {#if selectedServer}
              <form class="settings-form" onsubmit={(event) => event.preventDefault()}>
                {#if settingsTab === "mcp"}
                <McpConnectionSettings
                  server={selectedServer}
                  jiraBaseUrl={settings.jira.baseUrl}
                  jiraPrincipal={settings.jira.username}
                  jiraAuthMode={settings.jira.authMode}
                  onUpdate={updateSelectedServer}
                  onError={(message) => (settingsError = message)}
                />
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
                          (settings = {
                            ...settings,
                            aiWorker: {
                              ...settings.aiWorker,
                              apiKeys: {
                                ...settings.aiWorker.apiKeys,
                                [selectedAiProvider.id]: event.currentTarget.value,
                              },
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
                      <label>
                        <span>OpenCode model</span>
                        <input
                          placeholder="openai/gpt-5.5"
                          value={settings.aiWorker.opencodeModel}
                          oninput={(event) => {
                            settings = {
                              ...settings,
                              aiWorker: { ...settings.aiWorker, opencodeModel: event.currentTarget.value },
                            };
                            workerStatus = null;
                          }}
                        />
                      </label>
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
                        class="agent-instruction-field"
                        rows="10"
                        spellcheck="false"
                        placeholder="Never mark a task done unless it was actually executed and verified.&#10;Do not touch secrets unless explicitly requested.&#10;Block instead of guessing when tools or access are missing."
                        value={settings.aiWorker.agentRules}
                        oninput={(event) => {
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, agentRules: event.currentTarget.value },
                          };
                        }}
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
                        class="agent-instruction-field"
                        rows="12"
                        spellcheck="false"
                        placeholder="Skill: Bamboo diagnostics&#10;Check latest build status, fetch logs, identify failing job, summarize evidence.&#10;&#10;Skill: OCP troubleshooting&#10;Check pod status, recent events, logs, and resource usage before guessing."
                        value={settings.aiWorker.agentSkills}
                        oninput={(event) => {
                          settings = {
                            ...settings,
                            aiWorker: { ...settings.aiWorker, agentSkills: event.currentTarget.value },
                          };
                        }}
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
                        value={settings.jira.apiToken}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, apiToken: event.currentTarget.value },
                          })}
                      />
                    </label>
                  {:else if settings.jira.authMode === "pat"}
                    <label>
                      <span>Personal Access Token</span>
                      <input
                        type="password"
                        placeholder="Paste PAT here"
                        value={settings.jira.personalAccessToken}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, personalAccessToken: event.currentTarget.value },
                          })}
                      />
                    </label>
                  {:else}
                    <label>
                      <span>Password</span>
                      <input
                        type="password"
                        placeholder="Jira password"
                        value={settings.jira.password}
                        oninput={(event) =>
                          (settings = {
                            ...settings,
                            jira: { ...settings.jira, password: event.currentTarget.value },
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

                {#if settingsTab === "mcp" && discoveredTools.length > 0}
                  <details class="tool-list">
                    <summary>Available MCP tools ({discoveredTools.length})</summary>
                    <div>
                      {#each discoveredTools as tool}
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
        {#if appNotice}
          <div class={`app-notice ${appNotice.tone}`} role="status">
            <span>{appNotice.message}</span>
            <button type="button" aria-label="Dismiss notification" onclick={() => (appNotice = null)}>×</button>
          </div>
        {/if}

        {#if syncError}
          <div class="sync-error" role="status">
            <strong>Jira sync failed</strong>
            <span>{syncError}</span>
          </div>
        {/if}

        {#if workspaceMode === "board"}
        <div
          class:with-console={agentConsoleOpen && agentRunLogs.length > 0}
          class="workspace-body"
          style={agentConsoleOpen && agentRunLogs.length > 0 ? `--agent-console-width: ${layoutPrefs.agentConsoleWidth}px;` : undefined}
        >
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
                  onpointerdown={(event) => beginLayoutResize(event, "laneWidth", 260, 460, "x")}
                  onpointermove={moveLayoutResize}
                  onpointerup={endLayoutResize}
                  onpointercancel={endLayoutResize}
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
                  onpointerdown={(event) => beginLayoutResize(event, "cardMinHeight", 170, 360, "y")}
                  onpointermove={moveLayoutResize}
                  onpointerup={endLayoutResize}
                  onpointercancel={endLayoutResize}
                ></span>
              </span>
            </div>
          </div>
        <div class="board" style={`--lane-width: ${layoutPrefs.laneWidth}px;`}>
          {#each activeBoard.columns as column (column.id)}
            <section
              class="lane"
              class:drop-target={draggedCardId !== null}
              data-intent={column.intent}
              role="list"
              ondragover={(event) => event.preventDefault()}
              ondrop={(event) => {
                event.preventDefault();
                if (draggedCardId) void moveCardAndSync(draggedCardId, column.id);
                draggedCardId = null;
              }}
            >
              <header class="lane-header">
                <div>
                  <span class="caret">⌄</span>
                  <h2>{columnTitle(column.name)}</h2>
                  <span class="count">{column.cards.length}</span>
                </div>
                {#if column.intent === "done"}<button type="button">•••</button>{/if}
              </header>

              <div class="lane-cards">
                {#each column.cards as card (card.id)}
                  <TaskCard
                    card={card}
                    selected={selectedCard?.id === card.id}
                    canStartAgent={canStartAgent(card)}
                    actionLabel={agentActionLabel(card)}
                    executionLabel={executionLabel(card.execution)}
                    ticketLabel={ticketLabel(card)}
                    isBlocked={isBlocked(card.execution)}
                    showActions={column.intent === "backlog" || column.intent === "ready" || isBlocked(card.execution)}
                    showDelete={column.intent === "backlog"}
                    minHeight={layoutPrefs.cardMinHeight}
                    onSelect={() => selectCard(card)}
                    onStartAgent={() => void startWorkerForCard(card.id)}
                    onDelete={() => removeCard(card.id)}
                    onDragStart={(event) => {
                      draggedCardId = card.id;
                      event.dataTransfer?.setData("text/plain", card.id);
                    }}
                    onDragEnd={() => (draggedCardId = null)}
                  />
                {:else}
                  <div class="empty-card">Drop completed work here</div>
                {/each}
              </div>

              {#if column.intent === "backlog"}
                <footer class="lane-footer">
                  <button type="button" onclick={() => (newTaskOpen = true)}>＋ New task</button>
                </footer>
              {/if}
            </section>
          {/each}
        </div>
        </div>

        {#if agentConsoleOpen && agentRunLogs.length > 0}
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
          <aside
            class="agent-console"
            aria-label="Agent run console"
            style={`--agent-log-height: ${layoutPrefs.agentLogHeight}px; --agent-output-height: ${layoutPrefs.agentOutputHeight}px; --agent-approval-height: ${layoutPrefs.agentApprovalHeight}px;`}
          >
            <header>
              <div>
                <p>Agent Console</p>
                <h3>{agentRunTitle}</h3>
              </div>
              <div class={`run-state ${agentRunStatus}`}>{agentRunStatus}</div>
              <button type="button" aria-label="Close Agent console" onclick={() => (agentConsoleOpen = false)}>×</button>
            </header>
            <div class="console-progress" aria-label="Agent run progress">
              <div class="agent-progress-head">
                <div>
                  <span>Now</span>
                  <strong>{agentActivityTitle}</strong>
                </div>
                <strong>{agentRunProgress}%</strong>
              </div>
              <progress max="100" value={agentRunProgress}></progress>
              <p>{agentActivityDetail}</p>
              <div class="agent-next-step">
                <span>Next</span>
                <strong>{agentNextStep}</strong>
              </div>
              <div class="agent-phase-row" aria-label="Agent execution phases">
                {#each agentPhases as phase (phase.key)}
                  <span class={`agent-phase ${phase.state}`}>{phase.label}</span>
                {/each}
              </div>
            </div>
            <div class="console-lines">
              {#each agentRunLogs as log (log.id)}
                <div class={`console-line ${log.tone}`}>
                  <time>{log.at}</time>
                  <code>{log.label}</code>
                  <span>{log.message}</span>
                </div>
              {/each}
            </div>
            <div class="stack-resize-handle">
              <span
                class="drag-handle vertical"
                role="separator"
                aria-orientation="vertical"
                onpointerdown={(event) => beginLayoutResize(event, "agentLogHeight", 110, 320, "y")}
                onpointermove={moveLayoutResize}
                onpointerup={endLayoutResize}
                onpointercancel={endLayoutResize}
              ></span>
            </div>
            <div class="console-output">
              <header>
                <span>Current output</span>
              </header>
              <AgentOutput output={agentRunOutput} runStatus={agentRunStatus} />
            </div>
            <div class="stack-resize-handle">
              <span
                class="drag-handle vertical"
                role="separator"
                aria-orientation="vertical"
                onpointerdown={(event) => beginLayoutResize(event, "agentOutputHeight", 140, 420, "y")}
                onpointermove={moveLayoutResize}
                onpointerup={endLayoutResize}
                onpointercancel={endLayoutResize}
              ></span>
            </div>
            <div class="operator-terminal">
              <div class="terminal-lines">
                {#each agentTerminalLines as line (line.id)}
                  <div>
                    <code>{line.prompt}$</code>
                    <span>{line.text}</span>
                  </div>
                {/each}
              </div>
              <form
                onsubmit={(event) => {
                  event.preventDefault();
                  submitAgentTerminalInput();
                }}
              >
                <code>operator$</code>
                <input
                  placeholder="Type approval, constraint, or note for this run"
                  value={agentTerminalInput}
                  oninput={(event) => (agentTerminalInput = event.currentTarget.value)}
                />
                <button type="submit">Send</button>
              </form>
            </div>
            {#if agentRunCardId}
              <footer>
                <span>{agentRunCardId}</span>
                <button type="button" onclick={() => (selectedCardId = agentRunCardId)}>Open card</button>
              </footer>
            {/if}
          </aside>
        {/if}
        </div>

        {#if newTaskOpen}
          <aside class="new-task-popover" aria-label="Create new task">
            <button class="close-detail" type="button" aria-label="Close" onclick={() => (newTaskOpen = false)}>×</button>
            <div>
              <p class="section-kicker">New Task</p>
              <h3>Create Agent work</h3>
            </div>
            <label>
              <span>Task title</span>
              <input
                placeholder="What should the Agent do?"
                value={newTaskTitle}
                oninput={(event) => (newTaskTitle = event.currentTarget.value)}
              />
            </label>
            <label>
              <span>Description</span>
              <textarea
                placeholder="Add context, expected outcome, links, constraints, or anything the Agent should know."
                value={newTaskDescription}
                oninput={(event) => (newTaskDescription = event.currentTarget.value)}
              ></textarea>
            </label>
            <footer>
              <span>This creates a local Spacesly card. It can run without Jira.</span>
              <button type="button" onclick={createLocalTask}>Create task</button>
            </footer>
          </aside>
        {/if}

        {#if selectedCard}
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
                    disabled={!canStartAgent(selectedCard)}
                    onclick={() => void startWorkerForCard(selectedCard.id)}
                  >
                    {agentActionLabel(selectedCard)}
                  </button>
                {/if}
                <button type="button" onclick={() => (selectedCardId = null)}>Close</button>
              </div>
            </footer>
          </aside>
        {/if}
        {:else}
          <div class="term-workspace" style={`--terminal-width: ${layoutPrefs.terminalWidth}px;`}>
            <section class="workspace-terminal-pane" aria-label="Local shell terminal">
              <header>
                <div>
                  <p>Local Shell</p>
                  <h2>Workspace Terminal</h2>
                </div>
                <input
                  aria-label="Terminal working directory"
                  placeholder="Initial directory (optional)"
                  value={workspaceShellWorkdir}
                  disabled={workspaceTerminalOpened}
                  oninput={(event) => {
                    workspaceShellWorkdir = event.currentTarget.value;
                    saveUiState();
                  }}
                />
              </header>
              <div class="xterm-host" bind:this={workspaceTerminalContainer}></div>
            </section>

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

            <section class="workspace-chat-pane" aria-label="Agent chat">
              <header>
                <div>
                  <p>Agent Chat</p>
                  <h2>{settings.aiWorker.runtime === "opencode" ? settings.aiWorker.opencodeModel : selectedAiModel.label}</h2>
                </div>
                <button type="button" onclick={() => (settingsOpen = true)}>Runtime</button>
              </header>
              <div class="workspace-chat-messages">
                {#each workspaceChatMessages as message (message.id)}
                  <article class={`workspace-chat-message ${message.role}`}>
                    <strong>{message.role}</strong>
                    <p>{message.text}</p>
                  </article>
                {/each}
              </div>
              <form
                class="workspace-chat-input"
                onsubmit={(event) => {
                  event.preventDefault();
                  void sendWorkspaceChat();
                }}
              >
                <textarea
                  placeholder="Ask about work, or say: create a task, move ABC-123 to ready, start agent on this card, sync Jira..."
                  value={workspaceChatInput}
                  disabled={workspaceChatRunning}
                  oninput={(event) => (workspaceChatInput = event.currentTarget.value)}
                ></textarea>
                <button type="submit" disabled={workspaceChatRunning}>{workspaceChatRunning ? "Thinking" : "Send"}</button>
              </form>
            </section>
          </div>
        {/if}
      </section>
    {:else}
      <section class="state-panel">Preparing workspace projection...</section>
    {/if}

    <footer class="statusbar">
      <span>{cacheStatusLabel}</span>
      <span>{currentDate}</span>
      <span>{currentTime}</span>
    </footer>
  </section>
</main>
