<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    description,
    children,
  }: { title?: string; description?: string; children: Snippet } = $props();
</script>

<div class="settings-group">
  {#if title || description}
    <div class="settings-group-heading">
      {#if title}<strong>{title}</strong>{/if}
      {#if description}<span>{description}</span>{/if}
    </div>
  {/if}
  <div class="settings-group-content">{@render children()}</div>
</div>
