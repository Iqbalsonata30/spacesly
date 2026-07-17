<script lang="ts">
  import CodeEditor from "$lib/components/CodeEditor.svelte";
  import AiEditReview from "$lib/components/AiEditReview.svelte";
  import type { AiEditProposal } from "$lib/aiEdit";
  import type { CodeEditorHandle, DocumentSession } from "$lib/editorDocument";
  import type { EditorCommandId } from "$lib/editorCommands";
  import type { LspCodeAction, LspCompletionResult, LspDiagnostic } from "$lib/ipc";

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
    canNavigateBack: boolean;
    canNavigateForward: boolean;
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
    canNavigateBack,
    canNavigateForward,
  }: Props = $props();

  let tablist: HTMLDivElement | null = $state(null);

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
      <span>Esc then Tab exits · Ctrl/Cmd+S saves · F12 definition · Ctrl/Cmd+. fixes</span>
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
