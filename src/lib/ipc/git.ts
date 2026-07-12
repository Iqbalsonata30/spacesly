import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface GitWorkspaceInfo {
  is_git_repo: boolean;
  repo_root: string | null;
  current_branch: string | null;
  branches: string[];
}

export async function getWorkspaceGitInfo(): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("get_workspace_git_info", undefined, IPC_POLICIES.gitRead);
}

export async function checkoutWorkspaceGitBranch(branch: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("checkout_workspace_git_branch", { branch }, IPC_POLICIES.gitMutation);
}
