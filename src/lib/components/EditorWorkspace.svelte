<script lang="ts">
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import AiEditReview from "$lib/components/AiEditReview.svelte";
  import type { AiEditProposal } from "$lib/aiEdit";
  import type { CodeEditorHandle, DocumentSession } from "$lib/editorDocument";
  import type { EditorCommandId } from "$lib/editorCommands";
  import type {
    LspCodeAction,
    LspCompletionResult,
    LspDiagnostic,
    LspDocumentSymbol,
    LspLocation,
  } from "$lib/ipc";

  type Props = {
    openEditorFiles: DocumentSession[];
    activeEditorPath: string | null;
    activeEditorFile: DocumentSession | null;
    activeEditorReady: boolean;
    activeEditorDirty: boolean;
    formattingFilePath: string | null;
    savingFilePath: string | null;
    editorDiagnostic: string | null;
    fileStatusLabel: string;
    onExecuteCommand: (command: EditorCommandId) => void;
    onSelectEditorTab: (path: string) => void;
    onCloseEditorTab: (path: string) => void;
    onSetEditorDirty: (path: string, dirty: boolean) => void;
    onEditorChange: (path: string) => void;
    onEditorReady: (handle: CodeEditorHandle | null) => void;
    vimMode: boolean;
    onToggleVimMode: () => void;
    onReloadActiveFile: () => void;
    lspDiagnostics: LspDiagnostic[];
    lspStatus: string;
    onLspHover: (position: { line: number; character: number }) => Promise<string | null>;
    onLspCompletion: (position: {
      line: number;
      character: number;
    }) => Promise<LspCompletionResult | null>;
    lspCodeActions: LspCodeAction[];
    lspCodeActionsLoading: boolean;
    onRequestLspCodeActions: () => void;
    onApplyLspCodeAction: (action: LspCodeAction) => void;
    aiEditProposal: AiEditProposal | null;
    aiEditGenerating: boolean;
    aiEditStale: boolean;
    aiEditSelectedHunkIds: string[];
    aiEditError: string | null;
    onRequestAiEdit: (instruction: string) => void;
    onCancelAiEdit: () => void;
    onToggleAiEditHunk: (id: string) => void;
    onApplySelectedAiEdit: () => void;
    onAcceptAllAiEdit: () => void;
    onRejectAiEdit: () => void;
    aiEditContextDocuments: Array<{
      id: string;
      path: string;
      pinned: boolean;
      characters: number;
    }>;
    aiEditContextCharacters: number;
    onToggleAiEditContext: (documentId: string) => void;
    canNavigateBack: boolean;
    canNavigateForward: boolean;
    lspSymbols: LspDocumentSymbol[];
    lspSymbolsLoading: boolean;
    onRefreshLspSymbols: () => void;
    onSelectLspSymbol: (symbol: LspDocumentSymbol) => void;
    lspReferences: LspLocation[];
    lspReferencesLoading: boolean;
    onSelectLspReference: (location: LspLocation) => void;
    onCloseLspReferences: () => void;
  };

  let {
    openEditorFiles,
    activeEditorPath,
    activeEditorFile,
    activeEditorReady,
    activeEditorDirty,
    formattingFilePath,
    savingFilePath,
    editorDiagnostic,
    fileStatusLabel,
    onExecuteCommand,
    onSelectEditorTab,
    onCloseEditorTab,
    onSetEditorDirty,
    onEditorChange,
    onEditorReady,
    vimMode,
    onToggleVimMode,
    onReloadActiveFile,
    lspDiagnostics,
    lspStatus,
    onLspHover,
    onLspCompletion,
    lspCodeActions,
    lspCodeActionsLoading,
    onRequestLspCodeActions,
    onApplyLspCodeAction,
    aiEditProposal,
    aiEditGenerating,
    aiEditStale,
    aiEditSelectedHunkIds,
    aiEditError,
    onRequestAiEdit,
    onCancelAiEdit,
    onToggleAiEditHunk,
    onApplySelectedAiEdit,
    onAcceptAllAiEdit,
    onRejectAiEdit,
    aiEditContextDocuments,
    aiEditContextCharacters,
    onToggleAiEditContext,
    canNavigateBack,
    canNavigateForward,
    lspSymbols,
    lspSymbolsLoading,
    onRefreshLspSymbols,
    onSelectLspSymbol,
    lspReferences,
    lspReferencesLoading,
    onSelectLspReference,
    onCloseLspReferences,
  }: Props = $props();

  let tablist: HTMLDivElement | null = $state(null);
  let outlineOpen = $state(false);

  function focusTab(index: number) {
    requestAnimationFrame(() => {
      const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
      tabs?.[index]?.focus();
    });
  }

  function onTabKeydown(event: KeyboardEvent, index: number, path: string) {
    let nextIndex: number | null = null;
    if (event.key === "ArrowRight") nextIndex = (index + 1) % openEditorFiles.length;
    else if (event.key === "ArrowLeft")
      nextIndex = (index - 1 + openEditorFiles.length) % openEditorFiles.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = openEditorFiles.length - 1;
    else if (event.key === "Delete") {
      event.preventDefault();
      onCloseEditorTab(path);
      return;
    }
    if (nextIndex === null) return;
    event.preventDefault();
    onSelectEditorTab(openEditorFiles[nextIndex].path);
    focusTab(nextIndex);
  }

  function encodingLabel(file: DocumentSession): string {
    const encoding =
      file.encoding === "utf8"
        ? "UTF-8"
        : file.encoding === "utf8-bom"
          ? "UTF-8 BOM"
          : file.encoding === "utf16le"
            ? "UTF-16 LE"
            : "UTF-16 BE";
    return `${encoding} · ${file.lineEnding.toUpperCase()}`;
  }

  function symbolKindLabel(kind: number): string {
    if ([5, 23].includes(kind)) return "class";
    if ([6, 9].includes(kind)) return "method";
    if ([12, 3].includes(kind)) return "function";
    if ([7, 8, 10, 13].includes(kind)) return "field";
    if ([11, 14].includes(kind)) return "constant";
    if (kind === 4) return "package";
    if (kind === 2) return "module";
    return "symbol";
  }
