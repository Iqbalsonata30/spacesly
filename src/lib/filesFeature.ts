import type { UiState } from "$lib/uiState";

export function normalizeAbsolutePath(path: string): string {
  return path.replace(/\\/g, "/").replace(/\/+$/, "");
}

export function displayPath(path: string): string {
  return normalizeAbsolutePath(path) || "~";
}

export function parentDirectory(path: string): string | null {
  if (!path) return null;
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.join("/");
}

export function fileName(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

export function directoryBreadcrumbs(path: string): Array<{ label: string; path: string }> {
  const crumbs = [{ label: "workspace", path: "" }];
  const parts = path.split("/").filter(Boolean);
  let current = "";

  for (const part of parts) {
    current = current ? `${current}/${part}` : part;
    crumbs.push({ label: part, path: current });
  }

  return crumbs;
}

export function filesRestoreTarget(state: UiState): {
  root: string;
  directory: string;
  activePath: string | null;
} {
  const root = normalizeAbsolutePath(state.workspaceFilesRoot);
  const directory = state.workspaceFilesDirectory || "";
  const activePath = state.workspaceFilesActivePath;
  return {
    root,
    directory: directory || (activePath ? (parentDirectory(activePath) ?? "") : ""),
    activePath,
  };
}
