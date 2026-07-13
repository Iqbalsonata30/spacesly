<script lang="ts">
  type AppNotice = {
    tone: "info" | "success" | "error";
    message: string;
  };

  type Props = {
    notice: AppNotice | null;
    syncError: string | null;
    onDismissNotice: () => void;
  };

  let { notice, syncError, onDismissNotice }: Props = $props();
</script>

{#if notice || syncError}
  <div class="notification-stack" aria-live="polite">
    {#if notice}
      <div class={`notification-card app-notice ${notice.tone}`} role="status">
        <div>
          <strong>{notice.tone === "error" ? "Action failed" : notice.tone === "success" ? "Success" : "Notice"}</strong>
          <span>{notice.message}</span>
        </div>
        <button type="button" aria-label="Dismiss notification" onclick={onDismissNotice}>×</button>
      </div>
    {:else if syncError}
      <div class="notification-card sync-error" role="status">
        <div>
          <strong>Jira sync failed</strong>
          <span>{syncError}</span>
        </div>
      </div>
    {/if}
  </div>
{/if}
