import { aiProviders, defaultModelForProvider, providerById } from "$lib/aiModels";
import { normalizeOpencodeModel } from "$lib/opencodeModels";

export interface McpServerSettings {
  id: string;
  name: string;
  kind: "generic" | "jira" | "ocp" | "bamboo";
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface JiraSyncSettings {
  serverId: string;
  baseUrl: string;
  authMode: "api_token" | "pat" | "password";
  username: string;
  apiToken: string;
  personalAccessToken: string;
  password: string;
  toolName: string;
  boardToolName: string;
  boardIssuesToolName: string;
  jql: string;
  boardName: string;
  boardId: string;
  projectKey: string;
  boardNameFilter: string;
  pageSize: number;
  maxPages: number;
  boards: { id: string; name: string; board_type: string }[];
}

export interface AppSettings {
  mcpServers: McpServerSettings[];
  jira: JiraSyncSettings;
  aiWorker: AiWorkerSettings;
}

export interface AppSecrets {
  jira_api_token: string;
  jira_personal_access_token: string;
  jira_password: string;
  ai_api_keys: Record<string, string>;
}

export interface AiWorkerSettings {
  runtime: "api" | "opencode";
  providerId: string;
  modelId: string;
  modelIds: Record<string, string>;
  apiKeys: Record<string, string>;
  opencodeCommand: string;
  opencodeModel: string;
  opencodeWorkdir: string;
  opencodeAutoApprove: boolean;
  agentRules: string;
  agentSkills: string;
  temperature: number;
}

const SETTINGS_KEY = "spacesly.settings.v1";

export const defaultSettings: AppSettings = {
  mcpServers: [
    {
      id: "jira-default",
      name: "Jira MCP",
      kind: "jira",
      command: "",
      args: [],
      env: {},
    },
  ],
  jira: {
    serverId: "jira-default",
    baseUrl: "",
    authMode: "api_token",
    username: "",
    apiToken: "",
    personalAccessToken: "",
    password: "",
    toolName: "jira_search",
    boardToolName: "jira_get_agile_boards",
    boardIssuesToolName: "jira_get_board_issues",
    jql: "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC",
    boardName: "My Jira work",
    boardId: "",
    projectKey: "",
    boardNameFilter: "",
    pageSize: 25,
    maxPages: 1,
    boards: [],
  },
  aiWorker: {
    runtime: "api",
    providerId: "openai",
    modelId: "gpt-5.5",
    modelIds: { openai: "gpt-5.5" },
    apiKeys: {},
    opencodeCommand: "opencode",
    opencodeModel: "openai/gpt-5.5",
    opencodeWorkdir: "",
    opencodeAutoApprove: false,
    agentRules: [
      "- Follow the repository architecture: UI is projection, workflows own execution, providers/tools are replaceable infrastructure.",
      "- Humans approve; agents execute. Ask for approval when a task may affect secrets, credentials, deployments, external systems, or destructive file changes.",
      "- Do not mark work complete unless it was actually executed and verified.",
      "- If a task requires shell/file/network access that is unavailable, report BLOCKED instead of pretending completion.",
      "- Do not modify credentials, secrets, or environment files unless the user explicitly asks.",
    ].join("\n"),
    agentSkills: [
      "Skill: Production task execution",
      "Use concrete evidence. For file changes, verify with shell commands or file reads. For Jira tasks, summarize exact status/comment actions.",
      "",
      "Skill: Operational troubleshooting",
      "Prefer checking logs, events, build output, and recent changes before guessing. Report blockers with next evidence needed.",
      "",
      "Skill: Architecture compliance",
      "Keep domain independent from UI/Tauri/provider details. Treat every model provider and external service as replaceable infrastructure.",
    ].join("\n"),
    temperature: 0.2,
  },
};

export function loadSettings(): AppSettings {
  if (typeof localStorage === "undefined") return structuredClone(defaultSettings);

  const raw = localStorage.getItem(SETTINGS_KEY);
  if (!raw) return structuredClone(defaultSettings);

  try {
    const settings = settingsWithoutSecrets(normalizeSettings(JSON.parse(raw)));
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    return settings;
  } catch {
    return structuredClone(defaultSettings);
  }
}

export function loadLegacySettingsSecrets(): AppSecrets {
  if (typeof localStorage === "undefined") return emptyAppSecrets();

  const raw = localStorage.getItem(SETTINGS_KEY);
  if (!raw) return emptyAppSecrets();

  try {
    return secretsFromSettings(normalizeSettings(JSON.parse(raw)));
  } catch {
    return emptyAppSecrets();
  }
}

export function saveSettings(settings: AppSettings): void {
  localStorage.setItem(SETTINGS_KEY, JSON.stringify(settingsWithoutSecrets(settings)));
}

export function settingsWithoutSecrets(settings: AppSettings): AppSettings {
  return {
    ...settings,
    jira: {
      ...settings.jira,
      apiToken: "",
      personalAccessToken: "",
      password: "",
    },
    aiWorker: {
      ...settings.aiWorker,
      apiKeys: {},
    },
  };
}

function emptyAppSecrets(): AppSecrets {
  return {
    jira_api_token: "",
    jira_personal_access_token: "",
    jira_password: "",
    ai_api_keys: {},
  };
}

export function secretsFromSettings(settings: AppSettings): AppSecrets {
  return {
    jira_api_token: settings.jira.apiToken,
    jira_personal_access_token: settings.jira.personalAccessToken,
    jira_password: settings.jira.password,
    ai_api_keys: settings.aiWorker.apiKeys,
  };
}

export function hasAnySecret(secrets: AppSecrets): boolean {
  return Boolean(
    secrets.jira_api_token
      || secrets.jira_personal_access_token
      || secrets.jira_password
      || Object.values(secrets.ai_api_keys).some((value) => value.trim()),
  );
}

export function mergeAppSecrets(localSecrets: AppSecrets, storedSecrets: AppSecrets): AppSecrets {
  return {
    jira_api_token: localSecrets.jira_api_token || storedSecrets.jira_api_token,
    jira_personal_access_token: localSecrets.jira_personal_access_token || storedSecrets.jira_personal_access_token,
    jira_password: localSecrets.jira_password || storedSecrets.jira_password,
    ai_api_keys: { ...storedSecrets.ai_api_keys, ...localSecrets.ai_api_keys },
  };
}

export function createMcpServer(): McpServerSettings {
  const id = `mcp-${Date.now().toString(36)}`;

  return {
    id,
    name: "New MCP Server",
    kind: "generic",
    command: "",
    args: [],
    env: {},
  };
}

export function parseArgsText(value: string): string[] {
  const trimmed = value.trim();
  if (!trimmed) return [];

  if (trimmed.startsWith("[")) {
    const parsed = JSON.parse(trimmed) as unknown;
    if (!Array.isArray(parsed) || parsed.some((entry) => typeof entry !== "string")) {
      throw new Error("Args JSON must be an array of strings.");
    }
    return parsed;
  }

  return trimmed.split(/\s+/);
}

export function parseCommandText(value: string): { command: string; args: string[] } {
  const parts = splitCommand(value.trim());
  const command = parts[0] ?? "";

  return {
    command,
    args: parts.slice(1),
  };
}

export function parseEnvText(value: string): Record<string, string> {
  const trimmed = value.trim();
  if (!trimmed) return {};

  if (trimmed.startsWith("{")) {
    const parsed = JSON.parse(trimmed) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("Env JSON must be an object.");
    }

    return Object.fromEntries(
      Object.entries(parsed).map(([key, entry]) => [key, String(entry)]),
    );
  }

