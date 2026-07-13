<script lang="ts">
  import { ArrowDownToLine, ArrowUpFromLine, ChevronDown, FileText, GitMerge, Loader2, RefreshCw } from "lucide-svelte";
  import GitBranchPicker from "$lib/components/GitBranchPicker.svelte";
  import WorkspaceRow from "$lib/components/WorkspaceRow.svelte";
  import type { GitChangedFile, GitWorkspaceInfo } from "$lib/ipc/git";

  type Props = {
    workspaceGitInfo: GitWorkspaceInfo | null;
    workspaceGitLoading: boolean;
    workspaceGitError: string | null;
    switchingWorkspaceBranch: boolean;
    hasDirtyEditors: boolean;
    changedFiles: GitChangedFile[];
    onSwitchBranch: (branch: string) => void;
    onPull: () => Promise<void>;
    onCommit: (message: string) => Promise<void>;
    onPush: () => Promise<void>;
    onMerge: (branch: string) => Promise<void>;
    onRebase: (branch: string) => Promise<void>;
    onRefresh: () => Promise<void>;
    onOpenFile: (path: string) => void;
  };

  type GitActionName = "pull" | "commit" | "push" | "merge" | "rebase" | "refresh";

  let {
    workspaceGitInfo,
    workspaceGitLoading,
    workspaceGitError,
    switchingWorkspaceBranch,
    hasDirtyEditors,
    changedFiles,
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
  let mergeBranch = $state("");
  let actionsMenuOpen = $state(false);
  let actionBusy = $state<GitActionName | null>(null);

  let branchChoices = $derived(
    (workspaceGitInfo?.branches ?? []).filter((branch) => branch !== workspaceGitInfo?.current_branch),
  );

  $effect(() => {
    if (!branchChoices.includes(mergeBranch)) {
      mergeBranch = branchChoices[0] ?? "";
    }
  });

  let hasRepo = $derived(Boolean(workspaceGitInfo?.is_git_repo));
  let worktreeDirty = $derived(Boolean(workspaceGitInfo?.dirty_worktree || hasDirtyEditors));
  let changedCount = $derived(changedFiles.length);
  let currentBranchLabel = $derived(workspaceGitInfo?.current_branch ?? "detached");
  let commitCharCount = $derived(commitMessage.length);

  let headerMeta = $derived.by(() => {
    const chips: string[] = [];
    if (hasRepo) chips.push(currentBranchLabel);
    if (changedCount > 0) chips.push(`${changedCount} change${changedCount === 1 ? "" : "s"}`);
    if (workspaceGitInfo?.ahead_count) chips.push(`${workspaceGitInfo.ahead_count} ahead`);
    if (workspaceGitInfo?.behind_count) chips.push(`${workspaceGitInfo.behind_count} behind`);
    if (worktreeDirty) chips.push("Unsaved edits");
    if (!hasRepo) chips.push("Not a git repository");
    return chips;
  });

  const busyLabels: Record<GitActionName, string> = {
    pull: "Pulling...",
    commit: "Committing...",
    push: "Pushing...",
    merge: "Merging...",
    rebase: "Rebasing...",
    refresh: "Refreshing...",
  };

  function statusTone(status: string): "neutral" | "modified" | "added" | "deleted" {
    if (status === "M") return "modified";
    if (status === "A" || status === "U") return "added";
    if (status === "D") return "deleted";
    return "neutral";
  }

  function actionLabel(base: GitActionName) {
    return actionBusy === base ? busyLabels[base] : base[0].toUpperCase() + base.slice(1);
  }

  async function runAction(name: GitActionName, handler: () => Promise<void>) {
    if (actionBusy) return;
    actionBusy = name;
    try {
      await handler();
    } finally {
      actionBusy = null;
    }
  }

  const commitDisabled = $derived(!hasRepo || worktreeDirty || !changedCount || !commitMessage.trim() || actionBusy !== null);
  const syncDisabled = $derived(!hasRepo || worktreeDirty || workspaceGitLoading || switchingWorkspaceBranch || actionBusy !== null);
  const mergeDisabled = $derived(!hasRepo || worktreeDirty || !mergeBranch || switchingWorkspaceBranch || actionBusy !== null);
  const rebaseDisabled = $derived(!hasRepo || worktreeDirty || !mergeBranch || switchingWorkspaceBranch || actionBusy !== null);
  const pushDisabled = $derived(!hasRepo || worktreeDirty || switchingWorkspaceBranch || actionBusy !== null);
  const refreshDisabled = $derived(!hasRepo || workspaceGitLoading || actionBusy !== null);

  function closeMenu() {
    actionsMenuOpen = false;
  }

  async function runMergeOrRebase(kind: "merge" | "rebase") {
    if (!mergeBranch) return;
    closeMenu();
    if (kind === "merge") {
      await runAction("merge", () => onMerge(mergeBranch));
    } else {
      await runAction("rebase", () => onRebase(mergeBranch));
    }
  }
</script>

<aside class="git-actions-pane" aria-label="Source control">
  <header class="source-header">
    <div class="source-header-copy">
      <p>Source control</p>
      <h2>Manage branches and changes</h2>
      <div class="source-header-meta">
        {#if headerMeta.length > 0}
          {#each headerMeta as meta}
            <span>{meta}</span>
          {/each}
        {/if}
      </div>
    </div>

    <div class="source-header-controls">
      <GitBranchPicker gitInfo={workspaceGitInfo} loading={workspaceGitLoading} switching={switchingWorkspaceBranch} dirty={worktreeDirty} onSwitch={onSwitchBranch} />

      <div class="branch-menu-wrap">
        <button type="button" class="branch-menu-button" disabled={!hasRepo || switchingWorkspaceBranch || actionBusy !== null} aria-haspopup="dialog" aria-expanded={actionsMenuOpen} onclick={() => (actionsMenuOpen = !actionsMenuOpen)}>
          <span class="branch-menu-button-icon">
            <GitMerge size={14} />
          </span>
          <span class="branch-menu-button-copy">
            <small>Target branch</small>
            <strong>{mergeBranch || branchChoices[0] || "Choose branch"}</strong>
          </span>
          <span class="branch-menu-button-indicator">
            <ChevronDown size={14} class="open" />
          </span>
        </button>
        {#if actionsMenuOpen}
          <div class="branch-menu" role="dialog" aria-label="Merge or rebase target branch" aria-modal="false">
            <div class="branch-menu-copy">
              <p>Merge / rebase</p>
              <strong>Choose a target branch</strong>
              <span>Pick a branch, then choose how to combine history.</span>
            </div>
            <label>
              <span>Branch</span>
              <select bind:value={mergeBranch} disabled={branchChoices.length === 0 || switchingWorkspaceBranch || actionBusy !== null}>
                {#if branchChoices.length === 0}
                  <option value="">No other branches</option>
                {:else}
                  {#each branchChoices as branch}
                    <option value={branch}>{branch}</option>
                  {/each}
                {/if}
              </select>
            </label>
            <div class="branch-menu-actions">
              <button type="button" disabled={mergeDisabled} onclick={() => void runMergeOrRebase("merge")}><GitMerge size={14} /> Merge</button>
              <button type="button" disabled={rebaseDisabled} onclick={() => void runMergeOrRebase("rebase")}>Rebase</button>
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
          <h3>Sync the workspace</h3>
        </div>
        <small>{worktreeDirty ? "Review changes before syncing" : "Ready"}</small>
      </div>
      <div class="source-toolbar">
        <button type="button" disabled={syncDisabled} onclick={() => void runAction("pull", onPull)}><ArrowDownToLine size={14} /> {actionLabel("pull")}</button>
        <button type="button" disabled={pushDisabled} onclick={() => void runAction("push", onPush)}><ArrowUpFromLine size={14} /> {actionLabel("push")}</button>
        <button type="button" disabled={refreshDisabled} onclick={() => void runAction("refresh", onRefresh)}><RefreshCw size={14} /> {actionLabel("refresh")}</button>
      </div>
    </section>

    <section class="source-card commit-card" aria-label="Commit message">
      <div class="card-label-row">
        <div>
          <span>Commit</span>
          <h3>Summarize the change</h3>
        </div>
        <small>{commitCharCount} chars</small>
      </div>

      <label class="commit-box">
        <textarea bind:value={commitMessage} placeholder={changedCount > 0 ? "Describe what changed and why" : "No changes yet"} rows="6" disabled={!hasRepo || actionBusy !== null}></textarea>
      </label>

      <div class="commit-helper">
        {#if changedCount > 0}
          <span>Use a short, specific message. The first line becomes the summary.</span>
        {:else}
          <span>Make a change to unlock commit creation.</span>
        {/if}
      </div>

      <button type="button" class="commit-button" disabled={commitDisabled} onclick={() => void runAction("commit", () => onCommit(commitMessage.trim()))}>
        {#if actionBusy === "commit"}
          <Loader2 size={15} />
        {/if}
        {actionBusy === "commit" ? "Committing..." : changedCount > 0 ? `Commit ${changedCount} ${changedCount === 1 ? "change" : "changes"}` : "No changes to commit"}
      </button>
    </section>

    <section class="source-card changes-card" aria-label="Changes">
      <div class="card-label-row">
        <div>
          <span>Changes</span>
          <h3>{changedCount > 0 ? `${changedCount} file${changedCount === 1 ? "" : "s"}` : "Working tree clean"}</h3>
        </div>
        {#if changedCount > 0}
          <strong>{changedCount}</strong>
        {/if}
      </div>

      {#if changedFiles.length > 0}
        <div class="changes-list">
          {#each changedFiles as file (file.path)}
            {#snippet rowLeading()}
              <FileText size={14} class="row-icon" />
            {/snippet}
            <WorkspaceRow
              label={file.path}
              title={file.path}
              status={file.status}
              statusTone={statusTone(file.status)}
              leading={rowLeading}
              onClick={() => onOpenFile(file.path)}
            />
          {/each}
        </div>
      {:else}
        <div class="empty-state">
          <div class="empty-icon"><FileText size={18} /></div>
          <strong>Working tree clean</strong>
          <span>Modified, added, and deleted files will appear here.</span>
        </div>
      {/if}
    </section>
  </div>
</aside>

<style>
  .git-actions-pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    border: 1px solid #272530;
    border-radius: 14px;
    background: linear-gradient(180deg, #121118, #101017);
  }

  .source-header {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(200px, 260px);
    gap: 12px;
    align-items: start;
    border-bottom: 1px solid #25232d;
    padding: 14px 14px 15px;
    background: linear-gradient(180deg, #1c1b21, #17161c);
  }

  .source-header-copy {
    min-width: 0;
  }

  .source-header-copy p,
  .source-header-copy h2 {
    margin: 0;
  }

  .source-header-copy p {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .source-header-copy h2 {
    margin-top: 6px;
    color: #f1edf5;
    font-size: 13px;
    font-weight: 900;
    letter-spacing: -0.01em;
  }

  .source-header-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
  }

  .source-header-meta span,
  .card-label-row small,
  .commit-helper span,
  .empty-state span {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 800;
  }

  .source-header-meta span {
    display: inline-flex;
    align-items: center;
    min-height: 24px;
    padding: 0 9px;
    border: 1px solid #2f2d37;
    border-radius: 999px;
    background: rgba(31, 30, 39, 0.8);
  }

  .source-header-controls {
    display: grid;
    justify-self: end;
    width: 100%;
    max-width: 260px;
    gap: 12px;
    min-width: 0;
  }

  .branch-menu-wrap {
    position: relative;
  }

  .branch-menu-button,
  .source-toolbar button,
  .commit-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 36px;
    border: 1px solid #34313d;
    border-radius: 10px;
    background: #1f1e27;
    color: #b8d6e4;
    font: inherit;
    font-size: 12px;
    font-weight: 900;
    cursor: pointer;
    transition:
      border-color 180ms ease,
      background-color 180ms ease,
      color 180ms ease,
      box-shadow 180ms ease,
      transform 180ms ease;
  }

  .branch-menu-button {
    display: flex;
    justify-content: space-between;
    width: 100%;
    min-width: 0;
    min-height: 40px;
    padding: 0 10px 0 8px;
    border-radius: 12px;
    background: linear-gradient(180deg, #22212a, #1b1a21);
  }

  .branch-menu-button-copy {
    display: grid;
    min-width: 0;
    gap: 0;
    text-align: left;
  }

  .branch-menu-button-copy small {
    color: #8f88a8;
    font-size: 8px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .branch-menu-button-copy strong {
    overflow: hidden;
    font-family: var(--font-mono);
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 900;
    color: #f1edf5;
  }

  .branch-menu-button-icon,
  .branch-menu-button-indicator {
    display: inline-grid;
    place-items: center;
    flex: 0 0 auto;
    width: 24px;
    height: 24px;
    border-radius: 8px;
    background: rgba(31, 30, 39, 0.9);
    color: #b8d6e4;
  }

  .branch-menu-button-indicator {
    width: 22px;
    height: 22px;
    border-radius: 7px;
    color: #8f88a8;
  }

  .branch-menu-button:hover:not(:disabled),
  .branch-menu-button:focus-visible:not(:disabled),
  .source-toolbar button:hover:not(:disabled),
  .source-toolbar button:focus-visible:not(:disabled),
  .commit-button:hover:not(:disabled),
  .commit-button:focus-visible:not(:disabled) {
    border-color: #52606a;
    background: #28313a;
    color: #f1edf5;
    box-shadow: 0 0 0 3px rgba(184, 214, 228, 0.08);
  }

  .branch-menu-button:active:not(:disabled),
  .source-toolbar button:active:not(:disabled),
  .commit-button:active:not(:disabled) {
    transform: translateY(1px);
  }

  .branch-menu-button:disabled,
  .source-toolbar button:disabled,
  .commit-button:disabled,
  .commit-box textarea:disabled,
  .branch-menu select:disabled {
    cursor: not-allowed;
    opacity: 0.68;
  }

  .branch-menu {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 20;
    display: grid;
    gap: 12px;
    width: min(320px, calc(100vw - 32px));
    border: 1px solid #34313d;
    border-radius: 13px;
    padding: 12px;
    background: linear-gradient(180deg, #1d1c23, #181720);
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.48);
  }

  .branch-menu-copy {
    display: grid;
    gap: 4px;
  }

  .branch-menu-copy p,
  .branch-menu-copy strong {
    margin: 0;
  }

  .branch-menu-copy p {
    color: #8f88a8;
    font-size: 9px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .branch-menu-copy strong {
    color: #f1edf5;
    font-size: 13px;
    font-weight: 900;
  }

  .branch-menu-copy span {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 800;
  }

  .branch-menu label {
    display: grid;
    gap: 6px;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .branch-menu select {
    min-height: 32px;
    border: 1px solid #34313d;
    border-radius: 8px;
    background: #1f1e27;
    color: #f1edf5;
    font: inherit;
    padding: 0 10px;
  }

  .branch-menu-actions {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }

  .branch-menu-actions button {
    min-height: 36px;
    border: 1px solid #34313d;
    border-radius: 9px;
    background: #20262d;
    color: #b8d6e4;
    font: inherit;
    font-size: 11px;
    font-weight: 900;
  }

  .branch-menu-actions button:first-child {
    background: #223140;
    color: #f1edf5;
  }

  .branch-menu-actions button:first-child:hover:not(:disabled),
  .branch-menu-actions button:first-child:focus-visible:not(:disabled) {
    border-color: #52606a;
    background: #2b3b4d;
  }

  .git-error {
    border-bottom: 1px solid #3d2930;
    padding: 10px 16px;
    background: rgba(124, 73, 82, 0.18);
    color: #f0b0aa;
    font-size: 12px;
    font-weight: 800;
  }

  .source-body {
    display: grid;
    gap: 16px;
    min-height: 0;
    padding: 16px 14px 16px;
    overflow: auto;
    scrollbar-gutter: stable;
  }

  .source-card {
    display: grid;
    gap: 14px;
    min-width: 0;
    border: 1px solid #25232d;
    border-radius: 15px;
    padding: 15px;
    background: #111016;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.02);
  }

  .card-label-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 12px;
  }

  .card-label-row span {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .card-label-row h3 {
    margin: 4px 0 0;
    color: #f1edf5;
    font-size: 13px;
    font-weight: 900;
  }

  .card-label-row strong {
    color: #b8d6e4;
    font-size: 12px;
    font-weight: 900;
  }

  .source-toolbar {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 9px;
  }

  .source-toolbar button {
    min-width: 0;
    padding: 0 10px;
    justify-content: center;
  }

  .commit-box {
    display: grid;
    min-width: 0;
  }

  .commit-box textarea {
    resize: vertical;
    min-height: 114px;
    border: 1px solid #34313d;
    border-radius: 12px;
    background: #1f1e27;
    color: #f1edf5;
    font: inherit;
    line-height: 1.45;
    padding: 12px 13px;
    transition:
      border-color 180ms ease,
      box-shadow 180ms ease,
      background-color 180ms ease;
  }

  .commit-box textarea::placeholder {
    color: #706980;
  }

  .commit-box textarea:focus-visible {
    outline: none;
    border-color: #52606a;
    box-shadow: 0 0 0 3px rgba(184, 214, 228, 0.1);
    background: #23212b;
  }

  .commit-helper {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  .commit-button {
    width: 100%;
    min-height: 48px;
    border-color: #e0ddd7;
    background: #f4f2ee;
    color: #111016;
    box-shadow: 0 8px 20px rgba(0, 0, 0, 0.14);
  }

  .commit-button:hover:not(:disabled),
  .commit-button:focus-visible:not(:disabled) {
    border-color: #f4f2ee;
    background: #ffffff;
    color: #111016;
    box-shadow: 0 10px 22px rgba(0, 0, 0, 0.18), 0 0 0 3px rgba(244, 242, 238, 0.15);
  }

  .changes-list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: auto;
    scrollbar-gutter: stable;
    padding: 2px 6px 2px 0;
  }

  .empty-state {
    display: grid;
    justify-items: center;
    gap: 8px;
    padding: 20px 14px 22px;
    text-align: center;
  }

  .empty-icon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border: 1px solid #2f2d37;
    border-radius: 12px;
    background: #1f1e27;
    color: #8f88a8;
  }

  .empty-state strong {
    color: #f1edf5;
    font-size: 13px;
    font-weight: 900;
  }
</style>
