import { invokeWithPolicy } from "$lib/ipc/policy";

const FILE_READ_POLICY = { timeoutMs: 15_000, retries: 0 };
const FILE_WRITE_POLICY = { timeoutMs: 20_000, retries: 0 };

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
  return invokeWithPolicy<FileEntry[]>("list_directory", { workspaceId, relativePath }, FILE_READ_POLICY);
}

export async function readFile(workspaceId: string, relativePath: string): Promise<string> {
  return invokeWithPolicy<string>("read_file", { workspaceId, relativePath }, FILE_READ_POLICY);
}

export async function writeFile(
  workspaceId: string,
  relativePath: string,
  content: string,
): Promise<void> {
  return invokeWithPolicy<void>("write_file", { workspaceId, relativePath, content }, FILE_WRITE_POLICY);
}

export async function workspaceRootPath(workspaceId: string): Promise<string> {
  return invokeWithPolicy<string>("workspace_root_path", { workspaceId }, FILE_READ_POLICY);
}

export async function setWorkspaceRoot(absolutePath: string): Promise<string> {
  return invokeWithPolicy<string>("set_workspace_root", { absolutePath }, FILE_WRITE_POLICY);
}
