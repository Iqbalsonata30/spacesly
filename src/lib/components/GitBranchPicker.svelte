<script lang="ts">
  import type { GitWorkspaceInfo } from "$lib/ipc";

  type Props = {
    gitInfo: GitWorkspaceInfo | null;
    loading: boolean;
    switching: boolean;
    dirty: boolean;
    onSwitch: (branch: string) => void;
  };

  let { gitInfo, loading, switching, dirty, onSwitch }: Props = $props();
  let open = $state(false);
  let filter = $state("");

  let branches = $derived.by(() => {
    const value = filter.trim().toLowerCase();
    const allBranches = gitInfo?.branches ?? [];
    if (!value) return allBranches;
    return allBranches.filter((branch) => branch.toLowerCase().includes(value));
  });

  function openPicker() {
    if (!gitInfo?.is_git_repo) return;
    filter = "";
    open = true;
  }

  function switchBranch(branch: string) {
    open = false;
    onSwitch(branch);
  }
</script>

{#if gitInfo?.is_git_repo}
  <button class="branch-button" type="button" disabled={loading || switching} onclick={openPicker}>
    branch: {switching ? "switching" : gitInfo.current_branch ?? "detached"}
  </button>
{/if}

{#if open && gitInfo?.is_git_repo}
  <div class="branch-picker-backdrop" role="presentation" onclick={() => (open = false)}></div>
  <div class="branch-picker" role="dialog" aria-label="Switch git branch" aria-modal="true" tabindex="-1">
    <header>
      <div>
        <p>Switch Branch</p>
        <h3>{gitInfo.current_branch ?? "Detached HEAD"}</h3>
      </div>
      <button type="button" onclick={() => (open = false)} aria-label="Close branch picker">×</button>
    </header>
    <input aria-label="Filter branches" placeholder="Type to filter branches" bind:value={filter} />
    {#if dirty}
      <div class="branch-picker-warning">Save or close unsaved files before switching branches.</div>
    {/if}
    <div class="branch-picker-list">
      {#each branches as branch}
        <button
          class:active={branch === gitInfo.current_branch}
          type="button"
          disabled={branch === gitInfo.current_branch || dirty || switching}
          onclick={() => switchBranch(branch)}
        >
          <strong>{branch}</strong>
          <small>{branch === gitInfo.current_branch ? "current branch" : "switch to branch"}</small>
        </button>
      {:else}
        <div class="branch-picker-empty">No branches match.</div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .branch-button {
    flex: 0 0 auto;
    height: 24px;
    max-width: 220px;
    overflow: hidden;
    padding: 0 9px;
    border: 1px solid #34313d;
    border-radius: 6px;
    background: #1f1e27;
    color: #b8d6e4;
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 900;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-button:hover:not(:disabled),
  .branch-button:focus-visible:not(:disabled) {
    border-color: #b8d6e4;
    background: #28313a;
    color: #f1edf5;
  }

  .branch-picker-backdrop {
    position: absolute;
    inset: 0;
    z-index: 15;
    background: rgba(0, 0, 0, 0.18);
  }

  .branch-picker {
    position: absolute;
    left: 50%;
    bottom: 42px;
    z-index: 16;
    display: grid;
    width: min(460px, calc(100% - 36px));
    max-height: min(520px, calc(100% - 84px));
    overflow: hidden;
    transform: translateX(-50%);
    border: 1px solid #34313d;
    border-radius: 14px;
    background: #181720;
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.48);
  }

  .branch-picker header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid #25232d;
    padding: 12px 14px;
    background: #1c1b21;
  }

  .branch-picker header p,
  .branch-picker header h3 {
    margin: 0;
  }

  .branch-picker header p {
    color: #8f88a8;
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .branch-picker header h3 {
    overflow: hidden;
    color: #f1edf5;
    font-size: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-picker header button {
    width: 32px;
    height: 32px;
    padding: 0;
    border-color: #34313d;
    background: #1f1e27;
    color: #b8d6e4;
  }

  .branch-picker input {
    height: 42px;
    border: 0;
    border-bottom: 1px solid #25232d;
    padding: 0 14px;
    background: #111016;
    color: #f1edf5;
    font: inherit;
    outline: none;
  }

  .branch-picker-warning {
    border-bottom: 1px solid #25232d;
    padding: 9px 14px;
    background: rgba(240, 209, 137, 0.1);
    color: #f0d189;
    font-size: 12px;
    font-weight: 850;
  }

  .branch-picker-list {
    display: grid;
    align-content: start;
    gap: 4px;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
  }

  .branch-picker-list button {
    display: grid;
    gap: 3px;
    min-width: 0;
    border-color: transparent;
    padding: 9px 10px;
    background: transparent;
    text-align: left;
  }

  .branch-picker-list button:hover:not(:disabled),
  .branch-picker-list button:focus-visible:not(:disabled),
  .branch-picker-list button.active {
    border-color: #52606a;
    background: #20262d;
  }

  .branch-picker-list strong {
    overflow: hidden;
    color: #f1edf5;
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-picker-list small,
  .branch-picker-empty {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 800;
  }

  .branch-picker-empty {
    padding: 18px 10px;
    text-align: center;
  }
</style>
