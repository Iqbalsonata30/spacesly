<script lang="ts">
  import type { WorkspaceChatMessage } from "$lib/uiState";
  import type { WorkspaceChatSession } from "$lib/uiState";

  type Props = {
    title: string;
    onOpenRuntimeSettings: () => void;
    sessions: WorkspaceChatSession[];
    activeSessionId: string;
    onNewSession: () => void;
    onSwitchSession: (sessionId: string) => void;
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
    sessions,
    activeSessionId,
    onNewSession,
    onSwitchSession,
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
    <div class="workspace-chat-actions">
      <button type="button" onclick={onNewSession}>New session</button>
      <button type="button" onclick={onOpenRuntimeSettings}>Runtime</button>
    </div>
  </header>
  <div class="workspace-chat-sessions" aria-label="Chat sessions">
    {#each sessions as session (session.id)}
      <button
        type="button"
        class:active={session.id === activeSessionId}
        title={`Updated ${new Date(session.updatedAt).toLocaleString()}`}
        onclick={() => onSwitchSession(session.id)}
      >
        <strong>{session.title}</strong>
        <span>{session.messages.length} msg{session.messages.length === 1 ? "" : "s"}</span>
      </button>
    {/each}
  </div>
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

<style>
  .workspace-chat-actions,
  .workspace-chat-sessions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .workspace-chat-sessions {
    margin: 0 0 8px;
  }

  .workspace-chat-sessions button {
    display: grid;
    gap: 2px;
    min-width: 0;
    padding: 8px 10px;
    border: 1px solid #262432;
    border-radius: 10px;
    background: #111017;
    color: #e8e2ef;
    text-align: left;
  }

  .workspace-chat-sessions button.active {
    border-color: #6f63c9;
    background: rgba(111, 99, 201, 0.18);
  }

  .workspace-chat-sessions strong {
    font-size: 0.78rem;
    font-weight: 600;
  }

  .workspace-chat-sessions span {
    font-size: 0.68rem;
    opacity: 0.72;
  }
</style>
