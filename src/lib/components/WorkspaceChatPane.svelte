<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { scale } from "svelte/transition";
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
  let sessionPickerOpen = $state(false);
  let sessionPickerRoot: HTMLDivElement | null = $state(null);
  let sessionSearchInput: HTMLInputElement | null = $state(null);
  let sessionFilter = $state("");
  let highlightedSessionId = $state<string | null>(null);
  let activeSession = $derived(sessions.find((session) => session.id === activeSessionId) ?? null);
  let filteredSessions = $derived(
    sessions.filter((session) => {
      const query = sessionFilter.trim().toLowerCase();
      if (!query) return true;
      return [
        session.title,
        new Date(session.updatedAt).toLocaleString(),
        `${session.messages.length}`,
      ]
        .join(" ")
        .toLowerCase()
        .includes(query);
    }),
  );
  let highlightedSession = $derived(
    filteredSessions.find((session) => session.id === highlightedSessionId) ??
      filteredSessions[0] ??
      null,
  );

  $effect(() => {
    onTextareaReady(textarea);
    return () => onTextareaReady(null);
  });

  $effect(() => {
    onEndReady(end);
    return () => onEndReady(null);
  });

  $effect(() => {
    if (!sessionPickerOpen) return;

    if (filteredSessions.length === 0) {
      highlightedSessionId = null;
      return;
    }

    if (
      !highlightedSessionId ||
      !filteredSessions.some((session) => session.id === highlightedSessionId)
    ) {
      highlightedSessionId =
        activeSessionId && filteredSessions.some((session) => session.id === activeSessionId)
          ? activeSessionId
          : filteredSessions[0].id;
    }
  });

  $effect(() => {
    if (!sessionPickerOpen || !highlightedSessionId) return;

    void tick().then(() => {
      const element = sessionPickerRoot?.querySelector<HTMLElement>(
        `[data-session-id="${highlightedSessionId}"]`,
      );
      element?.scrollIntoView({ block: "nearest" });
    });
  });

  $effect(() => {
    if (!sessionPickerOpen) return;

    const closeOnInteractOutside = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (sessionPickerRoot && target && !sessionPickerRoot.contains(target)) {
        sessionPickerOpen = false;
      }
    };

    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        sessionPickerOpen = false;
      }
    };

    document.addEventListener("pointerdown", closeOnInteractOutside, true);
    document.addEventListener("keydown", closeOnEscape);

    return () => {
      document.removeEventListener("pointerdown", closeOnInteractOutside, true);
      document.removeEventListener("keydown", closeOnEscape);
    };
  });

  onDestroy(() => {
    sessionPickerOpen = false;
  });

  function openSessionPicker() {
    sessionPickerOpen = !sessionPickerOpen;
    if (sessionPickerOpen) {
      sessionFilter = "";
      highlightedSessionId = activeSessionId;
      void tick().then(() => sessionSearchInput?.focus());
    }
  }

  function selectSession(sessionId: string) {
    onSwitchSession(sessionId);
    sessionPickerOpen = false;
  }

  function moveSessionHighlight(offset: number) {
    if (filteredSessions.length === 0) return;

    const currentIndex = filteredSessions.findIndex(
      (session) => session.id === highlightedSessionId,
    );
    const nextIndex =
      currentIndex < 0
        ? 0
        : Math.max(0, Math.min(filteredSessions.length - 1, currentIndex + offset));
    highlightedSessionId = filteredSessions[nextIndex]?.id ?? null;
  }

  function sessionPickerKeydown(event: KeyboardEvent) {
    if (!sessionPickerOpen) return;

    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveSessionHighlight(1);
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      moveSessionHighlight(-1);
      return;
    }

    if (event.key === "Home") {
      event.preventDefault();
      highlightedSessionId = filteredSessions[0]?.id ?? null;
      return;
    }

    if (event.key === "End") {
      event.preventDefault();
      highlightedSessionId = filteredSessions.at(-1)?.id ?? null;
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      if (highlightedSessionId) selectSession(highlightedSessionId);
    }
  }
</script>

