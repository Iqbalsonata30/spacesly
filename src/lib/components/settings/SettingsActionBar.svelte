<script lang="ts">
  let { children }: { children: import("svelte").Snippet } = $props();
</script>

<footer class="settings-action-bar">{@render children()}</footer>
