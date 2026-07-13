<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ChevronDown, GitBranch } from "lucide-svelte";
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
  let filterInput: HTMLInputElement | null = $state(null);

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

  $effect(() => {
    if (!open) return;
    void tick().then(() => filterInput?.focus());
  });

  onMount(() => {
    const handleKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && open) {
        open = false;
      }
    };

    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  });
</script>

{#if gitInfo?.is_git_repo}
  <div class="branch-selector">
    <button
      class="branch-button"
      type="button"
      disabled={loading || switching}
      aria-haspopup="dialog"
      aria-expanded={open}
      onclick={openPicker}
    >
      <span class="branch-button-icon">
        <GitBranch size={14} />
      </span>
      <span class="branch-button-copy">
        <small>{switching ? "Switching branch" : "Current branch"}</small>
        <strong>{gitInfo.current_branch ?? "detached"}</strong>
      </span>
      <span class="branch-button-indicator">
        <ChevronDown size={14} class="open" />
      </span>
    </button>

    {#if open}
      <div class="branch-picker-backdrop" role="presentation" onclick={() => (open = false)}></div>
      <div class="branch-picker" role="dialog" aria-label="Switch git branch" aria-modal="true" tabindex="-1">
        <header>
          <div>
            <p>Switch Branch</p>
            <h3>{gitInfo.current_branch ?? "Detached HEAD"}</h3>
          </div>
          <button type="button" onclick={() => (open = false)} aria-label="Close branch picker">×</button>
        </header>
        <input aria-label="Filter branches" placeholder="Filter branches" bind:this={filterInput} bind:value={filter} />
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
  </div>
{/if}

<style>
  .branch-selector {
    position: relative;
    display: grid;
    min-width: 0;
  }

  .branch-button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    min-width: 0;
    min-height: 46px;
    padding: 0 12px 0 10px;
    border: 1px solid var(--border-light);
    border-radius: 13px;
    background: linear-gradient(180deg, #22212a, #1b1a21);
    color: var(--text-bright);
    font: inherit;
    font-size: 12px;
    font-weight: 850;
    cursor: pointer;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
    transition:
      border-color 180ms ease,
      background-color 180ms ease,
      color 180ms ease,
      box-shadow 180ms ease,
      transform 180ms ease;
  }

  .branch-button:hover:not(:disabled),
  .branch-button:focus-visible:not(:disabled) {
    border-color: #52606a;
    background: linear-gradient(180deg, #2a3038, #20262d);
    color: #f1edf5;
    box-shadow: 0 0 0 3px rgba(184, 214, 228, 0.08), inset 0 1px 0 rgba(255, 255, 255, 0.04);
  }

  .branch-button:active:not(:disabled) {
    transform: translateY(1px);
  }

  .branch-button:disabled {
    cursor: not-allowed;
    opacity: 0.72;
  }

  .branch-button-copy {
    display: grid;
    min-width: 0;
    gap: 1px;
    text-align: left;
  }

  .branch-button-copy small {
    color: var(--text-secondary);
    font-size: 9px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .branch-button-copy strong {
    overflow: hidden;
    font-family: var(--font-mono);
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 900;
  }

  .branch-button-icon,
  .branch-button-indicator {
    display: inline-grid;
    place-items: center;
    flex: 0 0 auto;
    width: 28px;
    height: 28px;
    border-radius: 9px;
    background: rgba(31, 30, 39, 0.9);
    color: var(--accent);
  }

  .branch-button-indicator {
    width: 26px;
    height: 26px;
    border-radius: 8px;
    color: var(--text-secondary);
  }

  .branch-button :global(svg) {
    flex: 0 0 auto;
    transition: transform 180ms ease;
  }

  .branch-button :global(svg.open) {
    transform: rotate(180deg);
  }

  .branch-picker-backdrop {
    position: fixed;
    inset: 0;
    z-index: 15;
    background: rgba(9, 9, 13, 0.32);
    backdrop-filter: blur(2px);
  }

  .branch-picker {
    position: absolute;
    top: calc(100% + 10px);
    left: 0;
    z-index: 16;
    display: grid;
    width: min(100%, 360px);
    max-height: min(520px, calc(100vh - 220px));
    overflow: hidden;
    border: 1px solid var(--border-light);
    border-radius: 14px;
    background: linear-gradient(180deg, var(--bg-titlebar), #17161c);
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.48);
  }

  .branch-picker header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid var(--border);
    padding: 12px 14px;
    background: linear-gradient(180deg, var(--bg-titlebar), var(--bg-card));
  }

  .branch-picker header p,
  .branch-picker header h3 {
    margin: 0;
  }

  .branch-picker header p {
    color: var(--text-secondary);
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .branch-picker header h3 {
    overflow: hidden;
    color: var(--text-bright);
    font-size: 15px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-picker header button {
    width: 32px;
    height: 32px;
    padding: 0;
    border-color: var(--border-light);
    background: var(--bg-card);
    color: var(--accent);
  }

  .branch-picker input {
    height: 42px;
    border: 0;
    border-bottom: 1px solid var(--border);
    padding: 0 14px;
    background: #111016;
    color: var(--text-bright);
    font: inherit;
    outline: none;
  }

  .branch-picker input::placeholder {
    color: var(--text-dim);
  }

  .branch-picker-warning {
    border-bottom: 1px solid var(--border);
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
    color: var(--text-bright);
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch-picker-list small,
  .branch-picker-empty {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 800;
  }

  .branch-picker-empty {
    padding: 18px 10px;
    text-align: center;
  }
</style>