  return Object.fromEntries(
    trimmed.split("\n").flatMap((line) => {
      const separator = line.indexOf("=");
      if (separator === -1) return [];
      return [[line.slice(0, separator).trim(), line.slice(separator + 1).trim()]];
    }),
  );
}

function normalizeSettings(value: unknown): AppSettings {
  const fallback = structuredClone(defaultSettings);
  if (!value || typeof value !== "object") return fallback;

  const candidate = value as Partial<AppSettings>;
  const mcpServers = Array.isArray(candidate.mcpServers)
    ? candidate.mcpServers.map(normalizeServer).filter((server) => server.id)
    : fallback.mcpServers;
  const aiProviderId = normalizeProviderId(candidate.aiWorker);
  const aiModelId = normalizeModelId(candidate.aiWorker, aiProviderId);
  const aiModelIds = normalizeAiModelIds(candidate.aiWorker, aiProviderId, aiModelId);
  const aiApiKeys = normalizeAiApiKeys(candidate.aiWorker, aiProviderId);

  return {
    mcpServers: mcpServers.length > 0 ? mcpServers : fallback.mcpServers,
    aiWorker: {
      runtime: candidate.aiWorker?.runtime === "opencode" ? "opencode" : fallback.aiWorker.runtime,
      providerId: aiProviderId,
      modelId: aiModelId,
      modelIds: aiModelIds,
      apiKeys: aiApiKeys,
      opencodeCommand: stringOrFallback(candidate.aiWorker?.opencodeCommand, fallback.aiWorker.opencodeCommand),
      opencodeModel: normalizeOpencodeModel(candidate.aiWorker?.opencodeModel, fallback.aiWorker.opencodeModel),
      opencodeWorkdir: String(candidate.aiWorker?.opencodeWorkdir ?? fallback.aiWorker.opencodeWorkdir),
      opencodeAutoApprove: candidate.aiWorker?.opencodeAutoApprove === true,
      agentRules: String(candidate.aiWorker?.agentRules ?? fallback.aiWorker.agentRules),
      agentSkills: String(candidate.aiWorker?.agentSkills ?? fallback.aiWorker.agentSkills),
      temperature: boundedFloat(candidate.aiWorker?.temperature, fallback.aiWorker.temperature, 0, 2),
    },
    jira: {
      serverId: candidate.jira?.serverId ?? fallback.jira.serverId,
      baseUrl: candidate.jira?.baseUrl ?? fallback.jira.baseUrl,
      authMode: candidate.jira?.authMode ?? fallback.jira.authMode,
      username: candidate.jira?.username ?? fallback.jira.username,
      apiToken: candidate.jira?.apiToken ?? fallback.jira.apiToken,
      personalAccessToken: candidate.jira?.personalAccessToken ?? fallback.jira.personalAccessToken,
      password: candidate.jira?.password ?? fallback.jira.password,
      toolName: candidate.jira?.toolName ?? fallback.jira.toolName,
      boardToolName: candidate.jira?.boardToolName ?? fallback.jira.boardToolName,
      boardIssuesToolName: candidate.jira?.boardIssuesToolName ?? fallback.jira.boardIssuesToolName,
      jql: candidate.jira?.jql ?? fallback.jira.jql,
      boardName: candidate.jira?.boardName ?? fallback.jira.boardName,
      boardId: candidate.jira?.boardId ?? fallback.jira.boardId,
      projectKey: candidate.jira?.projectKey ?? fallback.jira.projectKey,
      boardNameFilter: candidate.jira?.boardNameFilter ?? fallback.jira.boardNameFilter,
      pageSize: boundedNumber(candidate.jira?.pageSize, fallback.jira.pageSize, 1, 100),
      maxPages: boundedNumber(candidate.jira?.maxPages, fallback.jira.maxPages, 1, 20),
      boards: Array.isArray(candidate.jira?.boards)
        ? candidate.jira.boards.map((board) => ({
            id: String(board.id ?? ""),
            name: String(board.name ?? "Jira board"),
            board_type: String(board.board_type ?? "board"),
          }))
        : fallback.jira.boards,
    },
  };
}

