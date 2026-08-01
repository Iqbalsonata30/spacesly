<script lang="ts">
  let { children }: { children: import("svelte").Snippet } = $props();
</script>

<p class="settings-description">{@render children()}</p>
