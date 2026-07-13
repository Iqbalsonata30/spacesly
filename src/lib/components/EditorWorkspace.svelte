<script lang="ts">
  import CodeEditor from "$lib/components/CodeEditor.svelte";

  type CodeEditorHandle = {
    getValue: () => string;
    setValue: (value: string) => void;
    markSaved: (value?: string) => void;
    focus: () => void;
  };

  type OpenEditorFile = {
    path: string;
    name: string;
    content: string;
    savedContent: string;
    dirty: boolean;
    editor: CodeEditorHandle | null;
  };

  type Props = {
    openEditorFiles: OpenEditorFile[];
    activeEditorPath: string | null;
    activeEditorFile: OpenEditorFile | null;
    activeEditorReady: boolean;
    activeEditorDirty: boolean;
    formattingFilePath: string | null;
    savingFilePath: string | null;
    editorDiagnostic: string | null;
    fileStatusLabel: string;
    onFormatActiveFile: () => void;
    onSaveActiveFile: () => void;
    onSelectEditorTab: (path: string) => void;
    onCloseEditorTab: (path: string) => void;
    onSetEditorDirty: (path: string, dirty: boolean) => void;
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
    onFormatActiveFile,
    onSaveActiveFile,
    onSelectEditorTab,
    onCloseEditorTab,
    onSetEditorDirty,
  }: Props = $props();

</script>

<section class="code-editor-pane" aria-label="Code editor">
  <header>
    <div>
      <p>Editor</p>
      <h2>{activeEditorFile?.name ?? "No file open"}</h2>
    </div>
  </header>
  {#if openEditorFiles.length > 0}
    <div class="editor-tabs" role="tablist" aria-label="Open files">
      {#each openEditorFiles as file (file.path)}
        <div class:active={file.path === activeEditorPath} class:dirty={file.dirty} class="editor-tab">
          <button type="button" onclick={() => onSelectEditorTab(file.path)}>
            <span>{file.name}</span>
            <small>{file.dirty ? "•" : ""}</small>
          </button>
          <button type="button" aria-label={`Close ${file.name}`} onclick={() => onCloseEditorTab(file.path)}>×</button>
        </div>
      {/each}
    </div>
  {/if}
  <div class="editor-stage">
    {#if activeEditorFile}
      {#each openEditorFiles as file (file.path)}
        <section class:active={file.path === activeEditorPath} class="editor-panel" aria-hidden={file.path !== activeEditorPath}>
          <CodeEditor
            bind:this={file.editor}
            initialValue={file.savedContent}
            path={file.path}
            onDirtyChange={(dirty) => onSetEditorDirty(file.path, dirty)}
            onSave={() => onSaveActiveFile()}
            onFormat={() => onFormatActiveFile()}
          />
        </section>
      {/each}
    {:else}
      <div class="editor-empty">
        <strong>Open a file to start editing</strong>
        <span>Browse the workspace on the left. Text files up to 1 MB are supported in this first editor pass.</span>
      </div>
    {/if}
  </div>
  {#if editorDiagnostic}
    <div class="editor-diagnostic" role="status">
      <strong>Language check</strong>
      <span>{editorDiagnostic}</span>
    </div>
  {/if}
  <footer>
    <div class="editor-footer-info">
      <span>{fileStatusLabel}</span>
    </div>
    <div class="editor-actions">
      <span class:dirty={activeEditorDirty}>{activeEditorDirty ? "Unsaved" : activeEditorFile ? "Saved" : "Idle"}</span>
      <button type="button" disabled={!activeEditorReady || formattingFilePath !== null} onclick={() => onFormatActiveFile()}>
        {formattingFilePath ? "Formatting" : "Format"}
      </button>
      <button type="button" disabled={!activeEditorReady || savingFilePath !== null} onclick={() => onSaveActiveFile()}>
        {savingFilePath ? "Saving" : "Save"}
      </button>
      <span>Vim · Ctrl/Cmd+S saves · Ctrl/Cmd+Shift+F formats</span>
    </div>
  </footer>
</section>
