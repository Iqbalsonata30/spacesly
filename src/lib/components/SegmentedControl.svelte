<script lang="ts">
  export type SegmentedControlItem = {
    value: string;
    label: string;
    badge?: string | number;
  };

  type Props = {
    items: SegmentedControlItem[];
    activeValue: string;
    ariaLabel: string;
    onSelect: (value: string) => void;
  };

  let { items, activeValue, ariaLabel, onSelect }: Props = $props();

  let activeIndex = $derived(
    Math.max(
      0,
      items.findIndex((item) => item.value === activeValue),
    ),
  );
  let segmentCount = $derived(Math.max(1, items.length));

  function selectIndex(index: number) {
    const item = items[index];
    if (item) onSelect(item.value);
  }

  function handleKeydown(event: KeyboardEvent, index: number) {
    if (items.length === 0) return;

    let nextIndex = index;
    if (event.key === "ArrowLeft") {
      nextIndex = (index - 1 + items.length) % items.length;
    } else if (event.key === "ArrowRight") {
      nextIndex = (index + 1) % items.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = items.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    selectIndex(nextIndex);
  }
</script>

<div
  class="segmented-control"
  role="tablist"
  aria-orientation="horizontal"
  aria-label={ariaLabel}
  style={`--segment-count: ${segmentCount}; --segment-index: ${activeIndex};`}
>
  <div class="segmented-indicator" aria-hidden="true"></div>
  {#each items as item, index (item.value)}
    <button
      type="button"
      role="tab"
      class:active={item.value === activeValue}
      aria-selected={item.value === activeValue}
      tabindex={item.value === activeValue ? 0 : -1}
      onclick={() => onSelect(item.value)}
      onkeydown={(event) => handleKeydown(event, index)}
    >
      <span class="segmented-label">{item.label}</span>
      {#if item.badge !== undefined}
        <span class="segmented-badge">{item.badge}</span>
      {/if}
    </button>
  {/each}
</div>

<style>
  .segmented-control {
    position: relative;
    display: grid;
    grid-template-columns: repeat(var(--segment-count), minmax(0, 1fr));
    gap: 0;
    width: 100%;
    box-sizing: border-box;
    min-height: 48px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: 15px;
    background: linear-gradient(180deg, var(--bg-titlebar), var(--bg-card));
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.02);
    overflow: hidden;
  }

  .segmented-indicator {
    position: absolute;
    inset: 5px;
    width: calc((100% - 10px) / var(--segment-count));
    border-radius: 11px;
    background: linear-gradient(180deg, #2a3038, #20262d);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.05);
    transform: translateX(calc(var(--segment-index) * 100%));
    transition:
      transform 200ms ease,
      background-color 200ms ease,
      box-shadow 200ms ease,
      opacity 200ms ease;
    pointer-events: none;
  }

  button {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 38px;
    min-width: 0;
    border: 0;
    border-radius: 11px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 12px;
    font-weight: 850;
    letter-spacing: 0.01em;
    transition:
      color 180ms ease,
      background-color 180ms ease,
      box-shadow 180ms ease,
      transform 180ms ease;
  }

  button:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.03);
    color: var(--text-bright);
  }

  button.active {
    color: var(--text-bright);
    background: rgba(255, 255, 255, 0.025);
  }

  button:focus-visible {
    outline: none;
    box-shadow:
      inset 0 0 0 1px rgba(184, 214, 228, 0.25),
      0 0 0 3px rgba(184, 214, 228, 0.18);
  }

  .segmented-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .segmented-badge {
    display: inline-grid;
    place-items: center;
    min-width: 24px;
    flex: 0 0 auto;
    height: 20px;
    padding: 0 7px;
    border-radius: 999px;
    background: #22344a;
    color: #b8d6e4;
    font-size: 11px;
    font-weight: 900;
    line-height: 1;
  }
</style>
