<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, description, children }: { title: string; description?: string; children: Snippet } =
    $props();
</script>

<section class="settings-section-block">
  <header>
    <h4>{title}</h4>
    {#if description}<p>{description}</p>{/if}
  </header>
  <div class="settings-section-content">{@render children()}</div>
</section>
