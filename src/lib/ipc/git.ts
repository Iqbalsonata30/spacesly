import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface GitWorkspaceInfo {
  is_git_repo: boolean;
  repo_root: string | null;
  current_branch: string | null;
  branches: string[];
  head_commit: string | null;
  upstream_branch: string | null;
  dirty_worktree: boolean;
  ahead_count: number;
  behind_count: number;
}

export interface GitChangedFile {
  path: string;
  status: string;
  original_path: string | null;
}

export interface GitStatus {
  staged: GitChangedFile[];
  unstaged: GitChangedFile[];
}

export interface CommitResult {
  hash: string;
  message: string;
}

export async function getWorkspaceGitInfo(): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("get_workspace_git_info", undefined, IPC_POLICIES.gitRead);
}

export async function getPathGitInfo(path: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("get_path_git_info", { path }, IPC_POLICIES.gitRead);
}

export async function getWorkspaceChangedFiles(): Promise<GitChangedFile[]> {
  return invokeWithPolicy<GitChangedFile[]>("get_workspace_changed_files", undefined, IPC_POLICIES.gitRead);
}

export async function getWorkspaceGitStatus(): Promise<GitStatus> {
  return invokeWithPolicy<GitStatus>("get_workspace_git_status", undefined, IPC_POLICIES.gitRead);
}

export async function stageWorkspaceGitFile(path: string): Promise<GitStatus> {
  return invokeWithPolicy<GitStatus>("stage_workspace_git_file", { path }, IPC_POLICIES.gitMutation);
}

export async function stageAllWorkspaceGitFiles(): Promise<GitStatus> {
  return invokeWithPolicy<GitStatus>("stage_all_workspace_git_files", undefined, IPC_POLICIES.gitMutation);
}

export async function unstageWorkspaceGitFile(path: string): Promise<GitStatus> {
  return invokeWithPolicy<GitStatus>("unstage_workspace_git_file", { path }, IPC_POLICIES.gitMutation);
}

export async function unstageAllWorkspaceGitFiles(): Promise<GitStatus> {
  return invokeWithPolicy<GitStatus>("unstage_all_workspace_git_files", undefined, IPC_POLICIES.gitMutation);
}

export async function checkoutWorkspaceGitBranch(branch: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("checkout_workspace_git_branch", { branch }, IPC_POLICIES.gitMutation);
}

export async function gitCommit(message: string): Promise<CommitResult> {
  return invokeWithPolicy<CommitResult>("commit_workspace_git_changes", { message }, IPC_POLICIES.gitMutation);
}

export async function gitPush(): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("push_workspace_git_changes", undefined, IPC_POLICIES.gitMutation);
}

export async function gitPull(): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("pull_workspace_git_changes", undefined, IPC_POLICIES.gitMutation);
}

export async function gitMergeBranch(branch: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("merge_workspace_git_branch", { branch }, IPC_POLICIES.gitMutation);
}

export async function gitRebaseBranch(branch: string): Promise<GitWorkspaceInfo> {
  return invokeWithPolicy<GitWorkspaceInfo>("rebase_workspace_git_branch", { branch }, IPC_POLICIES.gitMutation);
}
