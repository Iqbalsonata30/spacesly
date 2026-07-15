<script lang="ts">
  import { onMount } from "svelte";
  import {
    ChevronsLeft,
    File as FileIcon,
    FilePlus2,
    FileSearch,
    Folder,
    FolderOpen,
    Loader2,
    RefreshCw,
  } from "lucide-svelte";
  import WorkspaceRow from "$lib/components/WorkspaceRow.svelte";
  import type { FileEntry } from "$lib/ipc";
  import type { GitChangedFile } from "$lib/ipc/git";
  import { flattenFileBrowserRows, folderDisclosureState } from "$lib/fileBrowser";

  type Props = {
    fileRootLabel: string;
    fileDirectory: string;
    fileLoading: boolean;
    fileError: string | null;
    fileEntries: FileEntry[];
    fileFilter: string;
    changedFiles: GitChangedFile[];
    expandedFolders: Record<string, FileEntry[]>;
    expandingFolders: Record<string, true>;
    activeEditorPath: string | null;
    onOpenFolder: () => void;
    onOpenFile: () => void;
    onCreateFile: () => void;
    onRefreshDirectory: () => void;
    onOpenEntry: (entry: FileEntry) => void;
    onToggleFolder: (entry: FileEntry) => void;
    onFilterChange: (filter: string) => void;
    onClearFilter: () => void;
    onCollapseAll: () => void;
    onToggleSidebar: () => void;
  };

  let {
    fileRootLabel,
    fileDirectory,
    fileLoading,
    fileError,
    fileEntries,
    fileFilter,
    changedFiles,
    expandedFolders,
    expandingFolders,
    activeEditorPath,
    onOpenFolder,
    onOpenFile,
    onCreateFile,
    onRefreshDirectory,
    onOpenEntry,
    onToggleFolder,
    onFilterChange,
    _onClearFilter,
    onCollapseAll,
    onToggleSidebar,
  }: Props = $props();

  let visibleRows = $derived(flattenFileBrowserRows(fileEntries, expandedFolders, fileFilter));
  let changedByPath = $derived(new Map(changedFiles.map((file) => [file.path, file.status])));
  let currentPath = $derived(fileDirectory ? `${fileRootLabel}/${fileDirectory}` : fileRootLabel);

  function statusTone(status?: string): "neutral" | "modified" | "added" | "deleted" {
    if (status === "M") return "modified";
    if (status === "A" || status === "U") return "added";
    if (status === "D") return "deleted";
    return "neutral";
  }

  function statusBadge(status?: string): string | undefined {
    if (!status) return undefined;
    return status;
  }

  function rowLabel(entry: FileEntry): string {
    return entry.name;
  }

  function truncateMiddle(value: string, max = 48) {
    if (value.length <= max) return value;
    const keep = Math.max(10, Math.floor((max - 1) / 2));
    return `${value.slice(0, keep)}…${value.slice(-keep)}`;
  }

  function handleCreateFile() {
    onCreateFile();
  }

  onMount(() => {
    const handleKeydown = (event: KeyboardEvent) => {
      if (event.metaKey && event.key.toLowerCase() === "o") {
        event.preventDefault();
        onOpenFile();
      }
    };

    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  });
</script>