function normalizeAiModelIds(
  value: Partial<AiWorkerSettings> | undefined,
  providerId: string,
  modelId: string,
): Record<string, string> {
  const modelIds = value?.modelIds && typeof value.modelIds === "object"
    ? Object.fromEntries(
        Object.entries(value.modelIds).filter(([key, entry]) => {
          const provider = providerById(key);
          return provider.id === key && typeof entry === "string" && provider.models.some((model) => model.id === entry);
        }),
      )
    : {};

  modelIds[providerId] = modelId;
  return modelIds;
}

function normalizeAiApiKeys(value: Partial<AiWorkerSettings> | undefined, providerId: string): Record<string, string> {
  const legacy = value as Partial<AiWorkerSettings> & { apiKey?: string } | undefined;
  const apiKeys = value?.apiKeys && typeof value.apiKeys === "object"
    ? Object.fromEntries(
        Object.entries(value.apiKeys)
          .filter(([key, entry]) => aiProviders.some((provider) => provider.id === key) && typeof entry === "string"),
      )
    : {};

  if (!apiKeys[providerId] && legacy?.apiKey) {
    apiKeys[providerId] = legacy.apiKey;
  }

  return apiKeys;
}

function normalizeProviderId(value: Partial<AiWorkerSettings> | undefined): string {
  const legacy = value as Partial<AiWorkerSettings> & { providerName?: string; baseUrl?: string } | undefined;
  const explicit = String(value?.providerId ?? "");
  if (aiProviders.some((provider) => provider.id === explicit)) return explicit;

  const baseUrl = legacy?.baseUrl ?? "";
  const providerName = (legacy?.providerName ?? "").toLowerCase();
  const matched = aiProviders.find(
    (provider) => provider.baseUrl === baseUrl || providerName.includes(provider.label.toLowerCase()),
  );
  return matched?.id ?? defaultSettings.aiWorker.providerId;
}

