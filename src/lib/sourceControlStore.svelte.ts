import {
  checkoutWorkspaceGitBranch,
  gitCommit,
  gitMergeBranch,
  gitPull,
  gitPush,
  gitRebaseBranch,
  getWorkspaceGitInfo,
  getWorkspaceGitStatus,
  stageAllWorkspaceGitFiles,
  stageWorkspaceGitFile,
  unstageAllWorkspaceGitFiles,
  unstageWorkspaceGitFile,
  type CommitResult,
  type GitStatus,
  type GitWorkspaceInfo,
} from "$lib/ipc/git";

export type SourceControlOperation =
  | "refresh"
  | "checkout"
  | "pull"
  | "commit"
  | "push"
  | "merge"
  | "rebase"
  | "stage"
  | "stage-all"
  | "unstage"
  | "unstage-all";

type RefreshKind = "state" | "info" | "status";

const emptyStatus: GitStatus = { staged: [], unstaged: [] };

export function createSourceControlStore(
  options: {
    onRepositoryChanged?: (refreshFiles: boolean, refreshEditors: boolean) => Promise<void> | void;
    onNotice?: (tone: "info" | "success" | "error", message: string) => void;
  } = {},
) {
  let info = $state<GitWorkspaceInfo | null>(null);
  let status = $state<GitStatus>(emptyStatus);
  let loading = $state(false);
  let operation = $state<SourceControlOperation | null>(null);
  let error = $state<string | null>(null);
  let requestId = 0;

  async function notifyRepositoryChanged(refreshFiles: boolean, refreshEditors: boolean) {
    await options.onRepositoryChanged?.(refreshFiles, refreshEditors);
  }

  function clearState() {
    info = null;
    status = emptyStatus;
  }

  async function refresh(kind: RefreshKind = "state") {
    const currentRequestId = ++requestId;
    loading = true;
    error = null;

    try {
      if (kind === "info") {
        const nextInfo = await getWorkspaceGitInfo();
        if (currentRequestId === requestId) info = nextInfo.is_git_repo ? nextInfo : null;
        return;
      }

      if (kind === "status") {
        const nextStatus = await getWorkspaceGitStatus();
        if (currentRequestId === requestId) status = nextStatus;
        return;
      }

      const nextInfo = await getWorkspaceGitInfo();
      if (currentRequestId !== requestId) return;

      if (!nextInfo.is_git_repo) {
        info = null;
        status = emptyStatus;
        return;
      }

      info = nextInfo;
      status = await getWorkspaceGitStatus();
      if (currentRequestId !== requestId) return;
    } catch (reason) {
      if (currentRequestId !== requestId) return;
      clearState();
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (currentRequestId === requestId) loading = false;
    }
  }

  async function run<T>(
    nextOperation: SourceControlOperation,
    action: () => Promise<T>,
    refreshKind: RefreshKind | null = "state",
    refreshFiles = false,
    refreshEditors = false,
  ): Promise<T | null> {
    if (operation) return null;
    operation = nextOperation;
    error = null;

    try {
      const result = await action();
      if (refreshKind) await refresh(refreshKind);
      await notifyRepositoryChanged(refreshFiles, refreshEditors);
      return result;
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      error = message;
      options.onNotice?.("error", message);
      return null;
    } finally {
      operation = null;
    }
  }

  async function stageFile(path: string) {
    await run(
      "stage",
      async () => {
        status = await stageWorkspaceGitFile(path);
        return null;
      },
      null,
    );
  }

  async function stageAll() {
    await run(
      "stage-all",
      async () => {
        status = await stageAllWorkspaceGitFiles();
        return null;
      },
      null,
    );
  }

  async function unstageFile(path: string) {
    await run(
      "unstage",
      async () => {
        status = await unstageWorkspaceGitFile(path);
        return null;
      },
      null,
    );
  }

  async function unstageAll() {
    await run(
      "unstage-all",
      async () => {
        status = await unstageAllWorkspaceGitFiles();
        return null;
      },
      null,
    );
  }

  async function checkoutBranch(branch: string) {
    const nextInfo = await run(
      "checkout",
      async () => checkoutWorkspaceGitBranch(branch),
      "state",
      true,
      true,
    );
    if (nextInfo) info = nextInfo;
  }

  async function pull() {
    const nextInfo = await run("pull", async () => gitPull(), "state", true, true);
    if (nextInfo) info = nextInfo;
  }

  async function push() {
    const nextInfo = await run("push", async () => gitPush(), "state", false, false);
    if (nextInfo) info = nextInfo;
  }

  async function merge(branch: string) {
    const nextInfo = await run("merge", async () => gitMergeBranch(branch), "state", true, true);
    if (nextInfo) info = nextInfo;
  }

  async function rebase(branch: string) {
    const nextInfo = await run("rebase", async () => gitRebaseBranch(branch), "state", true, true);
    if (nextInfo) info = nextInfo;
  }

  async function commit(message: string): Promise<CommitResult | null> {
    const result = await run("commit", async () => gitCommit(message), "state", false, false);
    if (result)
      options.onNotice?.("success", `Committed ${(result as CommitResult).hash.slice(0, 7)}`);
    return result as CommitResult | null;
  }

  return {
    get info() {
      return info;
    },
    get status() {
      return status;
    },
    get staged() {
      return status.staged;
    },
    get unstaged() {
      return status.unstaged;
    },
    get loading() {
      return loading;
    },
    get operation() {
      return operation;
    },
    get error() {
      return error;
    },
    setError(message: string | null) {
      error = message;
    },
    refresh,
    stageFile,
    stageAll,
    unstageFile,
    unstageAll,
    checkoutBranch,
    pull,
    push,
    merge,
    rebase,
    commit,
  };
}
