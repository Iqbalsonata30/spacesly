<script lang="ts">
  import { ChevronDown, Loader2, Minus, Plus } from "lucide-svelte";
  import GitBranchPicker from "$lib/components/GitBranchPicker.svelte";
  import WorkspaceRow from "$lib/components/WorkspaceRow.svelte";
  import type { GitChangedFile, GitWorkspaceInfo } from "$lib/ipc/git";

  type Props = {
    workspaceGitInfo: GitWorkspaceInfo | null;
    workspaceGitLoading: boolean;
    workspaceGitError: string | null;
    switchingWorkspaceBranch: boolean;
    hasDirtyEditors: boolean;
    stagedFiles: GitChangedFile[];
    unstagedFiles: GitChangedFile[];
    onStageFile: (path: string) => Promise<void>;
    onStageAll: () => Promise<void>;
    onUnstageFile: (path: string) => Promise<void>;
    onUnstageAll: () => Promise<void>;
    onSwitchBranch: (branch: string) => void;
    onPull: () => Promise<void>;
    onCommit: (message: string) => Promise<boolean>;
    onPush: () => Promise<void>;
    onMerge: (branch: string) => Promise<void>;
    onRebase: (branch: string) => Promise<void>;
    onRefresh: () => Promise<void>;
    onOpenFile: (path: string) => void;
  };

  type GitActionName =
    | "pull"
    | "commit"
    | "push"
    | "merge"
    | "rebase"
    | "refresh"
    | "stage"
    | "stage-all"
    | "unstage"
    | "unstage-all";
  type ContextMenuState = {
    file: GitChangedFile;
    kind: "staged" | "unstaged";
    x: number;
    y: number;
  } | null;

  let {
    workspaceGitInfo,
    workspaceGitLoading,
    workspaceGitError,
    switchingWorkspaceBranch,
    hasDirtyEditors,
    stagedFiles,
    unstagedFiles,
    onStageFile,
    onStageAll,
    onUnstageFile,
    onUnstageAll,
    onSwitchBranch,
    onPull,
    onCommit,
    onPush,
    onMerge,
    onRebase,
    onRefresh,
    onOpenFile,
  }: Props = $props();

  let commitMessage = $state("");
  let commitTouched = $state(false);
  let mergeBranch = $state("");
  let actionsMenuOpen = $state(false);
  let actionBusy = $state<GitActionName | null>(null);
  let contextMenu = $state<ContextMenuState>(null);

  let branchChoices = $derived(
    (workspaceGitInfo?.branches ?? []).filter(
      (branch) => branch !== workspaceGitInfo?.current_branch,
    ),
  );

  $effect(() => {
    if (!branchChoices.includes(mergeBranch)) {
      mergeBranch = branchChoices[0] ?? "";
    }
  });

  let hasRepo = $derived(Boolean(workspaceGitInfo?.is_git_repo));
  let stagedCount = $derived(stagedFiles.length);
  let unstagedCount = $derived(unstagedFiles.length);
  let changedCount = $derived(stagedCount + unstagedCount);
  let repoClean = $derived(hasRepo && changedCount === 0 && !hasDirtyEditors);
  let worktreeDirty = $derived(Boolean(workspaceGitInfo?.dirty_worktree || hasDirtyEditors));
  let currentBranchLabel = $derived(workspaceGitInfo?.current_branch ?? "detached HEAD");
  let commitCharCount = $derived(commitMessage.length);
  let commitMessageEmpty = $derived(commitMessage.trim().length === 0);
  let commitValidation = $derived(
    stagedCount === 0
      ? "Stage at least one file before committing."
      : commitMessageEmpty
        ? "Enter a commit message."
        : null,
  );

  let headerMeta = $derived.by(() => {
    if (workspaceGitLoading && !workspaceGitInfo) return ["Loading repository"];
    const chips: string[] = [];
    if (hasRepo) chips.push(currentBranchLabel);
    if (workspaceGitInfo?.head_commit)
      chips.push(`HEAD ${workspaceGitInfo.head_commit.slice(0, 7)}`);
    if (changedCount > 0) chips.push(`${changedCount} change${changedCount === 1 ? "" : "s"}`);
    if (workspaceGitInfo?.ahead_count) chips.push(`${workspaceGitInfo.ahead_count} ahead`);
    if (workspaceGitInfo?.behind_count) chips.push(`${workspaceGitInfo.behind_count} behind`);
    if (hasDirtyEditors) chips.push("Unsaved editors");
    if (!hasRepo) chips.push("Not a git repository");
    if (repoClean) chips.push("Working tree clean");
    return chips;
  });

  const busyLabels: Record<GitActionName, string> = {
    pull: "Pulling...",
    commit: "Committing...",
    push: "Pushing...",
    merge: "Merging...",
    rebase: "Rebasing...",
    refresh: "Refreshing...",
    stage: "Staging...",
    "stage-all": "Staging...",
    unstage: "Unstaging...",
    "unstage-all": "Unstaging...",
  };

  function actionLabel(base: GitActionName) {
    return actionBusy === base
      ? busyLabels[base]
      : base[0].toUpperCase() + base.slice(1).replace("-", " ");
  }

  async function runAction(name: GitActionName, handler: () => Promise<void>) {
    if (actionBusy) return;
    actionBusy = name;
    contextMenu = null;
    try {
      await handler();
    } finally {
      actionBusy = null;
    }
  }

  function statusTone(status: string): "neutral" | "modified" | "added" | "deleted" {
    if (status === "M" || status === "R") return "modified";
    if (status === "A" || status === "U") return "added";
    if (status === "D") return "deleted";
    return "neutral";
  }

  function statusLabel(status: string) {
    return (
      {
        M: "Modified",
        A: "Added",
        D: "Deleted",
        R: "Renamed",
        U: "Untracked",
      }[status] ?? status
    );
  }

  function fileLabel(file: GitChangedFile) {
    return file.original_path ? `${file.original_path} -> ${file.path}` : file.path;
  }

  function openContextMenu(kind: "staged" | "unstaged", file: GitChangedFile, event: MouseEvent) {
    event.preventDefault();
    const menuWidth = 180;
    const menuHeight = 92;
    contextMenu = {
      kind,
      file,
      x: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth - 8)),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight - 8)),
    };
  }

  async function commit() {
    commitTouched = true;
    if (commitValidation) return;
    await runAction("commit", async () => {
      if (await onCommit(commitMessage.trim())) {
        commitMessage = "";
        commitTouched = false;
      }
    });
  }

  async function runMergeOrRebase(kind: "merge" | "rebase") {
    if (!mergeBranch) return;
    actionsMenuOpen = false;
    if (kind === "merge") {
      await runAction("merge", () => onMerge(mergeBranch));
    } else {
      await runAction("rebase", () => onRebase(mergeBranch));
    }
  }

  const commitDisabled = $derived(
    !hasRepo || stagedCount === 0 || commitMessageEmpty || actionBusy !== null,
  );
  const syncDisabled = $derived(
    !hasRepo ||
      worktreeDirty ||
      workspaceGitLoading ||
      switchingWorkspaceBranch ||
      actionBusy !== null,
  );
  const mergeDisabled = $derived(
    !hasRepo || worktreeDirty || !mergeBranch || switchingWorkspaceBranch || actionBusy !== null,
  );
  const rebaseDisabled = $derived(
    !hasRepo || worktreeDirty || !mergeBranch || switchingWorkspaceBranch || actionBusy !== null,
  );
  const pushDisabled = $derived(!hasRepo || switchingWorkspaceBranch || actionBusy !== null);
  const refreshDisabled = $derived(!hasRepo || workspaceGitLoading || actionBusy !== null);
  const stageAllDisabled = $derived(!hasRepo || unstagedCount === 0 || actionBusy !== null);
  const unstageAllDisabled = $derived(!hasRepo || stagedCount === 0 || actionBusy !== null);
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape") {
      contextMenu = null;
      actionsMenuOpen = false;
    }
  }}
  onclick={(event) => {
    if (!(event.target as HTMLElement).closest(".context-menu, .branch-menu-wrap")) {
      contextMenu = null;
      actionsMenuOpen = false;
    }
  }}
