<script lang="ts">
  import type { AiEditProposal } from "$lib/aiEdit";

  type Props = {
    proposal: AiEditProposal | null;
    generating: boolean;
    stale: boolean;
    selectedHunkIds: string[];
    error: string | null;
    onRequest: (instruction: string) => void;
    onCancel: () => void;
    onToggleHunk: (id: string) => void;
    onApplySelected: () => void;
    onAcceptAll: () => void;
    onReject: () => void;
    contextDocuments: Array<{
      id: string;
      path: string;
      pinned: boolean;
      characters: number;
    }>;
    contextCharacters: number;
    diagnosticCount: number;
    onToggleContext: (documentId: string) => void;
  };

  let {
    proposal,
    generating,
    stale,
    selectedHunkIds,
    error,
    onRequest,
    onCancel,
    onToggleHunk,
    onApplySelected,
    onAcceptAll,
    onReject,
    contextDocuments,
    contextCharacters,
    diagnosticCount,
    onToggleContext,
  }: Props = $props();
  let instruction = $state("");

  function submit() {
    const value = instruction.trim();
    if (!value || generating) return;
    onRequest(value);
  }
</script>

<section class="ai-edit-review" aria-label="AI edit review">
  <form
    onsubmit={(event) => {
      event.preventDefault();
      submit();
    }}
  >
    <label for="ai-edit-instruction">AI edit</label>
    <input
      id="ai-edit-instruction"
      bind:value={instruction}
      placeholder="Describe a focused change to this file"
      disabled={generating}
    />
    {#if generating}
      <button type="button" class="secondary" onclick={onCancel}>Cancel</button>
    {:else}
      <button type="submit" disabled={!instruction.trim()}>Generate</button>
    {/if}
  </form>

  <details class="ai-context">
    <summary>
      <span>Context</span>
      <small
        >~{contextCharacters.toLocaleString()} / 524,288 characters · {diagnosticCount}
        diagnostics</small
      >
    </summary>
    <p>The current selection and diagnostics are included automatically when you generate.</p>
    <div>
      {#each contextDocuments as document (document.id)}
        <button
          type="button"
          class:pinned={document.pinned}
          aria-pressed={document.pinned}
          onclick={() => onToggleContext(document.id)}
        >
          <span>{document.path}</span>
          <small
            >{document.pinned ? "Pinned" : `${document.characters.toLocaleString()} chars`}</small
          >
        </button>
      {:else}
        <p>Open another file to make it available as context.</p>
      {/each}
    </div>
  </details>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if proposal}
    <header>
      <div>
        <strong>{proposal.summary}</strong>
        <span>{proposal.hunks.length} change {proposal.hunks.length === 1 ? "hunk" : "hunks"}</span>
      </div>
      {#if stale}<span class="stale">Document changed. Regenerate before applying.</span>{/if}
    </header>

    <div class="hunks">
      {#each proposal.hunks as hunk, index (hunk.id)}
        {@const selected = selectedHunkIds.includes(hunk.id)}
        <article class:selected>
          <button
            type="button"
            class="hunk-toggle"
            aria-pressed={selected}
            onclick={() => onToggleHunk(hunk.id)}
          >
            <span>{selected ? "Included" : "Excluded"}</span>
            <small>Hunk {index + 1} · line {hunk.oldStart + 1}</small>
          </button>
          <pre
            aria-label={`Diff hunk ${index + 1}`}>{#each hunk.oldLines.slice(0, 160) as line, lineIndex (`old-${lineIndex}`)}<span
                class="removed">- {line}</span
              >{/each}{#each hunk.newLines.slice(0, 160) as line, lineIndex (`new-${lineIndex}`)}<span
                class="added">+ {line}</span
              >{/each}{#if hunk.oldLines.length > 160 || hunk.newLines.length > 160}<span
                class="truncated">… diff preview truncated</span
              >{/if}</pre>
        </article>
      {/each}
    </div>

    <footer>
      <button type="button" class="secondary" onclick={onReject}>Reject</button>
      <button
        type="button"
        class="secondary"
        disabled={stale || selectedHunkIds.length === 0}
        onclick={onApplySelected}>Apply selected</button
      >
      <button type="button" disabled={stale} onclick={onAcceptAll}>Accept all</button>
    </footer>
  {/if}
</section>

<style>
  .ai-edit-review {
    display: grid;
    gap: 10px;
    max-height: 44%;
    overflow: auto;
    padding: 10px 12px;
    color: var(--text-primary);
    background: color-mix(in srgb, var(--bg-card) 92%, transparent);
    border-top: 1px solid var(--border-light);
  }

  .ai-context {
    border: 1px solid var(--border-light);
    border-radius: 6px;
    background: var(--bg-base);
  }

  .ai-context summary,
  .ai-context button {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .ai-context summary {
    padding: 7px 9px;
    cursor: pointer;
    font-size: 11px;
    font-weight: 750;
  }

  .ai-context summary small,
  .ai-context button small {
    color: var(--text-muted);
    font-weight: 500;
  }

  .ai-context > p {
    margin: 0;
    padding: 0 9px 7px;
    color: var(--text-muted);
    font-size: 10px;
  }

  .ai-context > div {
    display: grid;
    max-height: 150px;
    overflow: auto;
    border-top: 1px solid var(--border-light);
  }

  .ai-context button {
    border: 0;
    border-radius: 0;
    padding: 6px 9px;
    color: var(--text-secondary);
    background: transparent;
    text-align: left;
  }

  .ai-context button:hover,
  .ai-context button:focus-visible,
  .ai-context button.pinned {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .ai-context button span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  form,
  header,
  footer,
  .hunk-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  form label {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 800;
    text-transform: uppercase;
  }

  input {
    min-width: 0;
    flex: 1;
    border: 1px solid var(--border-light);
    border-radius: 6px;
    padding: 8px 10px;
    color: inherit;
    background: var(--bg-base);
  }

  button {
    border: 1px solid color-mix(in srgb, var(--accent) 55%, var(--border-light));
    border-radius: 6px;
    padding: 7px 10px;
    color: var(--bg-base);
    background: var(--accent);
    font-weight: 750;
    cursor: pointer;
  }

  button.secondary,
  .hunk-toggle {
    color: var(--text-primary);
    background: var(--bg-base);
    border-color: var(--border-light);
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  header {
    justify-content: space-between;
    align-items: flex-start;
  }

  header div {
    display: grid;
    gap: 2px;
  }

  header span,
  small {
    color: var(--text-muted);
    font-size: 11px;
  }

  .stale,
  .error {
    color: var(--error);
  }

  .hunks {
    display: grid;
    gap: 8px;
  }

  article {
    overflow: hidden;
    border: 1px solid var(--border-light);
    border-radius: 7px;
    opacity: 0.55;
  }

  article.selected {
    opacity: 1;
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border-light));
  }

  .hunk-toggle {
    width: 100%;
    justify-content: space-between;
    border: 0;
    border-bottom: 1px solid var(--border-light);
    border-radius: 0;
  }

  pre {
    display: grid;
    max-height: 220px;
    overflow: auto;
    margin: 0;
    padding: 6px 0;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
  }

  pre span {
    display: block;
    padding: 0 10px;
    white-space: pre-wrap;
  }

  .removed {
    color: var(--diff-del);
    background: var(--diff-del-bg);
  }

  .added {
    color: var(--diff-add);
    background: var(--diff-add-bg);
  }

  .truncated {
    color: var(--text-muted);
  }

  footer {
    justify-content: flex-end;
  }

  @media (max-width: 700px) {
    form {
      flex-wrap: wrap;
    }

    form label {
      width: 100%;
    }

    input {
      flex-basis: 70%;
    }
  }
</style>
