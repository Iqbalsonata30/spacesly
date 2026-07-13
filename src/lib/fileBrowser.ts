import type { FileEntry } from "$lib/ipc";

export type FileBrowserRow = {
  entry: FileEntry;
  depth: number;
};

export function flattenFileBrowserRows(
  entries: FileEntry[],
  expandedFolders: Record<string, FileEntry[]>,
  filter: string,
): FileBrowserRow[] {
  const query = filter.trim().toLowerCase();

  function visit(items: FileEntry[], depth: number): FileBrowserRow[] {
    const rows: FileBrowserRow[] = [];

    for (const entry of items) {
      const children = expandedFolders[entry.path] ?? [];
      const selfMatches = !query || entry.name.toLowerCase().includes(query) || entry.path.toLowerCase().includes(query);
      const childRows = query
        ? visit(children, depth + 1)
        : expandedFolders[entry.path]
          ? visit(children, depth + 1)
          : [];
      const visible = !query || selfMatches || childRows.length > 0;

      if (visible) {
        rows.push({ entry, depth });
        rows.push(...childRows);
      }
    }

    return rows;
  }

  return visit(entries, 0);
}

export function collectAncestorPaths(path: string): string[] {
  return path
    .split("/")
    .filter(Boolean)
    .slice(0, -1)
    .reduce<string[]>((paths, segment) => {
      const previous = paths.at(-1) ?? "";
      paths.push(previous ? `${previous}/${segment}` : segment);
      return paths;
    }, []);
}

export function pruneExpandedFolderTree(
  expandedFolders: Record<string, FileEntry[]>,
  folderPath: string,
): Record<string, FileEntry[]> {
  const prefix = `${folderPath}/`;
  return Object.fromEntries(
    Object.entries(expandedFolders).filter(([path]) => path !== folderPath && !path.startsWith(prefix)),
  );
}

export function folderDisclosureState(
  expandedFolders: Record<string, FileEntry[]>,
  expandingFolders: Record<string, true>,
  path: string,
): "collapsed" | "expanded" | "loading" {
  if (expandingFolders[path]) return "loading";
  return expandedFolders[path] ? "expanded" : "collapsed";
}