<section class="workspace-chat-pane" aria-label="Agent chat">
  <header>
    <div>
      <p>Agent Chat</p>
      <h2>{title}</h2>
    </div>
    <div class="workspace-chat-actions">
      <button type="button" onclick={onNewSession}>New session</button>
      <div class="workspace-chat-session-picker" bind:this={sessionPickerRoot}>
        <button
          type="button"
          class:active={sessionPickerOpen}
          aria-haspopup="menu"
          aria-expanded={sessionPickerOpen}
          onclick={openSessionPicker}
        >
          Sessions
        </button>
        {#if sessionPickerOpen}
          <div
            class="workspace-chat-session-menu"
            role="menu"
            aria-label="Available sessions"
            transition:scale={{ duration: 120, start: 0.98, opacity: 0.4 }}
          >
            <header>
              <div>
                <p>Switch session</p>
                <strong>{activeSession?.title ?? "No active session"}</strong>
              </div>
              <span>{filteredSessions.length} of {sessions.length}</span>
            </header>
            <label class="workspace-chat-session-search">
              <span>Search</span>
              <input
                bind:this={sessionSearchInput}
                type="search"
                placeholder="Filter sessions"
                autocomplete="off"
                spellcheck="false"
                value={sessionFilter}
                oninput={(event) => {
                  sessionFilter = (event.currentTarget as HTMLInputElement).value;
                }}
                onkeydown={sessionPickerKeydown}
              />
            </label>
            <div class="workspace-chat-session-list">
              {#if sessions.length === 0}
                <div class="workspace-chat-session-empty">
                  <strong>No sessions yet</strong>
                  <p>Create a new one to start a conversation.</p>
                </div>
              {:else if filteredSessions.length === 0}
                <div class="workspace-chat-session-empty">
                  <strong>No matches</strong>
                  <p>Try a different session title or timestamp.</p>
                </div>
              {:else}
                {#each filteredSessions as session (session.id)}
                  <button
                    type="button"
                    role="menuitemradio"
                    aria-checked={session.id === activeSessionId}
                    class:active={session.id === activeSessionId}
                    class:selected={session.id === highlightedSession?.id}
                    data-session-id={session.id}
                    onmouseenter={() => {
                      highlightedSessionId = session.id;
                    }}
                    onclick={() => selectSession(session.id)}
                  >
                    <div class="workspace-chat-session-main">
                      <strong>{session.title}</strong>
                      <span
                        >{session.messages.length} message{session.messages.length === 1
                          ? ""
                          : "s"}</span
                      >
                    </div>
                    <div class="workspace-chat-session-meta">
                      <span>{new Date(session.updatedAt).toLocaleString()}</span>
                      {#if session.id === activeSessionId}
                        <em>Active</em>
                      {/if}
                    </div>
                  </button>
                {/each}
              {/if}
            </div>
          </div>
        {/if}
      </div>
      <button type="button" onclick={onOpenRuntimeSettings}>Runtime</button>
    </div>
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
      onkeydown={onKeydown}></textarea>
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

  .workspace-chat-session-picker {
    position: relative;
  }

  .workspace-chat-sessions {
    margin: 0 0 8px;
  }

  .workspace-chat-actions > button,
  .workspace-chat-session-picker > button {
    display: grid;
    min-width: 112px;
    gap: 2px;
    align-items: center;
    justify-items: center;
    min-height: 40px;
    padding: 8px 14px;
    line-height: 1.1;
    text-align: center;
    white-space: nowrap;
  }

  .workspace-chat-session-menu {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 40;
    display: grid;
    gap: 12px;
    width: min(560px, calc(100vw - 24px));
    min-width: 320px;
    max-height: min(72vh, 560px);
    overflow: hidden;
    padding: 14px;
    border: 1px solid #2a2835;
    border-radius: 16px;
    background: linear-gradient(180deg, rgba(23, 22, 30, 0.98), rgba(16, 15, 22, 0.98));
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.45);
    backdrop-filter: blur(12px);
  }

  .workspace-chat-session-menu > header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .workspace-chat-session-menu > header div {
    min-width: 0;
  }

  .workspace-chat-session-menu > header p {
    margin: 0;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .workspace-chat-session-menu > header strong,
  .workspace-chat-session-menu > header span {
    display: block;
    margin-top: 4px;
    color: #e8e2ef;
    font-size: 13px;
    line-height: 1.35;
  }

  .workspace-chat-session-menu > header span {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 700;
    white-space: nowrap;
  }

  .workspace-chat-session-search {
    display: grid;
    gap: 6px;
  }

  .workspace-chat-session-search span {
    color: #8f88a8;
    font-size: 10px;
    font-weight: 800;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .workspace-chat-session-search input {
    width: 100%;
    min-width: 0;
    border: 1px solid #2c2a36;
    border-radius: 12px;
    padding: 10px 12px;
    background: #0f0e15;
    color: #f1edf5;
    font: inherit;
    outline: none;
    transition:
      border-color 120ms ease,
      box-shadow 120ms ease,
      background 120ms ease;
  }

  .workspace-chat-session-search input::placeholder {
    color: #6f6884;
  }

  .workspace-chat-session-search input:focus {
    border-color: #6f63c9;
    background: #12111a;
    box-shadow: 0 0 0 3px rgba(111, 99, 201, 0.18);
  }

  .workspace-chat-session-list {
    display: grid;
    gap: 10px;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
    padding-right: 2px;
    max-height: min(52vh, 420px);
    scroll-behavior: smooth;
    -webkit-overflow-scrolling: touch;
  }

  .workspace-chat-session-empty {
    display: grid;
    gap: 4px;
    padding: 16px;
    border: 1px dashed #2f2b3b;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.02);
  }

  .workspace-chat-session-empty strong {
    color: #f1edf5;
    font-size: 13px;
    font-weight: 700;
  }

  .workspace-chat-session-empty p {
    margin: 0;
    color: #8f88a8;
    font-size: 12px;
    line-height: 1.45;
  }

  .workspace-chat-session-list button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    min-width: 0;
    padding: 12px 13px;
    border: 1px solid #262432;
    border-radius: 14px;
    background: #111017;
    color: #e8e2ef;
    text-align: left;
    transition:
      border-color 120ms ease,
      background 120ms ease,
      transform 120ms ease,
      box-shadow 120ms ease;
  }

  .workspace-chat-session-list button:hover,
  .workspace-chat-session-list button.selected {
    border-color: #6f63c9;
    background: rgba(111, 99, 201, 0.18);
    box-shadow: 0 0 0 1px rgba(111, 99, 201, 0.18);
  }

  .workspace-chat-session-list button.active {
    border-color: rgba(111, 99, 201, 0.9);
  }

  .workspace-chat-session-list button.selected {
    transform: translateY(-1px);
  }

  .workspace-chat-session-main,
  .workspace-chat-session-meta {
    display: grid;
    gap: 3px;
    min-width: 0;
  }

  .workspace-chat-session-main strong {
    overflow: hidden;
    font-size: 0.88rem;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .workspace-chat-session-main span,
  .workspace-chat-session-meta span,
  .workspace-chat-session-meta em {
    font-size: 0.72rem;
    line-height: 1.35;
  }

  .workspace-chat-session-main span,
  .workspace-chat-session-meta span {
    color: #a79fc0;
  }

  .workspace-chat-session-meta {
    justify-items: end;
    text-align: right;
  }

  .workspace-chat-session-meta em {
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(111, 99, 201, 0.18);
    color: #d6d0ff;
    font-style: normal;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  @media (max-width: 700px) {
    .workspace-chat-actions {
      width: 100%;
      justify-content: stretch;
    }

    .workspace-chat-actions > button,
    .workspace-chat-session-picker > button {
      flex: 1 1 0;
      min-width: 0;
    }

    .workspace-chat-session-menu {
      right: 0;
      left: 0;
      width: auto;
      min-width: 0;
    }

    .workspace-chat-session-list button {
      align-items: flex-start;
      flex-direction: column;
    }

    .workspace-chat-session-meta {
      justify-items: start;
      text-align: left;
    }
  }
</style>
