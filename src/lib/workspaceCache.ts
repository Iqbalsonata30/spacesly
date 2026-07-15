import type { WorkspaceProjection } from "$lib/ipc";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface CachedWorkspace {
  savedAt: number;
  sizeBytes: number;
  workspace: WorkspaceProjection;
  deletedCardIds?: string[];
}

const LEGACY_CACHE_KEY = "spacesly.workspace.cache.v1";
const CACHE_WRITE_DELAY_MS = 250;
const LEGACY_SEED_CARD_ID = "local-list-current-directory";

let pendingCache: WorkspaceProjection | null = null;
let cacheWriteTimer: ReturnType<typeof setTimeout> | null = null;
let cacheWriteIdleId: number | null = null;
let lastCacheSizeBytes = 0;
let deletedCardIds = new Set<string>();

export async function loadCachedWorkspace(): Promise<CachedWorkspace | null> {
  const cached = normalizeCachedWorkspace(
    await invokeWithPolicy<CachedWorkspace | null>(
      "load_cached_workspace",
      undefined,
      IPC_POLICIES.workspaceCache,
    ),
  );
  if (cached) return rememberCacheSize(cached);

  const legacy = loadLegacyCachedWorkspace();
  if (!legacy) return null;

  saveCachedWorkspace(legacy.workspace);
  removeLegacyCachedWorkspace();
  return rememberCacheSize(legacy);
}

export function saveCachedWorkspace(workspace: WorkspaceProjection): void {
  pendingCache = workspace;

  if (cacheWriteTimer) clearTimeout(cacheWriteTimer);
  if (cacheWriteIdleId !== null && "cancelIdleCallback" in window) {
    window.cancelIdleCallback(cacheWriteIdleId);
    cacheWriteIdleId = null;
  }

  cacheWriteTimer = setTimeout(() => {
    cacheWriteTimer = null;
    if ("requestIdleCallback" in window) {
      cacheWriteIdleId = window.requestIdleCallback(
        () => {
          cacheWriteIdleId = null;
          void flushCachedWorkspace();
        },
        { timeout: 1_000 },
      );
      return;
    }

    void flushCachedWorkspace();
  }, CACHE_WRITE_DELAY_MS);
}

export async function flushCachedWorkspace(): Promise<void> {
  const workspace = pendingCache;
  if (!workspace) return;
  pendingCache = null;

  const cached = await invokeWithPolicy<CachedWorkspace>(
    "save_cached_workspace",
    { workspace: filterDeletedCards(workspace), deletedCardIds: [...deletedCardIds] },
    IPC_POLICIES.workspaceCache,
  );
  rememberCacheSize(cached);
}

export function locallyDeleteCachedCard(cardId: string): void {
  if (!cardId) return;
  deletedCardIds.add(cardId);
}

export function locallyDeletedCachedCardIds(): string[] {
  return [...deletedCardIds];
}

export function cachedWorkspaceSizeBytes(): number {
  return lastCacheSizeBytes;
}

function normalizeCachedWorkspace(value: CachedWorkspace | null): CachedWorkspace | null {
  if (!value?.workspace || !Array.isArray(value.workspace.projects)) return null;
  deletedCardIds = new Set(
    (value.deletedCardIds ?? []).filter((id) => typeof id === "string" && id.length > 0),
  );
  return {
    ...value,
    deletedCardIds: [...deletedCardIds],
    workspace: filterDeletedCards(normalizeWorkspace(value.workspace)),
  };
}

function loadLegacyCachedWorkspace(): CachedWorkspace | null {
  if (typeof localStorage === "undefined") return null;

  const raw = localStorage.getItem(LEGACY_CACHE_KEY);
  if (!raw) return null;

  try {
    const parsed = JSON.parse(raw) as Partial<CachedWorkspace>;
    if (!parsed.workspace || !Array.isArray(parsed.workspace.projects)) return null;
    return {
      savedAt: Number(parsed.savedAt) || Date.now(),
      sizeBytes: raw.length,
      workspace: normalizeWorkspace(parsed.workspace),
    };
  } catch {
    return null;
  }
}

function removeLegacyCachedWorkspace() {
  if (typeof localStorage === "undefined") return;
  localStorage.removeItem(LEGACY_CACHE_KEY);
}

function normalizeWorkspace(workspace: WorkspaceProjection): WorkspaceProjection {
  return {
    ...workspace,
    projects: workspace.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) => {
          const migratedColumn =
            String(column.intent) === "ready"
              ? {
                  ...column,
                  id: column.id === "column-ready" ? "column-queued" : column.id,
                  name: "Queued",
                  intent: "queued" as const,
                }
              : column;

          return {
            ...migratedColumn,
            cards: migratedColumn.cards.filter((card) => card.id !== LEGACY_SEED_CARD_ID),
          };
        }),
      })),
    })),
  };
}

function rememberCacheSize(cached: CachedWorkspace): CachedWorkspace {
  lastCacheSizeBytes = cached.sizeBytes;
  if (cached.deletedCardIds) deletedCardIds = new Set(cached.deletedCardIds);
  return cached;
}

function filterDeletedCards(workspace: WorkspaceProjection): WorkspaceProjection {
  if (deletedCardIds.size === 0) return workspace;
  return {
    ...workspace,
    projects: workspace.projects.map((project) => ({
      ...project,
      boards: project.boards.map((board) => ({
        ...board,
        columns: board.columns.map((column) => ({
          ...column,
          cards: column.cards.filter((card) => !deletedCardIds.has(card.id)),
        })),
      })),
    })),
  };
}

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => {
    void flushCachedWorkspace();
  });
}
