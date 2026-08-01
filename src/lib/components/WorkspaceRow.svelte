<script lang="ts">
  import type { Snippet } from "svelte";

  type Props = {
    label: string;
    title?: string;
    depth?: number;
    active?: boolean;
    disabled?: boolean;
    status?: string;
    statusTone?: "neutral" | "modified" | "added" | "deleted";
    leading?: Snippet;
    trailing?: Snippet;
    onClick?: () => void;
    treeItem?: boolean;
    tabIndex?: number;
    ariaLevel?: number;
    ariaExpanded?: boolean;
    ariaSelected?: boolean;
    treePath?: string;
    onKeydown?: (event: KeyboardEvent) => void;
    onFocus?: () => void;
  };

  let {
    label,
    title,
    depth = 0,
    active = false,
    disabled = false,
    status,
    statusTone = "neutral",
    leading,
    trailing,
    onClick,
    treeItem = false,
    tabIndex = 0,
    ariaLevel,
    ariaExpanded,
    ariaSelected,
    treePath,
    onKeydown,
    onFocus,
  }: Props = $props();

  function truncateMiddle(value: string, max = 34) {
    if (value.length <= max) return value;
    const keep = Math.max(8, Math.floor((max - 1) / 2));
    return `${value.slice(0, keep)}…${value.slice(-keep)}`;
  }
</script>

<button
  type="button"
  role={treeItem ? "treeitem" : undefined}
  aria-level={treeItem ? ariaLevel : undefined}
  aria-expanded={treeItem ? ariaExpanded : undefined}
  aria-selected={treeItem ? ariaSelected : undefined}
  tabindex={tabIndex}
  data-tree-path={treePath}
  class:active
  class="workspace-row"
  style={`--row-depth: ${depth};`}
  title={title ?? label}
  {disabled}
  onclick={() => onClick?.()}
  onkeydown={(event) => onKeydown?.(event)}
  onfocus={() => onFocus?.()}
>
  {#if leading}
    <span class="workspace-row-leading">{@render leading()}</span>
  {/if}
  <span class="workspace-row-label">{truncateMiddle(label)}</span>
  {#if status}
    <span class={`workspace-row-status ${statusTone}`}>{status}</span>
  {/if}
  {#if trailing}
    <span class="workspace-row-trailing">{@render trailing()}</span>
  {/if}
</button>

<style>
  .workspace-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    box-sizing: border-box;
    content-visibility: auto;
    contain-intrinsic-size: 36px;
    min-width: 0;
    border: 0;
    border-radius: 0;
    padding: 10px 12px 10px calc(12px + (var(--row-depth) * 16px));
    background: transparent;
    color: var(--text-bright);
    text-align: left;
  }

  .workspace-row::after {
    content: "";
    position: absolute;
    inset-inline: 0;
    inset-block-end: 0;
    height: 1px;
    background: var(--border-subtle);
    pointer-events: none;
  }

  .workspace-row:hover:not(:disabled),
  .workspace-row:focus-visible:not(:disabled),
  .workspace-row.active {
    background: var(--surface-selected);
  }

  .workspace-row:disabled {
    opacity: 0.7;
    cursor: default;
  }

  .workspace-row-leading,
  .workspace-row-trailing {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
    min-width: 0;
  }

  .workspace-row-status {
    flex: 0 0 auto;
    min-width: 18px;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-align: right;
    text-transform: uppercase;
  }

  .workspace-row-status.modified {
    color: var(--warning);
  }

  .workspace-row-status.added {
    color: var(--diff-add);
  }

  .workspace-row-status.deleted {
    color: var(--diff-del);
  }

  .workspace-row-label {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 800;
  }
</style>
