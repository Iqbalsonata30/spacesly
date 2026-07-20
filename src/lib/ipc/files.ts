import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

export interface FileSnapshot {
  content: string;
  version: string;
  root_revision: number;
  encoding: TextEncoding;
  line_ending: LineEnding;
}

export type TextEncoding = "utf8" | "utf8-bom" | "utf16le" | "utf16be";
export type LineEnding = "lf" | "crlf";

export interface FileWriteResult {
  version: string;
  root_revision: number;
}

export interface WorkspaceFileChange {
  workspace_id: string;
  kind: "created" | "modified" | "removed" | "renamed";
  paths: string[];
}

export async function listDirectory(
  workspaceId: string,
  relativePath: string = "",
): Promise<FileEntry[]> {
  return invokeWithPolicy<FileEntry[]>(
    "list_directory",
    { workspaceId, relativePath },
    IPC_POLICIES.fileRead,
  );
}

export async function readFile(workspaceId: string, relativePath: string): Promise<FileSnapshot> {
  return invokeWithPolicy<FileSnapshot>(
    "read_file",
    { workspaceId, relativePath },
    IPC_POLICIES.fileRead,
  );
}

export async function writeFile(
  workspaceId: string,
  relativePath: string,
  content: string,
  expectedVersion: string | null = null,
  expectedRootRevision: number | null = null,
  encoding: TextEncoding = "utf8",
  lineEnding: LineEnding = "lf",
): Promise<FileWriteResult> {
  return invokeWithPolicy<FileWriteResult>(
    "write_file",
    {
      workspaceId,
      relativePath,
      content,
      expectedVersion,
      expectedRootRevision,
      encoding,
      lineEnding,
    },
    IPC_POLICIES.fileWrite,
  );
}

export async function workspaceRootPath(workspaceId: string): Promise<string> {
  return invokeWithPolicy<string>("workspace_root_path", { workspaceId }, IPC_POLICIES.fileRead);
}

export async function setWorkspaceRoot(workspaceId: string, absolutePath: string): Promise<string> {
  return invokeWithPolicy<string>(
    "set_workspace_root",
    { workspaceId, absolutePath },
    IPC_POLICIES.fileWrite,
  );
}

export async function watchWorkspaceFiles(workspaceId: string): Promise<void> {
  return invokeWithPolicy<void>("watch_workspace_files", { workspaceId }, IPC_POLICIES.fileRead);
}

export async function unwatchWorkspaceFiles(workspaceId: string): Promise<boolean> {
  return invokeWithPolicy<boolean>(
    "unwatch_workspace_files",
    { workspaceId },
    IPC_POLICIES.fileRead,
  );
}

export function onWorkspaceFileChange(
  handler: (change: WorkspaceFileChange) => void,
): Promise<UnlistenFn> {
  return listen<WorkspaceFileChange>("workspace-file-change", (event) => handler(event.payload));
}
