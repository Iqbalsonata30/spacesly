<script lang="ts">
  import "../app.css";
  import {onMount} from "svelte";
  import { initTheme } from "$lib/stores/theme.svelte"

  let { children } = $props();

  initTheme();

  onMount(() => {
    function suppress(e: ErrorEvent){
      if(e.message?.startsWith("ResizeObserver loop")){
        e.stopImmediatePropagation();
      }
    }

    window.addEventListener("error",suppress);
    return () => window.removeEventListener("error",suppress);
  })
</script>


{@render children()}
