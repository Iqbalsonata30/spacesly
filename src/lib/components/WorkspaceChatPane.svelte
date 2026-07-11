<script lang="ts">
  import type { WorkspaceChatMessage } from "$lib/uiState";

  type Props = {
    title: string;
    onOpenRuntimeSettings: () => void;
    messages: WorkspaceChatMessage[];
    running: boolean;
    onTextareaReady: (element: HTMLTextAreaElement | null) => void;
    onEndReady: (element: HTMLDivElement | null) => void;
    onSubmit: () => void;
    onKeydown: (event: KeyboardEvent) => void;
  };

  let {
    title,
    onOpenRuntimeSettings,
    messages,
    running,
    onTextareaReady,
    onEndReady,
    onSubmit,
    onKeydown,
  }: Props = $props();

  let textarea: HTMLTextAreaElement | null = $state(null);
  let end: HTMLDivElement | null = $state(null);

  $effect(() => {
    onTextareaReady(textarea);
    return () => onTextareaReady(null);
  });

  $effect(() => {
    onEndReady(end);
    return () => onEndReady(null);
  });
</script>

<section class="workspace-chat-pane" aria-label="Agent chat">
  <header>
    <div>
      <p>Agent Chat</p>
      <h2>{title}</h2>
    </div>
    <button type="button" onclick={onOpenRuntimeSettings}>Runtime</button>
  </header>
  <div class="workspace-chat-messages">
    {#each messages as message (message.id)}
      <article class={`workspace-chat-message ${message.role}`}>
        <strong>{message.role}</strong>
        <p>{message.text}</p>
      </article>
    {/each}
    <div class="workspace-chat-end" bind:this={end}></div>
  </div>
  <form
    class="workspace-chat-input"
    onsubmit={(event) => {
      event.preventDefault();
      onSubmit();
    }}
  >
    <textarea
      bind:this={textarea}
      placeholder="Enter sends. Shift+Enter for multiline. Try: queue ABC-123, start agent on ABC-123, sync Jira..."
      disabled={running}
      rows="2"
      onkeydown={onKeydown}
    ></textarea>
    <button type="submit" disabled={running}>{running ? "Working" : "Run"}</button>
  </form>
</section>
