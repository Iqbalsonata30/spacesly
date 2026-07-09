import type { WorkspaceProjection } from "$lib/ipc";

export interface CachedWorkspace {
  savedAt: number;
  workspace: WorkspaceProjection;
}

const CACHE_KEY = "spacesly.workspace.cache.v1";

export function loadCachedWorkspace(): CachedWorkspace | null {
  if (typeof localStorage === "undefined") return null;

  const raw = localStorage.getItem(CACHE_KEY);
  if (!raw) return null;

  try {
    const parsed = JSON.parse(raw) as CachedWorkspace;
    if (!parsed.workspace || !Array.isArray(parsed.workspace.projects)) return null;
    return parsed;
  } catch {
    return null;
  }
}

export function saveCachedWorkspace(workspace: WorkspaceProjection): void {
  localStorage.setItem(
    CACHE_KEY,
    JSON.stringify({
      savedAt: Date.now(),
      workspace,
    } satisfies CachedWorkspace),
  );
}