<aside class="file-browser-pane" aria-label="Explorer">
  <header>
    <div class="file-header-copy">
      <p>Explorer</p>
      <nav class="file-breadcrumb" aria-label="Current folder" title={currentPath}>
        <span class="crumb root">{truncateMiddle(fileRootLabel)}</span>
        {#if fileDirectory}
          {#each fileDirectory.split("/").filter(Boolean) as segment, i (i)}
            <span class="crumb-separator">/</span>
            <span class="crumb">{truncateMiddle(segment)}</span>
          {/each}
        {/if}
      </nav>
    </div>
    <button
      class="file-collapse-button"
      type="button"
      onclick={onToggleSidebar}
      aria-label="Hide sidebar"
    >
      <ChevronsLeft size={15} />
    </button>
  </header>

  <div class="file-toolbar">
    <button
      type="button"
      disabled={fileLoading}
      onclick={onOpenFolder}
      aria-label="Open folder (Ctrl+Shift+O)"
      title="Open folder (Ctrl+Shift+O)"
    >
      <FolderOpen size={15} />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onOpenFile}
      aria-label="Open file"
      title="Open file (Ctrl+O)"
    >
      <FileSearch size={15} />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={handleCreateFile}
      aria-label="New file"
      title="New file"
    >
      <FilePlus2 size={15} />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onRefreshDirectory}
      aria-label="Refresh"
      title="Refresh root"
    >
      <RefreshCw size={15} />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onCollapseAll}
      aria-label="Collapse all"
      title="Collapse all"
    >
      <ChevronsLeft size={15} />
    </button>
  </div>

  <label class="file-filter">
    <span>Filter</span>
    <input
      type="search"
      placeholder="Filter by name or path"
      value={fileFilter}
      oninput={(event) => onFilterChange(event.currentTarget.value)}
    />
  </label>

  {#if fileError}
    <div class="file-error" role="status">{fileError}</div>
  {/if}

  <div class="file-list" role="list">
    {#each visibleRows as row (row.entry.path)}
      {@const status = changedByPath.get(row.entry.path)}
      {@const disclosure = row.entry.is_dir
        ? folderDisclosureState(expandedFolders, expandingFolders, row.entry.path)
        : null}
      {#snippet rowLeading()}
        {#if row.entry.is_dir}
          {#if disclosure === "loading"}
            <Loader2 size={14} class="row-icon" style="animation: spin 1s linear infinite;" />
          {:else if disclosure === "expanded"}
            <FolderOpen size={14} class="row-icon" />
          {:else}
            <Folder size={14} class="row-icon" />
          {/if}
        {:else}
          <FileIcon size={14} class="row-icon" />
        {/if}
      {/snippet}
      <WorkspaceRow
        label={rowLabel(row.entry)}
        title={row.entry.path}
        depth={row.depth}
        active={activeEditorPath === row.entry.path}
        status={statusBadge(status)}
        statusTone={statusTone(status)}
        leading={rowLeading}
        onClick={() => (row.entry.is_dir ? onToggleFolder(row.entry) : onOpenEntry(row.entry))}
      ></WorkspaceRow>
    {:else}
      <div class="file-empty">
        {fileFilter.trim() ? "No matching files." : "No files in this folder."}
      </div>
    {/each}
  </div>
</aside>

<style>
  .file-browser-pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    border: 1px solid #272530;
    border-radius: 12px;
    background: #101017;
  }

  .file-browser-pane > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid #25232d;
    padding: 14px 16px;
    background: #1c1b21;
  }

  .file-header-copy {
    min-width: 0;
  }

  .file-header-copy p {
    margin: 0 0 5px;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .file-breadcrumb {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    color: #f1edf5;
    font-size: 14px;
    font-weight: 850;
    white-space: nowrap;
  }

  .crumb,
  .crumb.root {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .crumb-separator {
    color: #8f88a8;
  }

  .file-collapse-button,
  .file-toolbar button {
    display: inline-grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border: 1px solid #34313d;
    border-radius: 8px;
    background: #1f1e27;
    color: #b8d6e4;
  }

  .file-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    border-bottom: 1px solid #25232d;
    padding: 12px 16px;
    background: #111016;
  }

  .file-toolbar button:hover:not(:disabled),
  .file-toolbar button:focus-visible:not(:disabled),
  .file-collapse-button:hover:not(:disabled),
  .file-collapse-button:focus-visible:not(:disabled) {
    border-color: #b8d6e4;
    background: #28313a;
  }

  .file-filter {
    display: grid;
    gap: 6px;
    border-bottom: 1px solid #25232d;
    padding: 12px 16px 14px;
    background: #111016;
  }

  .file-filter span {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .file-filter input {
    min-height: 36px;
    border: 1px solid #34313d;
    border-radius: 9px;
    background: #1f1e27;
    color: #f1edf5;
    font: inherit;
    padding: 0 12px;
  }

  .file-error {
    border-bottom: 1px solid #3d2930;
    padding: 11px 16px;
    background: rgba(124, 73, 82, 0.18);
    color: #f0b0aa;
    font-size: 12px;
    font-weight: 800;
  }

  .file-list {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    align-content: stretch;
    min-height: 0;
    overflow: auto;
    scrollbar-gutter: stable;
    padding: 4px 8px 2px 0;
  }

  .file-empty {
    padding: 18px 14px;
    color: #8f88a8;
    font-size: 12px;
    font-weight: 800;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
