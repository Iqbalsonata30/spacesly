<script lang="ts">
  import { onMount } from "svelte";
  import {
    ChevronsLeft,
    File as FileIcon,
    FilePlus2,
    Folder,
    FolderOpen,
    ListCollapse,
    Loader2,
    RefreshCw,
  } from "lucide-svelte";
  import WorkspaceRow from "$lib/components/WorkspaceRow.svelte";
  import type { FileEntry } from "$lib/ipc";
  import type { GitChangedFile } from "$lib/ipc/git";
  import {
    fileTreeNavigationIndex,
    flattenFileBrowserRows,
    folderDisclosureState,
    type FileTreeNavigationKey,
  } from "$lib/fileBrowser";

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
    onClearFilter: _onClearFilter,
    onCollapseAll,
    onToggleSidebar,
  }: Props = $props();

  let visibleRows = $derived(flattenFileBrowserRows(fileEntries, expandedFolders, fileFilter));
  let changedByPath = $derived(new Map(changedFiles.map((file) => [file.path, file.status])));
  let currentPath = $derived(fileDirectory ? `${fileRootLabel}/${fileDirectory}` : fileRootLabel);
  let tree: HTMLDivElement | null = $state(null);
  let focusedPath = $state<string | null>(null);
  let typeahead = "";
  let typeaheadTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    if (visibleRows.length === 0) {
      focusedPath = null;
      return;
    }
    if (focusedPath && visibleRows.some((row) => row.entry.path === focusedPath)) return;
    focusedPath =
      visibleRows.find((row) => row.entry.path === activeEditorPath)?.entry.path ??
      visibleRows[0].entry.path;
  });

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

  function focusRow(index: number) {
    const row = visibleRows[index];
    if (!row) return;
    focusedPath = row.entry.path;
    requestAnimationFrame(() => {
      const items = tree?.querySelectorAll<HTMLButtonElement>('[role="treeitem"]');
      items?.[index]?.focus();
    });
  }

  function activateRow(entry: FileEntry) {
    if (entry.is_dir) onToggleFolder(entry);
    else onOpenEntry(entry);
  }

  function handleTreeKeydown(event: KeyboardEvent, index: number) {
    const row = visibleRows[index];
    if (!row) return;
    let nextIndex: number | null = null;
    if (["ArrowDown", "ArrowUp", "ArrowRight", "ArrowLeft", "Home", "End"].includes(event.key)) {
      const key = event.key as FileTreeNavigationKey;
      nextIndex = fileTreeNavigationIndex(visibleRows, index, key, expandedFolders);
      if (key === "ArrowRight" && row.entry.is_dir && !expandedFolders[row.entry.path]) {
        onToggleFolder(row.entry);
      } else if (key === "ArrowLeft" && row.entry.is_dir && expandedFolders[row.entry.path]) {
        onToggleFolder(row.entry);
      }
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activateRow(row.entry);
      return;
    } else if (event.key.length === 1 && !event.altKey && !event.ctrlKey && !event.metaKey) {
      typeahead += event.key.toLowerCase();
      if (typeaheadTimer) clearTimeout(typeaheadTimer);
      typeaheadTimer = setTimeout(() => {
        typeahead = "";
        typeaheadTimer = null;
      }, 700);
      const ordered = [...visibleRows.slice(index + 1), ...visibleRows.slice(0, index + 1)];
      const matchOffset = ordered.findIndex((candidate) =>
        candidate.entry.name.toLowerCase().startsWith(typeahead),
      );
      if (matchOffset >= 0) {
        const remaining = visibleRows.length - index - 1;
        nextIndex = matchOffset < remaining ? index + 1 + matchOffset : matchOffset - remaining;
      }
    }
    if (nextIndex === null) return;
    event.preventDefault();
    focusRow(nextIndex);
  }

  onMount(() => {
    const handleKeydown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "o") {
        event.preventDefault();
        if (event.shiftKey) onOpenFolder();
        else onOpenFile();
      }
    };

    window.addEventListener("keydown", handleKeydown);
    return () => {
      window.removeEventListener("keydown", handleKeydown);
      if (typeaheadTimer) clearTimeout(typeaheadTimer);
    };
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
      <ChevronsLeft size={16} aria-hidden="true" />
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
      <FolderOpen size={16} aria-hidden="true" />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onOpenFile}
      aria-label="Open file"
      title="Open file (Ctrl+O)"
    >
      <FileIcon size={16} aria-hidden="true" />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={handleCreateFile}
      aria-label="New file"
      title="New file"
    >
      <FilePlus2 size={16} aria-hidden="true" />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onRefreshDirectory}
      aria-label="Refresh"
      title="Refresh root"
    >
      <RefreshCw size={16} aria-hidden="true" />
    </button>
    <button
      type="button"
      disabled={fileLoading}
      onclick={onCollapseAll}
      aria-label="Collapse all"
      title="Collapse all"
    >
      <ListCollapse size={16} aria-hidden="true" />
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

  <div class="file-list" role="tree" aria-label="Workspace files" bind:this={tree}>
    {#each visibleRows as row, index (row.entry.path)}
      {@const status = changedByPath.get(row.entry.path)}
      {@const disclosure = row.entry.is_dir
        ? folderDisclosureState(expandedFolders, expandingFolders, row.entry.path)
        : null}
      {#snippet rowLeading()}
        {#if row.entry.is_dir}
          {#if disclosure === "loading"}
            <Loader2
              size={14}
              class="row-icon"
              aria-hidden="true"
              style="animation: spin 1s linear infinite;"
            />
          {:else if disclosure === "expanded"}
            <FolderOpen size={14} class="row-icon" aria-hidden="true" />
          {:else}
            <Folder size={14} class="row-icon" aria-hidden="true" />
          {/if}
        {:else}
          <FileIcon size={14} class="row-icon" aria-hidden="true" />
        {/if}
      {/snippet}
      <WorkspaceRow
        label={rowLabel(row.entry)}
        title={row.entry.path}
        depth={row.depth}
        active={activeEditorPath === row.entry.path}
        treeItem={true}
        tabIndex={focusedPath === row.entry.path ? 0 : -1}
        ariaLevel={row.depth + 1}
        ariaExpanded={row.entry.is_dir ? disclosure === "expanded" : undefined}
        ariaSelected={activeEditorPath === row.entry.path}
        treePath={row.entry.path}
        status={statusBadge(status)}
        statusTone={statusTone(status)}
        leading={rowLeading}
        onClick={() => activateRow(row.entry)}
        onFocus={() => (focusedPath = row.entry.path)}
        onKeydown={(event) => handleTreeKeydown(event, index)}
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
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--surface);
  }

  .file-browser-pane > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid var(--border-subtle);
    padding: 14px 16px;
    background: var(--surface-raised);
  }

  .file-header-copy {
    min-width: 0;
  }

  .file-header-copy p {
    margin: 0 0 5px;
    color: var(--text-secondary);
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
    color: var(--text-bright);
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
    color: var(--text-secondary);
  }

  .file-collapse-button,
  .file-toolbar button {
    display: inline-grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--surface-raised);
    color: var(--text-link);
  }

  .file-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    border-bottom: 1px solid var(--border-subtle);
    padding: 12px 16px;
    background: var(--surface);
  }

  .file-toolbar button:hover:not(:disabled),
  .file-toolbar button:focus-visible:not(:disabled),
  .file-collapse-button:hover:not(:disabled),
  .file-collapse-button:focus-visible:not(:disabled) {
    border-color: var(--focus-border);
    background: var(--surface-hover);
  }

  .file-filter {
    display: grid;
    gap: 6px;
    border-bottom: 1px solid var(--border-subtle);
    padding: 12px 16px 14px;
    background: var(--surface);
  }

  .file-filter span {
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .file-filter input {
    min-height: 36px;
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    background: var(--surface-inset);
    color: var(--text-bright);
    font: inherit;
    padding: 0 12px;
  }

  .file-error {
    border-bottom: 1px solid var(--danger-border);
    padding: 11px 16px;
    background: var(--danger-bg);
    color: var(--danger);
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
    color: var(--text-secondary);
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
