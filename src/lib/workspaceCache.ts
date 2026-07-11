import type { WorkspaceProjection } from "$lib/ipc";

export interface CachedWorkspace {
  savedAt: number;
  workspace: WorkspaceProjection;
}

const CACHE_KEY = "spacesly.workspace.cache.v1";
const CACHE_WRITE_DELAY_MS = 250;
const LEGACY_SEED_CARD_ID = "local-list-current-directory";

let pendingCache: CachedWorkspace | null = null;
let cacheWriteTimer: ReturnType<typeof setTimeout> | null = null;
let cacheWriteIdleId: number | null = null;
let lastCacheSizeBytes = 0;
let lastCachePayload = "";

export function loadCachedWorkspace(): CachedWorkspace | null {
  if (typeof localStorage === "undefined") return null;

  const raw = localStorage.getItem(CACHE_KEY);
  if (!raw) return null;
  lastCacheSizeBytes = raw.length;
  lastCachePayload = raw;

  try {
    const parsed = JSON.parse(raw) as CachedWorkspace;
    if (!parsed.workspace || !Array.isArray(parsed.workspace.projects)) return null;
    parsed.workspace.projects = parsed.workspace.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) => {
          const migratedColumn = String(column.intent) === "ready"
            ? { ...column, id: column.id === "column-ready" ? "column-queued" : column.id, name: "Queued", intent: "queued" as const }
            : column;

          return {
            ...migratedColumn,
            cards: migratedColumn.cards.filter((card) => card.id !== LEGACY_SEED_CARD_ID),
          };
        }),
      })),
    }));
    return parsed;
  } catch {
    return null;
  }
}

export function saveCachedWorkspace(workspace: WorkspaceProjection): void {
  if (typeof localStorage === "undefined") return;

  pendingCache = {
    savedAt: Date.now(),
    workspace,
  };

  if (cacheWriteTimer) clearTimeout(cacheWriteTimer);
  if (cacheWriteIdleId !== null && "cancelIdleCallback" in window) {
    window.cancelIdleCallback(cacheWriteIdleId);
    cacheWriteIdleId = null;
  }

  cacheWriteTimer = setTimeout(() => {
    cacheWriteTimer = null;
    if ("requestIdleCallback" in window) {
      cacheWriteIdleId = window.requestIdleCallback(() => {
        cacheWriteIdleId = null;
        flushCachedWorkspace();
      }, { timeout: 1_000 });
      return;
    }

    flushCachedWorkspace();
  }, CACHE_WRITE_DELAY_MS);
}

export function flushCachedWorkspace(): void {
  if (typeof localStorage === "undefined" || !pendingCache) return;

  const payload = JSON.stringify(pendingCache);
  if (payload !== lastCachePayload) {
    localStorage.setItem(CACHE_KEY, payload);
    lastCachePayload = payload;
  }
  lastCacheSizeBytes = payload.length;
  pendingCache = null;
}

export function cachedWorkspaceSizeBytes(): number {
  if (typeof localStorage === "undefined") return lastCacheSizeBytes;
  return localStorage.getItem(CACHE_KEY)?.length ?? lastCacheSizeBytes;
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", flushCachedWorkspace);
}
