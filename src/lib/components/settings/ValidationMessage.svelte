<script lang="ts">
  let { error = null, success = null }: { error?: string | null; success?: string | null } =
    $props();
</script>

<div class="settings-validation" aria-live="polite" aria-atomic="true">
  {#if error}
    <p class="settings-validation-error" role="alert">{error}</p>
  {:else if success}
    <p class="settings-validation-success" role="status">{success}</p>
  {:else}
    <span aria-hidden="true"></span>
  {/if}
</div>
