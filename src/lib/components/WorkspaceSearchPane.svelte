<script lang="ts">
  import { onMount } from "svelte";
  import { Replace } from "lucide-svelte";
  import type { WorkspaceReplacePreviewResponse, WorkspaceSearchResult } from "$lib/ipc";

  type Props = {
    query: string;
    caseSensitive: boolean;
    results: WorkspaceSearchResult[];
    loading: boolean;
    error: string | null;
    filesSearched: number;
    truncated: boolean;
    replaceOpen: boolean;
    replacement: string;
    replacePreview: WorkspaceReplacePreviewResponse | null;
    replaceLoading: boolean;
    replaceApplying: boolean;
    replaceError: string | null;
    onQueryChange: (query: string) => void;
    onCaseSensitiveChange: (enabled: boolean) => void;
    onOpenResult: (result: WorkspaceSearchResult) => void;
    onToggleReplace: () => void;
    onReplacementChange: (replacement: string) => void;
    onPreviewReplace: () => void;
    onApplyReplace: () => void;
  };

  let {
    query,
    caseSensitive,
    results,
    loading,
    error,
    filesSearched,
    truncated,
    replaceOpen,
    replacement,
    replacePreview,
    replaceLoading,
    replaceApplying,
    replaceError,
    onQueryChange,
    onCaseSensitiveChange,
    onOpenResult,
    onToggleReplace,
    onReplacementChange,
    onPreviewReplace,
    onApplyReplace,
  }: Props = $props();
  let searchInput: HTMLInputElement | null = $state(null);

  onMount(() => requestAnimationFrame(() => searchInput?.focus()));
</script>

