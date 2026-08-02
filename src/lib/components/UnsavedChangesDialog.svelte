<script lang="ts">
  import { onDestroy, onMount } from "svelte";

  type Props = { onKeepEditing: () => void; onDiscard: () => void };
  let { onKeepEditing, onDiscard }: Props = $props();
  let dialog: HTMLDivElement | null = $state(null);
  let keepButton: HTMLButtonElement | null = $state(null);
  let previousFocus: HTMLElement | null = null;

  onMount(() => {
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    keepButton?.focus();
  });
  onDestroy(() => previousFocus?.focus());

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onKeepEditing();
      return;
    }
    if (event.key !== "Tab" || !dialog) return;
    const focusable = [...dialog.querySelectorAll<HTMLButtonElement>("button:not([disabled])")];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  }
</script>

<div class="confirm-backdrop rules-unsaved-backdrop" role="presentation"></div>
<div
  bind:this={dialog}
  class="confirm-panel rules-unsaved-dialog"
  role="dialog"
  aria-modal="true"
  aria-labelledby="rules-unsaved-title"
  aria-describedby="rules-unsaved-description"
  tabindex="-1"
  onkeydown={handleKeydown}
>
  <header>
    <div>
      <p>Unsaved changes</p>
      <h2 id="rules-unsaved-title">Discard unsaved changes?</h2>
    </div>
  </header>
  <div class="confirm-body">
    <p id="rules-unsaved-description">You have changes to Agent Rules that have not been saved.</p>
  </div>
  <footer>
    <button bind:this={keepButton} type="button" onclick={onKeepEditing}>Keep editing</button>
    <button class="confirm-danger" type="button" onclick={onDiscard}>Discard changes</button>
  </footer>
</div>

<style>
  .rules-unsaved-backdrop,
  .rules-unsaved-dialog {
    z-index: 80;
  }

  .rules-unsaved-dialog footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .rules-unsaved-dialog .confirm-danger {
    border-color: var(--danger-border);
    background: var(--danger-bg);
    color: var(--danger);
  }
</style>