/>

<aside class="git-actions-pane" aria-label="Source control">
  <header class="source-header">
    <div class="source-header-copy">
      <p>Source control</p>
      <h2>
        {workspaceGitLoading && !workspaceGitInfo
          ? "Loading repository"
          : !hasRepo
            ? "No Git repository"
            : repoClean
              ? "Working tree clean"
              : "Review and commit changes"}
      </h2>
      <div class="source-header-meta">
        {#each headerMeta as meta, i (i)}
          <span title={meta}>{meta}</span>
        {/each}
      </div>
    </div>

    <div class="source-header-controls">
      <GitBranchPicker
        gitInfo={workspaceGitInfo}
        loading={workspaceGitLoading}
        switching={switchingWorkspaceBranch}
        dirty={worktreeDirty}
        onSwitch={onSwitchBranch}
      />

      <div class="branch-menu-wrap">
        <button
          type="button"
          class="branch-menu-button"
          disabled={!hasRepo || switchingWorkspaceBranch || actionBusy !== null}
          aria-haspopup="dialog"
          aria-expanded={actionsMenuOpen}
          onclick={(event) => {
            event.stopPropagation();
            actionsMenuOpen = !actionsMenuOpen;
          }}
        >
          <span class="branch-menu-button-copy">
            <small>Target branch</small>
            <strong title={mergeBranch || branchChoices[0] || "Choose branch"}
              >{mergeBranch || branchChoices[0] || "Choose branch"}</strong
            >
          </span>
          <span class="branch-menu-button-indicator"
            ><ChevronDown size={14} aria-hidden="true" /></span
          >
        </button>
        {#if actionsMenuOpen}
          <div
            class="branch-menu"
            role="dialog"
            aria-label="Merge or rebase target branch"
            aria-modal="false"
            tabindex="-1"
          >
            <div class="branch-menu-copy">
              <p>Merge / rebase</p>
              <strong>Choose a target branch</strong>
              <span>Pick a branch, then choose how to combine history.</span>
            </div>
            <label>
              <span>Branch</span>
              <select
                bind:value={mergeBranch}
                disabled={branchChoices.length === 0 ||
                  switchingWorkspaceBranch ||
                  actionBusy !== null}
              >
                {#if branchChoices.length === 0}
                  <option value="">No other branches</option>
                {:else}
                  {#each branchChoices as branch (branch)}
                    <option value={branch}>{branch}</option>
                  {/each}
                {/if}
              </select>
            </label>
            <div class="branch-menu-actions">
              <button
                type="button"
                disabled={mergeDisabled}
                onclick={() => void runMergeOrRebase("merge")}>Merge</button
              >
              <button
                type="button"
                disabled={rebaseDisabled}
                onclick={() => void runMergeOrRebase("rebase")}>Rebase</button
              >
            </div>
          </div>
        {/if}
      </div>
    </div>
  </header>

  {#if workspaceGitError}
    <div class="git-error" role="status">{workspaceGitError}</div>
  {/if}

  <div class="source-body">
    <section class="source-card toolbar-card" aria-label="Git actions">
      <div class="card-label-row">
        <div>
          <span>Quick actions</span>
          <h3>Sync and staging</h3>
        </div>
        <small>{actionBusy ? busyLabels[actionBusy] : repoClean ? "Clean" : "Ready"}</small>
      </div>
      <div class="source-toolbar">
        <button
          type="button"
          disabled={stageAllDisabled}
          onclick={() => void runAction("stage-all", onStageAll)}>Stage All</button
        >
        <button
          type="button"
          disabled={unstageAllDisabled}
          onclick={() => void runAction("unstage-all", onUnstageAll)}>Unstage All</button
        >
        <button type="button" disabled={syncDisabled} onclick={() => void runAction("pull", onPull)}
          >{actionLabel("pull")}</button
        >
        <button type="button" disabled={pushDisabled} onclick={() => void runAction("push", onPush)}
          >{actionLabel("push")}</button
        >
        <button
          type="button"
          disabled={refreshDisabled}
          onclick={() => void runAction("refresh", onRefresh)}>{actionLabel("refresh")}</button
        >
      </div>
    </section>

    <section class="source-card commit-card" aria-label="Commit message">
      <div class="card-label-row">
        <div>
          <span>Commit</span>
          <h3>
            {stagedCount > 0
              ? `${stagedCount} staged file${stagedCount === 1 ? "" : "s"}`
              : "Nothing staged"}
          </h3>
        </div>
        <small>{commitCharCount} chars</small>
      </div>

      <label class="commit-box">
        <textarea
          bind:value={commitMessage}
          placeholder={stagedCount > 0
            ? "Message (Ctrl+Enter to commit)"
            : "Stage changes to enable commit"}
          rows="5"
          disabled={!hasRepo || actionBusy !== null}
          oninput={() => (commitTouched = true)}
          onkeydown={(event) => {
            if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
              event.preventDefault();
              void commit();
            }
          }}></textarea>
      </label>

      {#if commitTouched && commitValidation}
        <div class="commit-validation" role="status">{commitValidation}</div>
      {:else}
        <div class="commit-helper">
          <span>Commit uses staged changes only. Unstaged files stay in your working tree.</span>
        </div>
      {/if}

      <button
        type="button"
        class="commit-button"
        disabled={commitDisabled}
        onclick={() => void commit()}
      >
        {#if actionBusy === "commit"}<Loader2 class="spin" size={16} aria-hidden="true" />{/if}
        {actionBusy === "commit"
          ? "Committing..."
          : stagedCount > 0
            ? `Commit ${stagedCount} staged`
            : "No staged changes"}
      </button>
    </section>

    <section class="source-card changes-card" aria-label="Staged changes">
      <div class="card-label-row">
        <div>
          <span>Staged Changes</span>
          <h3>
            {stagedCount > 0
              ? `${stagedCount} file${stagedCount === 1 ? "" : "s"}`
              : "No staged files"}
          </h3>
        </div>
        {#if stagedCount > 0}<strong>{stagedCount}</strong>{/if}
      </div>

      {#if stagedFiles.length > 0}
        <div class="changes-list">
          {#each stagedFiles as file (fileLabel(file))}
            <div
              role="presentation"
              oncontextmenu={(event) => openContextMenu("staged", file, event)}
            >
              <div class="change-row">
                <WorkspaceRow
                  label={file.path}
                  title={fileLabel(file)}
                  status={statusLabel(file.status)}
                  statusTone={statusTone(file.status)}
                  onClick={() => onOpenFile(file.path)}
                />
                <button
                  class="row-action"
                  type="button"
                  aria-label={`Unstage ${file.path}`}
                  title={`Unstage ${file.path}`}
                  disabled={actionBusy !== null}
                  onclick={(event) => {
                    event.stopPropagation();
                    void runAction("unstage", () => onUnstageFile(file.path));
                  }}><Minus size={14} aria-hidden="true" /></button
                >
              </div>
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty-state">
          <strong>No staged changes</strong><span
            >Use Stage on a file or Stage All to prepare a commit.</span
          >
        </div>
      {/if}
    </section>

    <section class="source-card changes-card" aria-label="Changes">
      <div class="card-label-row">
        <div>
          <span>Changes</span>
          <h3>
            {unstagedCount > 0
              ? `${unstagedCount} file${unstagedCount === 1 ? "" : "s"}`
              : repoClean
                ? "Working tree clean"
                : "No unstaged changes"}
          </h3>
        </div>
        {#if unstagedCount > 0}<strong>{unstagedCount}</strong>{/if}
      </div>

      {#if unstagedFiles.length > 0}
        <div class="changes-list">
          {#each unstagedFiles as file (fileLabel(file))}
            <div
              role="presentation"
              oncontextmenu={(event) => openContextMenu("unstaged", file, event)}
            >
              <div class="change-row">
                <WorkspaceRow
                  label={file.path}
                  title={fileLabel(file)}
                  status={statusLabel(file.status)}
                  statusTone={statusTone(file.status)}
                  onClick={() => onOpenFile(file.path)}
                />
                <button
                  class="row-action"
                  type="button"
                  aria-label={`Stage ${file.path}`}
                  title={`Stage ${file.path}`}
                  disabled={actionBusy !== null}
                  onclick={(event) => {
                    event.stopPropagation();
                    void runAction("stage", () => onStageFile(file.path));
                  }}><Plus size={14} aria-hidden="true" /></button
                >
              </div>
            </div>
          {/each}
        </div>
      {:else if repoClean}
        <div class="empty-state clean">
          <strong>Working tree clean</strong><span
            >No modified, added, deleted, renamed, or untracked files.</span
          >
        </div>
      {:else}
        <div class="empty-state">
          <strong>No unstaged changes</strong><span>All current changes are staged for commit.</span
          >
        </div>
      {/if}
    </section>
  </div>

  {#if contextMenu}
    <div
      class="context-menu"
      role="menu"
      tabindex="-1"
      style={`left: ${contextMenu.x}px; top: ${contextMenu.y}px;`}
    >
      <button
        type="button"
        role="menuitem"
        onclick={() => {
          onOpenFile(contextMenu!.file.path);
          contextMenu = null;
        }}>Open File</button
      >
      {#if contextMenu.kind === "unstaged"}
        <button
          type="button"
          role="menuitem"
          disabled={actionBusy !== null}
          onclick={() => void runAction("stage", () => onStageFile(contextMenu!.file.path))}
          >Stage File</button
        >
      {:else}
        <button
          type="button"
          role="menuitem"
          disabled={actionBusy !== null}
          onclick={() => void runAction("unstage", () => onUnstageFile(contextMenu!.file.path))}
          >Unstage File</button
        >
      {/if}
    </div>
  {/if}
</aside>

<style>
  .git-actions-pane {
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100%;
    min-width: 0;
    min-height: 0;
    box-sizing: border-box;
    overflow: hidden;
    container-type: inline-size;
    border: 1px solid var(--border-subtle);
    border-radius: 14px;
    background: linear-gradient(
      180deg,
      var(--surface),
      color-mix(in srgb, var(--surface) 96%, var(--border-strong))
    );
  }

  .source-header {
    flex: 0 0 auto;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 12px;
    align-items: start;
    border-bottom: 1px solid var(--border-subtle);
    padding: 14px;
    background: linear-gradient(180deg, var(--surface-overlay), var(--surface-raised));
  }

  .source-header-copy {
    min-width: 0;
    overflow: hidden;
  }
  .source-header-copy p,
  .source-header-copy h2 {
    margin: 0;
  }
  .source-header-copy p,
  .card-label-row span {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  .source-header-copy h2 {
    margin-top: 6px;
    overflow: hidden;
    color: var(--text-bright);
    font-size: 13px;
    font-weight: 900;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .source-header-meta {
    display: flex;
    flex-wrap: wrap;
    min-width: 0;
    gap: 8px;
    margin-top: 10px;
  }
  .source-header-meta span,
  .card-label-row small,
  .commit-helper span,
  .empty-state span {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 800;
  }
  .source-header-meta span {
    display: block;
    max-width: 100%;
    min-height: 24px;
    box-sizing: border-box;
    overflow: hidden;
    padding: 0 9px;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    background: color-mix(in srgb, var(--surface-hover) 80%, transparent);
    line-height: 22px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .source-header-controls {
    display: grid;
    justify-self: stretch;
    width: 100%;
    gap: 12px;
    min-width: 0;
  }
  .branch-menu-wrap {
    position: relative;
    width: 100%;
    min-width: 0;
  }
  .branch-menu-button,
  .source-toolbar button,
  .commit-button,
  .branch-menu-actions button,
  .row-action,
  .context-menu button {
    border: 1px solid var(--border-strong);
    background: var(--surface-hover);
    color: var(--text-primary);
  }
  .branch-menu-button {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 9px;
    align-items: center;
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
    padding: 9px;
    border-radius: 12px;
    text-align: left;
  }
  .branch-menu-button-copy {
    display: grid;
    width: 100%;
    min-width: 0;
    overflow: hidden;
    gap: 3px;
  }
  .branch-menu-button-copy small,
  .branch-menu-button-copy strong {
    display: block;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .branch-menu-button-copy small {
    color: var(--text-secondary);
    font-size: 10px;
    font-weight: 900;
    text-transform: uppercase;
  }
  .branch-menu-button-copy strong {
    color: var(--text-bright);
    font-size: 12px;
  }
  .branch-menu-button-indicator {
    display: inline-flex;
    flex: 0 0 auto;
    color: var(--accent);
  }
  .branch-menu {
    position: absolute;
    z-index: 20;
    right: 0;
    top: calc(100% + 8px);
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    padding: 12px;
    background: var(--menu-bg);
    box-shadow: var(--shadow-popover);
  }
  .branch-menu-copy p,
  .branch-menu-copy strong,
  .branch-menu-copy span {
    display: block;
    margin: 0;
  }
  .branch-menu-copy {
    min-width: 0;
    overflow: hidden;
  }
  .branch-menu-copy p {
    color: var(--text-secondary);
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  .branch-menu-copy strong {
    margin-top: 5px;
    overflow: hidden;
    color: var(--text-bright);
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .branch-menu-copy span {
    margin-top: 4px;
    color: var(--text-secondary);
    font-size: 11px;
    overflow-wrap: anywhere;
  }
  .branch-menu label {
    display: grid;
    min-width: 0;
    gap: 6px;
    margin-top: 12px;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    text-transform: uppercase;
  }
  .branch-menu select,
  .commit-box textarea {
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--surface-inset);
    color: var(--text-bright);
  }
  .branch-menu select {
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
    padding: 9px 10px;
    text-overflow: ellipsis;
  }
  .branch-menu-actions {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    min-width: 0;
    gap: 8px;
    margin-top: 12px;
  }
  .branch-menu-actions button,
  .source-toolbar button,
  .commit-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    min-height: 34px;
    min-width: 0;
    box-sizing: border-box;
    border-radius: 10px;
    padding: 0 10px;
    font-size: 12px;
    font-weight: 900;
  }
  button:disabled {
    opacity: 0.46;
    cursor: not-allowed;
  }
  .git-error,
  .commit-validation {
    border: 1px solid var(--danger-border);
    border-radius: 12px;
    background: var(--danger-bg);
    color: var(--danger);
    font-size: 12px;
    font-weight: 800;
  }
  .git-error {
    margin: 12px 12px 0;
    padding: 10px 12px;
  }
  .commit-validation {
    padding: 8px 10px;
  }
  .source-body {
    flex: 1 1 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-auto-rows: max-content;
    align-content: start;
    gap: 12px;
    min-width: 0;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
    padding: 12px;
  }
  .source-card {
    min-width: 0;
    min-height: max-content;
    box-sizing: border-box;
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 14px;
    background: var(--surface-raised);
  }
  .card-label-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 12px;
    padding: 12px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .card-label-row > div {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
  }
  .card-label-row h3 {
    margin: 4px 0 0;
    overflow: hidden;
    color: var(--text-bright);
    font-size: 13px;
    font-weight: 900;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-label-row > small,
  .card-label-row > strong {
    flex: 0 0 auto;
    min-width: max-content;
  }
  .card-label-row strong {
    color: var(--accent);
    font-size: 12px;
  }
  .source-toolbar {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 128px), 1fr));
    min-width: 0;
    gap: 8px;
    padding: 12px;
  }
  .commit-card {
    display: grid;
    gap: 10px;
    padding-bottom: 12px;
  }
  .commit-box {
    padding: 0 12px;
  }
  .commit-box textarea {
    width: 100%;
    box-sizing: border-box;
    max-width: 100%;
    resize: vertical;
    min-height: 92px;
    padding: 11px 12px;
    font: inherit;
    line-height: 1.35;
  }
  .commit-helper,
  .commit-validation {
    margin: 0 12px;
  }
  .commit-button {
    margin: 0 12px;
    background: linear-gradient(
      135deg,
      var(--accent),
      color-mix(in srgb, var(--accent) 68%, var(--info))
    );
    border-color: transparent;
    color: var(--surface);
  }
  .changes-list {
    display: grid;
    min-width: 0;
    max-height: clamp(144px, 32vh, 320px);
    overflow-x: hidden;
    overflow-y: auto;
  }
  .change-row {
    position: relative;
    min-width: 0;
  }
  .change-row :global(.workspace-row) {
    padding-right: 48px;
  }
  :global(.workspace-row .workspace-row-status) {
    min-width: 66px;
    text-align: left;
    letter-spacing: 0;
    text-transform: none;
  }
  .row-action {
    position: absolute;
    z-index: 1;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    color: var(--text-primary);
  }
  .row-action:not(:disabled):hover {
    border-color: var(--focus-border);
    color: var(--text-bright);
  }
  .empty-state {
    display: grid;
    place-items: center;
    gap: 8px;
    min-height: 112px;
    padding: 18px;
    color: var(--text-bright);
    text-align: center;
  }
  .empty-state strong {
    font-size: 13px;
  }
  .empty-state span,
  .git-error,
  .commit-validation,
  .commit-helper {
    overflow-wrap: anywhere;
  }
  .context-menu {
    position: fixed;
    z-index: 200;
    display: grid;
    min-width: 160px;
    overflow: hidden;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--menu-bg);
    box-shadow: var(--shadow-popover);
  }
  .context-menu button {
    border: 0;
    border-bottom: 1px solid var(--border-subtle);
    border-radius: 0;
    padding: 10px 12px;
    text-align: left;
    font-size: 12px;
    font-weight: 850;
  }
  .context-menu button:last-child {
    border-bottom: 0;
  }
  .context-menu button:not(:disabled):hover {
    background: var(--surface-hover);
  }

  :global(.spin) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.spin) {
      animation: none;
    }
  }

  @container (min-width: 390px) {
    .source-header {
      grid-template-columns: minmax(0, 1fr) minmax(180px, 220px);
    }

    .source-header-controls {
      justify-self: end;
      max-width: 220px;
    }
  }
</style>
