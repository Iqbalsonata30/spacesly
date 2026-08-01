<script lang="ts">
  let { wide = false, children }: { wide?: boolean; children: import("svelte").Snippet } = $props();
</script>

<div class:wide class="settings-input">{@render children()}</div>
