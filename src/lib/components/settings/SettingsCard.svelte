<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    description,
    tone = "default",
    children,
    actions,
  }: {
    title?: string;
    description?: string;
    tone?: "default" | "subtle" | "warning";
    children: Snippet;
    actions?: Snippet;
  } = $props();
</script>

<section class={`settings-card settings-card-${tone}`}>
  {#if title || description}
    <header>
      {#if title}<h5>{title}</h5>{/if}
      {#if description}<p>{description}</p>{/if}
    </header>
  {/if}
  <div class="settings-card-content">{@render children()}</div>
  {#if actions}<footer class="settings-card-actions">{@render actions()}</footer>{/if}
</section>
