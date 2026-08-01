<script lang="ts">
  let { id, children }: { id?: string; children: import("svelte").Snippet } = $props();
</script>

<p class="settings-helper" {id}>{@render children()}</p>
