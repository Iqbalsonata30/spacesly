<script lang="ts">
  import { directoryBreadcrumbs, parentDirectory } from "$lib/filesFeature";
  import type { FileEntry } from "$lib/ipc";

  type Props = {
    fileRootLabel: string;
    fileDirectory: string;
    fileLoading: boolean;
    fileError: string | null;
    fileEntries: FileEntry[];
    activeEditorPath: string | null;
    onOpenFolder: () => void;
    onOpenFile: () => void;
    onRefreshDirectory: (path: string) => void;
    onOpenEntry: (entry: FileEntry) => void;
    onToggleSidebar: () => void;
  };

  let {
    fileRootLabel,
    fileDirectory,
    fileLoading,
    fileError,
    fileEntries,
    activeEditorPath,
    onOpenFolder,
    onOpenFile,
    onRefreshDirectory,
    onOpenEntry,
    onToggleSidebar,
  }: Props = $props();

  function formatBytes(bytes: number): string {
    if (bytes < 1_024) return `${bytes} B`;
    if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
    return `${(bytes / 1_048_576).toFixed(1)} MB`;
  }
</script>

<aside class="file-browser-pane" aria-label="Workspace files">
  <header>
    <div>
      <p>Files</p>
      <h2>{fileRootLabel}{fileDirectory ? `/${fileDirectory}` : ""}</h2>
    </div>
    <button class="file-collapse-button" type="button" onclick={onToggleSidebar} aria-label="Hide file browser">&lt;</button>
  </header>
  <div class="file-header-actions">
    <button class="file-pick-action primary" type="button" disabled={fileLoading} onclick={onOpenFolder}>
      <strong>Open Folder</strong>
      <small>Set editor root</small>
    </button>
    <button class="file-pick-action" type="button" disabled={fileLoading} onclick={onOpenFile}>
      <strong>Open File</strong>
      <small>Jump to file</small>
    </button>
  </div>
  <div class="file-sync-note">File browser starts at home. Use Open Folder or Open File to choose the editor root.</div>
  <nav class="file-breadcrumbs" aria-label="Current directory">
    {#each directoryBreadcrumbs(fileDirectory) as crumb (crumb.path)}
      <button class:active={crumb.path === fileDirectory} type="button" onclick={() => onRefreshDirectory(crumb.path)}>{crumb.label}</button>
    {/each}
  </nav>
  {#if fileError}
    <div class="file-error" role="status">{fileError}</div>
  {/if}
  <div class="file-list" role="list">
    {#if parentDirectory(fileDirectory) !== null}
      <button class="file-row directory" type="button" onclick={() => onRefreshDirectory(parentDirectory(fileDirectory) ?? "")}>
        <span>../</span>
        <small>Parent directory</small>
      </button>
    {/if}
    {#each fileEntries as entry (entry.path)}
      <button
        class:directory={entry.is_dir}
        class:active={activeEditorPath === entry.path}
        class="file-row"
        type="button"
        onclick={() => onOpenEntry(entry)}
      >
        <span>{entry.is_dir ? `${entry.name}/` : entry.name}</span>
        <small>{entry.is_dir ? "Directory" : formatBytes(entry.size)}</small>
      </button>
    {:else}
      <div class="file-empty">No files in this directory.</div>
    {/each}
  </div>
</aside>
