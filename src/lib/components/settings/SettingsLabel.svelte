<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    forId,
    text,
    optional = false,
    children,
  }: {
    forId?: string;
    text: string;
    optional?: boolean;
    children: Snippet;
  } = $props();
</script>

<label class="settings-label" for={forId}>
  <span
    >{text}{#if optional}<em>Optional</em>{/if}</span
  >
  {@render children()}
</label>
