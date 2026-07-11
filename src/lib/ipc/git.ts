import { invokeWithPolicy } from "$lib/ipc/policy";

const GIT_READ_POLICY = { timeoutMs: 15_000, retries: 1, retryDelayMs: 250 };
const GIT_MUTATION_POLICY = { timeoutMs: 30_000, retries: 0 };

export interface GitWorkspaceInfo {
  is_git_repo: boolean;
  repo_root: string | null;
  current_branch: string | null;
  branches: string[];
}

export async function getWorkspaceGitInfo(): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("get_workspace_git_info", undefined, GIT_READ_POLICY);
}

export async function checkoutWorkspaceGitBranch(branch: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("checkout_workspace_git_branch", { branch }, GIT_MUTATION_POLICY);
}