<aside class="workspace-search-pane" aria-label="Workspace search">
  <header>
    <div>
      <p>Search</p>
      <h2>Workspace content</h2>
    </div>
  </header>

  <div class="search-controls">
    <label>
      <input
        bind:this={searchInput}
        value={query}
        placeholder="Search text across files"
        aria-label="Search workspace content"
        oninput={(event) => onQueryChange(event.currentTarget.value)}
      />
    </label>
    <button
      type="button"
      class:active={caseSensitive}
      aria-pressed={caseSensitive}
      aria-label="Match case"
      title="Match case"
      onclick={() => onCaseSensitiveChange(!caseSensitive)}>Aa</button
    >
    <button
      type="button"
      class:active={replaceOpen}
      aria-pressed={replaceOpen}
      aria-label="Toggle replace"
      title="Toggle replace"
      onclick={onToggleReplace}><Replace size={14} aria-hidden="true" /></button
    >
  </div>

  {#if replaceOpen}
    <div class="replace-controls">
      <input
        value={replacement}
        placeholder="Replace with"
        aria-label="Replacement text"
        oninput={(event) => onReplacementChange(event.currentTarget.value)}
      />
      <button
        type="button"
        disabled={query.trim().length < 2 || replaceLoading || replaceApplying}
        onclick={onPreviewReplace}>{replaceLoading ? "Previewing" : "Preview"}</button
      >
    </div>
  {/if}

  <div class="search-status" role="status">
    {#if loading}
      Searching…
    {:else if error}
      <span class="error">{error}</span>
    {:else if query.trim().length < 2}
      Enter at least two characters.
    {:else}
      {results.length} matches in {filesSearched} files{truncated ? " · result limit reached" : ""}
    {/if}
  </div>

  <div class="search-results" aria-label="Workspace search results">
    {#each results as result, index (`${result.file_path}:${result.line}:${result.character}:${index}`)}
      <button type="button" onclick={() => onOpenResult(result)}>
        <span class="result-path">{result.file_path}</span>
        <span class="result-preview">{result.preview || "(empty line)"}</span>
        <small>{result.line + 1}:{result.character + 1}</small>
      </button>
    {:else}
      {#if !loading && query.trim().length >= 2 && !error}
        <p>No matching text found.</p>
      {/if}
    {/each}
  </div>

  {#if replaceOpen && (replacePreview || replaceError)}
    <section class="replace-preview" aria-label="Workspace replace preview">
      {#if replaceError}<p class="error" role="alert">{replaceError}</p>{/if}
      {#if replacePreview}
        <header>
          <div>
            <strong>{replacePreview.total_replacements} replacements</strong>
            <small>{replacePreview.files.length} files</small>
          </div>
          <button
            type="button"
            disabled={replacePreview.truncated || replaceApplying}
            onclick={onApplyReplace}>{replaceApplying ? "Applying" : "Apply all"}</button
          >
        </header>
        {#if replacePreview.truncated}
          <p class="error">Preview exceeded safety limits and cannot be applied.</p>
        {/if}
        <div class="replace-files">
          {#each replacePreview.files as file (file.file_path)}
            <article>
              <strong>{file.file_path}</strong>
              <small>{file.replacement_count} replacements</small>
              <span class="before">- {file.before_preview}</span>
              <span class="after">+ {file.after_preview}</span>
            </article>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</aside>

<style>
  .workspace-search-pane {
    display: flex;
    min-height: 0;
    flex: 1;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    color: var(--text-primary);
    background: var(--surface);
  }

  .workspace-search-pane > header {
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
    background: var(--surface-raised);
  }

  .workspace-search-pane > header p,
  .workspace-search-pane > header h2 {
    margin: 0;
  }

  .workspace-search-pane > header p {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .workspace-search-pane > header h2 {
    color: var(--text-bright);
    font-size: 16px;
  }

  .search-controls {
    display: flex;
    gap: 6px;
    padding: 10px;
    border-bottom: 1px solid var(--border-strong);
  }

  label {
    display: flex;
    min-width: 0;
    flex: 1;
    align-items: center;
    gap: 7px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    padding: 0 8px;
    background: var(--surface-inset);
  }

  label:focus-within {
    border-color: var(--focus-border);
    box-shadow: 0 0 0 3px var(--focus-ring);
  }

  input {
    min-width: 0;
    flex: 1;
    border: 0;
    padding: 8px 0;
    color: var(--text-primary);
    background: transparent;
    outline: none;
  }

  .search-controls button {
    width: 36px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    color: var(--text-secondary);
    background: var(--surface-raised);
    font-family: var(--font-mono);
    cursor: pointer;
  }

  .search-controls button.active {
    border-color: var(--selection-border);
    color: var(--text-bright);
    background: var(--surface-selected);
  }

  .replace-controls {
    display: flex;
    gap: 6px;
    padding: 0 10px 10px;
    border-bottom: 1px solid var(--border-strong);
  }

  .replace-controls input {
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    padding: 8px;
    background: var(--surface-inset);
  }

  .replace-controls button,
  .replace-preview header button {
    border: 1px solid var(--border-interactive);
    border-radius: 6px;
    padding: 0 10px;
    color: var(--surface);
    background: var(--text-link);
    font-weight: 750;
    cursor: pointer;
  }

  .replace-controls button:disabled,
  .replace-preview header button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .search-status {
    padding: 7px 11px;
    border-bottom: 1px solid var(--border-strong);
    color: var(--text-muted);
    font-size: 10px;
  }

  .search-status .error {
    color: var(--danger);
  }

  .search-results {
    display: grid;
    min-height: 0;
    flex: 1 1 auto;
    align-content: start;
    overflow: auto;
  }

  .search-results button {
    position: relative;
    display: grid;
    min-width: 0;
    gap: 3px;
    border: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border-strong) 55%, transparent);
    padding: 8px 42px 8px 11px;
    color: var(--text-primary);
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .search-results button:hover,
  .search-results button:focus-visible {
    outline: none;
    background: var(--surface-hover);
    box-shadow: inset 0 0 0 2px var(--focus-border);
  }

  .result-path,
  .result-preview {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .result-path {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 750;
  }

  .result-preview {
    font-family: var(--font-mono);
    font-size: 10px;
  }

  .search-results small {
    position: absolute;
    top: 9px;
    right: 10px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 9px;
  }

  .search-results p {
    margin: 0;
    padding: 20px 12px;
    color: var(--text-muted);
    font-size: 11px;
    text-align: center;
  }

  .replace-preview {
    display: grid;
    max-height: 48%;
    overflow: hidden;
    border-top: 1px solid var(--border-strong);
    background: var(--surface-raised);
  }

  .replace-preview > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
  }

  .replace-preview > header div {
    display: grid;
  }

  .replace-preview > header small {
    color: var(--text-muted);
  }

  .replace-preview .error {
    margin: 0;
    padding: 7px 10px;
    color: var(--danger);
    font-size: 10px;
  }

  .replace-files {
    display: grid;
    overflow: auto;
  }

  .replace-files article {
    display: grid;
    gap: 3px;
    padding: 8px 10px;
    border-top: 1px solid var(--border-strong);
    font-size: 10px;
  }

  .replace-files article > strong,
  .replace-files article > span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .replace-files article > small {
    color: var(--text-muted);
  }

  .replace-files .before {
    color: var(--diff-del);
    background: var(--diff-del-bg);
  }

  .replace-files .after {
    color: var(--diff-add);
    background: var(--diff-add-bg);
  }
</style>
