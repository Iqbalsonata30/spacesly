<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    columns = 2,
    align = "start",
    children,
  }: {
    columns?: 1 | 2 | 3;
    align?: "start" | "end" | "center";
    children: Snippet;
  } = $props();
</script>

<div class="settings-row" data-columns={columns} data-align={align}>{@render children()}</div>
