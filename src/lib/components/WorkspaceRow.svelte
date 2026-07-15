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
  }: Props = $props();

  function truncateMiddle(value: string, max = 34) {
    if (value.length <= max) return value;
    const keep = Math.max(8, Math.floor((max - 1) / 2));
    return `${value.slice(0, keep)}…${value.slice(-keep)}`;
  }
</script>

<button
  type="button"
  class:active
  class="workspace-row"
  style={`--row-depth: ${depth};`}
  title={title ?? label}
  {disabled}
  onclick={() => onClick?.()}
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
    min-width: 0;
    border: 0;
    border-radius: 0;
    padding: 10px 12px 10px calc(12px + (var(--row-depth) * 16px));
    background: transparent;
    color: #f1edf5;
    text-align: left;
  }

  .workspace-row::after {
    content: "";
    position: absolute;
    inset-inline: 0;
    inset-block-end: 0;
    height: 1px;
    background: #1f1e27;
    pointer-events: none;
  }

  .workspace-row:hover:not(:disabled),
  .workspace-row:focus-visible:not(:disabled),
  .workspace-row.active {
    background: #20262d;
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
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-align: right;
    text-transform: uppercase;
  }

  .workspace-row-status.modified {
    color: #f0b05f;
  }

  .workspace-row-status.added {
    color: #7bc67b;
  }

  .workspace-row-status.deleted {
    color: #d87a7a;
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
