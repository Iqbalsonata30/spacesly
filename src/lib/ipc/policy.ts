import { invoke } from "@tauri-apps/api/core";

export type IpcErrorCategory =
  | "timeout"
  | "transient"
  | "auth"
  | "permission"
  | "validation"
  | "not_found"
  | "cancelled"
  | "unknown";

export type IpcPolicy = {
  timeoutMs: number;
  retries?: number;
  retryDelayMs?: number;
};

type StructuredIpcError = {
  category: IpcErrorCategory;
  message: string;
  retryable?: boolean;
};

export const IPC_POLICIES = {
  aiChat: { timeoutMs: 120_000, retries: 0 },
  aiExecution: { timeoutMs: 180_000, retries: 0 },
  aiTest: { timeoutMs: 30_000, retries: 1, retryDelayMs: 750 },
  fileRead: { timeoutMs: 15_000, retries: 0 },
  fileWrite: { timeoutMs: 20_000, retries: 0 },
  gitMutation: { timeoutMs: 30_000, retries: 0 },
  gitRead: { timeoutMs: 15_000, retries: 1, retryDelayMs: 250 },
  jiraMutation: { timeoutMs: 30_000, retries: 0 },
  jiraRead: { timeoutMs: 30_000, retries: 2, retryDelayMs: 500 },
  jiraTest: { timeoutMs: 20_000, retries: 1, retryDelayMs: 500 },
  pty: { timeoutMs: 5_000, retries: 0 },
  secret: { timeoutMs: 10_000, retries: 0 },
  shellCompletion: { timeoutMs: 3_000, retries: 0 },
  workspaceCache: { timeoutMs: 10_000, retries: 0 },
} satisfies Record<string, IpcPolicy>;

export class IpcPolicyError extends Error {
  readonly category: IpcErrorCategory;
  readonly command: string;
  readonly retryable: boolean;

  constructor(command: string, category: IpcErrorCategory, message: string, retryable = false) {
    super(message);
    this.name = "IpcPolicyError";
    this.command = command;
    this.category = category;
    this.retryable = retryable;
  }
}

export async function invokeWithPolicy<T>(
  command: string,
  args: Parameters<typeof invoke<T>>[1],
  policy: IpcPolicy,
): Promise<T> {
  const retries = policy.retries ?? 0;
  let lastError: IpcPolicyError | null = null;

  for (let attempt = 0; attempt <= retries; attempt += 1) {
    try {
      return await withTimeout(invoke<T>(command, args), policy.timeoutMs, command);
    } catch (reason) {
      const error = normalizeIpcError(command, reason);
      lastError = error;
      if (attempt >= retries || !error.retryable) throw error;
      await delay(backoffMs(policy.retryDelayMs ?? 250, attempt));
    }
  }

  throw lastError ?? new IpcPolicyError(command, "unknown", `${command} failed.`);
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, command: string): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout> | null = null;

  return new Promise<T>((resolve, reject) => {
    timeoutId = setTimeout(() => {
      reject(new IpcPolicyError(command, "timeout", `${command} timed out after ${timeoutMs}ms.`, true));
    }, timeoutMs);

    promise.then(
      (value) => {
        if (timeoutId) clearTimeout(timeoutId);
        resolve(value);
      },
      (reason) => {
        if (timeoutId) clearTimeout(timeoutId);
        reject(reason);
      },
    );
  });
}

function normalizeIpcError(command: string, reason: unknown): IpcPolicyError {
  if (reason instanceof IpcPolicyError) return reason;

  const structured = parseStructuredIpcError(reason);
  if (structured) {
    return new IpcPolicyError(
      command,
      structured.category,
      structured.message,
      structured.retryable === true,
    );
  }

  const message = reason instanceof Error ? reason.message : String(reason);
  return new IpcPolicyError(command, "unknown", message, false);
}

function parseStructuredIpcError(reason: unknown): StructuredIpcError | null {
  const value = typeof reason === "string" ? parseJsonObject(reason) : reason;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;

  const candidate = value as Partial<StructuredIpcError>;
  if (!isIpcErrorCategory(candidate.category) || typeof candidate.message !== "string") return null;

  return {
    category: candidate.category,
    message: candidate.message,
    retryable: candidate.retryable === true,
  };
}

function parseJsonObject(value: string): unknown {
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return null;
  }
}

function isIpcErrorCategory(value: unknown): value is IpcErrorCategory {
  return value === "timeout"
    || value === "transient"
    || value === "auth"
    || value === "permission"
    || value === "validation"
    || value === "not_found"
    || value === "cancelled"
    || value === "unknown";
}

function backoffMs(baseDelayMs: number, attempt: number): number {
  const jitter = Math.floor(Math.random() * Math.max(1, baseDelayMs));
  return baseDelayMs * 2 ** attempt + jitter;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
