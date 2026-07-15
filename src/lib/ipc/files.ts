import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
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

export async function readFile(workspaceId: string, relativePath: string): Promise<string> {
  return invokeWithPolicy<string>(
    "read_file",
    { workspaceId, relativePath },
    IPC_POLICIES.fileRead,
  );
}

export async function writeFile(
  workspaceId: string,
  relativePath: string,
  content: string,
): Promise<void> {
  return invokeWithPolicy<void>(
    "write_file",
    { workspaceId, relativePath, content },
    IPC_POLICIES.fileWrite,
  );
}

export async function workspaceRootPath(workspaceId: string): Promise<string> {
  return invokeWithPolicy<string>("workspace_root_path", { workspaceId }, IPC_POLICIES.fileRead);
}

export async function setWorkspaceRoot(absolutePath: string): Promise<string> {
  return invokeWithPolicy<string>("set_workspace_root", { absolutePath }, IPC_POLICIES.fileWrite);
}