function normalizeModelId(value: Partial<AiWorkerSettings> | undefined, providerId: string): string {
  const legacy = value as Partial<AiWorkerSettings> & { model?: string } | undefined;
  const provider = providerById(providerId);
  const modelId = String(value?.modelId ?? legacy?.model ?? "");
  if (provider.models.some((model) => model.id === modelId)) return modelId;
  return defaultModelForProvider(providerId);
}

function boundedNumber(value: unknown, fallback: number, min: number, max: number): number {
  const number = Number(value);
  if (!Number.isFinite(number)) return fallback;
  return Math.min(max, Math.max(min, Math.floor(number)));
}

function boundedFloat(value: unknown, fallback: number, min: number, max: number): number {
  const number = Number(value);
  if (!Number.isFinite(number)) return fallback;
  return Math.min(max, Math.max(min, number));
}

function stringOrFallback(value: unknown, fallback: string): string {
  const text = String(value ?? "").trim();
  return text || fallback;
}

function splitCommand(value: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let escaping = false;

  for (const char of value) {
    if (escaping) {
      current += char;
      escaping = false;
      continue;
    }

    if (char === "\\") {
      escaping = true;
      continue;
    }

    if (quote) {
      if (char === quote) {
        quote = null;
      } else {
        current += char;
      }
      continue;
    }

    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }

    if (/\s/.test(char)) {
      if (current) {
        parts.push(current);
        current = "";
      }
      continue;
    }

    current += char;
  }

  if (current) parts.push(current);

  return parts;
}

function normalizeServer(value: unknown): McpServerSettings {
  const server = value as Partial<McpServerSettings>;

  return {
    id: String(server.id ?? ""),
    name: String(server.name ?? "MCP Server"),
    kind: normalizeKind(server.kind),
    command: String(server.command ?? ""),
    args: Array.isArray(server.args) ? server.args.map(String) : [],
    env: server.env && typeof server.env === "object" && !Array.isArray(server.env)
      ? Object.fromEntries(Object.entries(server.env).map(([key, entry]) => [key, String(entry)]))
      : {},
  };
}

function normalizeKind(value: unknown): McpServerSettings["kind"] {
  return value === "jira" || value === "ocp" || value === "bamboo" || value === "generic"
    ? value
    : "generic";
}
