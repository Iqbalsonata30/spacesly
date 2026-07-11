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
  args: Record<string, unknown> | undefined,
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
  const message = reason instanceof Error ? reason.message : String(reason);
  const lower = message.toLowerCase();
  const category = categorizeMessage(lower);
  return new IpcPolicyError(command, category, message, isRetryable(category, lower));
}

function categorizeMessage(message: string): IpcErrorCategory {
  if (message.includes("timed out") || message.includes("timeout")) return "timeout";
  if (message.includes("cancel") || message.includes("aborted")) return "cancelled";
  if (message.includes("unauthorized") || message.includes("forbidden") || message.includes("401")) return "auth";
  if (message.includes("permission denied") || message.includes("access denied") || message.includes("403")) return "permission";
  if (message.includes("not found") || message.includes("no such file") || message.includes("404")) return "not_found";
  if (message.includes("invalid") || message.includes("validation") || message.includes("bad request") || message.includes("400")) return "validation";
  if (message.includes("429") || message.includes("502") || message.includes("503") || message.includes("504")) return "transient";
  if (message.includes("connection") || message.includes("temporarily") || message.includes("unavailable")) return "transient";
  return "unknown";
}

function isRetryable(category: IpcErrorCategory, message: string): boolean {
  if (category === "timeout" || category === "transient") return true;
  return message.includes("rate limit") || message.includes("reset by peer");
}

function backoffMs(baseDelayMs: number, attempt: number): number {
  const jitter = Math.floor(Math.random() * Math.max(1, baseDelayMs));
  return baseDelayMs * 2 ** attempt + jitter;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
