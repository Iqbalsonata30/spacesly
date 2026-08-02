<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    id,
    eyebrow,
    title,
    description,
    badge,
    status,
    children,
  }: {
    id: string;
    eyebrow: string;
    title: string;
    description: string;
    badge?: string;
    status?: Snippet;
    children: Snippet;
  } = $props();
</script>

<div class="settings-page" role="tabpanel" aria-labelledby={`${id}-tab`} id={`${id}-panel`}>
  <header class="settings-page-header">
    <div>
      <p class="settings-page-eyebrow">{eyebrow}</p>
      <h3>{title}</h3>
      <p>{description}</p>
    </div>
    {#if status}{@render status()}{:else if badge}<strong class="settings-page-badge"
        >{badge}</strong
      >{/if}
  </header>
  <div class="settings-page-body">{@render children()}</div>
</div>
