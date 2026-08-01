<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { initTheme } from "$lib/stores/theme.svelte";

  let { children } = $props();

  onMount(() => {
    const destroyThemeManager = initTheme();
    function suppress(e: ErrorEvent) {
      if (e.message?.startsWith("ResizeObserver loop")) {
        e.stopImmediatePropagation();
      }
    }

    window.addEventListener("error", suppress);
    return () => {
      destroyThemeManager();
      window.removeEventListener("error", suppress);
    };
  });
</script>

{@render children()}