</script>

<section class="code-editor-pane" aria-label="Code editor">
  <header>
    <div>
      <p>Editor</p>
      <h2>{activeEditorFile?.name ?? "No file open"}</h2>
    </div>
    <nav aria-label="Editor navigation">
      <button
        type="button"
        disabled={!canNavigateBack}
        aria-label="Navigate back"
        title="Back (Alt+Left)"
        onclick={() => onExecuteCommand("editor.navigateBack")}>←</button
      >
      <button
        type="button"
        disabled={!canNavigateForward}
        aria-label="Navigate forward"
        title="Forward (Alt+Right)"
        onclick={() => onExecuteCommand("editor.navigateForward")}>→</button
      >
    </nav>
  </header>
  {#if openEditorFiles.length > 0}
    <div class="editor-tabs" role="tablist" aria-label="Open files" bind:this={tablist}>
      {#each openEditorFiles as file, index (file.id)}
        <div
          class:active={file.path === activeEditorPath}
          class:dirty={file.dirty}
          class="editor-tab"
          role="presentation"
        >
          <button
            id={`editor-tab-${index}`}
            type="button"
            role="tab"
            aria-selected={file.path === activeEditorPath}
            aria-controls="active-editor-panel"
            aria-label={`${file.name}, ${file.path}${file.dirty ? ", unsaved" : ""}${file.externalConflict ? ", changed externally" : ""}`}
            tabindex={file.path === activeEditorPath ? 0 : -1}
            onclick={() => onSelectEditorTab(file.path)}
            onkeydown={(event) => onTabKeydown(event, index, file.path)}
          >
            <span>{file.name}</span>
            <small>{file.externalConflict ? "!" : file.dirty ? "•" : ""}</small>
          </button>
          <button
            type="button"
            tabindex="-1"
            aria-label={`Close ${file.name}`}
            onclick={() => onCloseEditorTab(file.path)}>×</button
          >
        </div>
      {/each}
    </div>
  {/if}
  {#if activeEditorFile && lspStatus !== "unsupported"}
    <section class="editor-outline" aria-label="Document outline">
      <div>
        <button
          type="button"
          class="outline-toggle"
          aria-expanded={outlineOpen}
          onclick={() => (outlineOpen = !outlineOpen)}
        >
          <span>{outlineOpen ? "▾" : "▸"} Outline</span>
          <small>{lspSymbolsLoading ? "loading" : `${lspSymbols.length} symbols`}</small>
        </button>
        <button
          type="button"
          class="outline-refresh"
          disabled={lspSymbolsLoading}
          aria-label="Refresh document outline"
          onclick={onRefreshLspSymbols}>↻</button
        >
      </div>
      {#if outlineOpen}
        <div class="outline-tree" aria-label={`Symbols in ${activeEditorFile.name}`}>
          {#each lspSymbols as symbol, index (`${symbol.depth}:${symbol.name}:${symbol.selection_range.start.line}:${index}`)}
            <button
              type="button"
              style={`--symbol-depth: ${symbol.depth}`}
              onclick={() => onSelectLspSymbol(symbol)}
            >
              <span>{symbol.name}</span>
              <small>{symbol.detail ?? symbolKindLabel(symbol.kind)}</small>
            </button>
          {:else}
            <p>{lspSymbolsLoading ? "Loading symbols…" : "No symbols reported for this file."}</p>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
  {#if lspReferencesLoading || lspReferences.length > 0}
    <section class="editor-references" aria-label="Symbol references">
      <header>
        <strong
          >{lspReferencesLoading
            ? "Finding references"
            : `${lspReferences.length} references`}</strong
        >
        <button type="button" aria-label="Close references" onclick={onCloseLspReferences}>×</button
        >
      </header>
      {#if !lspReferencesLoading}
        <div>
          {#each lspReferences as location, index (`${location.file_path}:${location.line}:${location.character}:${index}`)}
            <button type="button" onclick={() => onSelectLspReference(location)}>
              <span>{location.file_path}</span>
              <small>{location.line + 1}:{location.character + 1}</small>
            </button>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
  <div class="editor-stage">
    {#if activeEditorFile}
      <div
        id="active-editor-panel"
        class="active editor-panel"
        role="tabpanel"
        aria-labelledby={`editor-tab-${openEditorFiles.findIndex((file) => file.path === activeEditorPath)}`}
      >
        {#key `${activeEditorFile.id}:${vimMode}`}
          <CodeEditor
            session={activeEditorFile}
            onDirtyChange={(dirty) => onSetEditorDirty(activeEditorFile.path, dirty)}
            onChange={() => onEditorChange(activeEditorFile.path)}
            onReady={onEditorReady}
            onCommand={onExecuteCommand}
            {vimMode}
            diagnostics={lspDiagnostics}
            onHover={onLspHover}
            onCompletion={onLspCompletion}
          />
        {/key}
      </div>
    {:else}
      <div class="editor-empty">
        <strong>Open a file to start editing</strong>
        <span
          >Browse the workspace on the left. Text files up to 1 MB are supported in this first
          editor pass.</span
        >
      </div>
    {/if}
  </div>
  {#if activeEditorFile}
    <AiEditReview
      proposal={aiEditProposal}
      generating={aiEditGenerating}
      stale={aiEditStale}
      selectedHunkIds={aiEditSelectedHunkIds}
      error={aiEditError}
      onRequest={onRequestAiEdit}
      onCancel={onCancelAiEdit}
      onToggleHunk={onToggleAiEditHunk}
      onApplySelected={onApplySelectedAiEdit}
      onAcceptAll={onAcceptAllAiEdit}
      onReject={onRejectAiEdit}
      contextDocuments={aiEditContextDocuments}
      contextCharacters={aiEditContextCharacters}
      diagnosticCount={lspDiagnostics.length}
      onToggleContext={onToggleAiEditContext}
    />
  {/if}
  {#if editorDiagnostic}
    <div class="editor-diagnostic" role="status">
      <strong>Language check</strong>
      <span>{editorDiagnostic}</span>
    </div>
  {/if}
  {#if activeEditorFile && lspDiagnostics.length > 0}
    <div class="code-actions" aria-label="Language quick fixes">
      <button type="button" disabled={lspCodeActionsLoading} onclick={onRequestLspCodeActions}>
        {lspCodeActionsLoading ? "Loading fixes" : "Quick fixes"}
      </button>
      {#each lspCodeActions as action, index (`${index}:${action.title}`)}
        <button
          type="button"
          class:preferred={action.is_preferred}
          onclick={() => onApplyLspCodeAction(action)}
        >
          {action.title}
        </button>
      {/each}
    </div>
  {/if}
  <footer>
    <div class="editor-footer-info">
      <span>{fileStatusLabel}</span>
      {#if activeEditorFile}<span>{encodingLabel(activeEditorFile)}</span>{/if}
      {#if activeEditorFile}<span>LSP {lspStatus} · {lspDiagnostics.length} issues</span>{/if}
    </div>
    <div class="editor-actions">
      <span class:dirty={activeEditorDirty || activeEditorFile?.externalConflict}
        >{activeEditorFile?.externalConflict
          ? "External conflict"
          : activeEditorDirty
            ? "Unsaved"
            : activeEditorFile
              ? "Saved"
              : "Idle"}</span
      >
      {#if activeEditorFile?.externalConflict}
        <button type="button" onclick={onReloadActiveFile}>Reload disk</button>
      {/if}
      <button
        type="button"
        disabled={!activeEditorReady || formattingFilePath !== null}
        onclick={() => onExecuteCommand("editor.format")}
      >
        {formattingFilePath ? "Formatting" : "Format"}
      </button>
      <button
        type="button"
        disabled={!activeEditorReady || savingFilePath !== null}
        onclick={() => onExecuteCommand("editor.save")}
      >
        {savingFilePath ? "Saving" : "Save"}
      </button>
      <button type="button" aria-pressed={vimMode} onclick={onToggleVimMode}>
        {vimMode ? "Vim on" : "Vim off"}
      </button>
      <span>Esc then Tab exits · F12 definition · Shift+F12 references · Ctrl/Cmd+. fixes</span>
    </div>
  </footer>
</section>

<style>
  .code-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    overflow-x: auto;
    padding: 7px 12px;
    border-top: 1px solid var(--border-light);
    background: var(--bg-card);
  }

  header nav {
    display: flex;
    gap: 5px;
  }

  header nav button {
    width: 30px;
    height: 30px;
    border: 1px solid var(--border-light);
    border-radius: 6px;
    color: var(--text-primary);
    background: var(--bg-base);
    cursor: pointer;
  }

  header nav button:disabled {
    cursor: default;
    opacity: 0.35;
  }

  .editor-outline {
    flex: 0 0 auto;
    border-bottom: 1px solid var(--border-light);
    background: var(--bg-card);
  }

  .editor-outline > div:first-child {
    display: flex;
  }

  .outline-toggle,
  .outline-refresh,
  .outline-tree button {
    border: 0;
    color: var(--text-primary);
    background: transparent;
    cursor: pointer;
  }

  .outline-toggle {
    display: flex;
    min-width: 0;
    flex: 1;
    align-items: center;
    justify-content: space-between;
    padding: 7px 12px;
    font-size: 11px;
    font-weight: 750;
  }

  .outline-toggle small,
  .outline-tree small {
    overflow: hidden;
    color: var(--text-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .outline-refresh {
    width: 34px;
    border-left: 1px solid var(--border-light);
  }

  .outline-refresh:disabled {
    cursor: wait;
    opacity: 0.4;
  }

  .outline-tree {
    display: grid;
    max-height: 240px;
    overflow: auto;
    border-top: 1px solid var(--border-light);
    padding: 4px 0;
  }

  .outline-tree button {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 5px 12px 5px calc(12px + var(--symbol-depth) * 14px);
    text-align: left;
  }

  .outline-tree button:hover,
  .outline-tree button:focus-visible {
    outline: none;
    background: var(--bg-hover);
    box-shadow: inset 0 0 0 2px var(--focus-border);
  }

  .outline-tree button span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .outline-tree p {
    margin: 0;
    padding: 10px 12px;
    color: var(--text-muted);
    font-size: 11px;
  }

  .editor-references {
    display: grid;
    flex: 0 0 auto;
    max-height: 220px;
    overflow: hidden;
    border-bottom: 1px solid var(--border-light);
    background: var(--bg-base);
  }

  .editor-references > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 10px 6px 12px;
    border-bottom: 1px solid var(--border-light);
    color: var(--text-secondary);
    font-size: 11px;
  }

  .editor-references > header button {
    border: 0;
    color: var(--text-secondary);
    background: transparent;
    cursor: pointer;
  }

  .editor-references > div {
    display: grid;
    overflow: auto;
  }

  .editor-references > div button {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border: 0;
    padding: 6px 12px;
    color: var(--text-primary);
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .editor-references > div button:hover,
  .editor-references > div button:focus-visible {
    outline: none;
    background: var(--bg-hover);
    box-shadow: inset 0 0 0 2px var(--focus-border);
  }

  .editor-references span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .editor-references small {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }

  .code-actions button {
    flex: 0 0 auto;
    border: 1px solid var(--border-light);
    border-radius: 5px;
    padding: 5px 8px;
    color: var(--text-primary);
    background: var(--bg-base);
    font-size: 11px;
    cursor: pointer;
  }

  .code-actions button.preferred {
    border-color: var(--accent);
  }

  .code-actions button:disabled {
    cursor: wait;
    opacity: 0.55;
  }
</style>
