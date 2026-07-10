import type { WorkspaceProjection } from "$lib/ipc";

export interface CachedWorkspace {
  savedAt: number;
  workspace: WorkspaceProjection;
}

const CACHE_KEY = "spacesly.workspace.cache.v1";
const CACHE_WRITE_DELAY_MS = 250;

let pendingCache: CachedWorkspace | null = null;
let cacheWriteTimer: ReturnType<typeof setTimeout> | null = null;
let cacheWriteIdleId: number | null = null;

export function loadCachedWorkspace(): CachedWorkspace | null {
  if (typeof localStorage === "undefined") return null;

  const raw = localStorage.getItem(CACHE_KEY);
  if (!raw) return null;

  try {
    const parsed = JSON.parse(raw) as CachedWorkspace;
    if (!parsed.workspace || !Array.isArray(parsed.workspace.projects)) return null;
    parsed.workspace.projects = parsed.workspace.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) =>
          String(column.intent) === "ready"
            ? { ...column, id: column.id === "column-ready" ? "column-queued" : column.id, name: "Queued", intent: "queued" as const }
            : column,
        ),
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

  localStorage.setItem(CACHE_KEY, JSON.stringify(pendingCache));
  pendingCache = null;
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", flushCachedWorkspace);
}
