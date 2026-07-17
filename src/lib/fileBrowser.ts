import type { FileEntry } from "$lib/ipc";

export type FileBrowserRow = {
  entry: FileEntry;
  depth: number;
};

export type FileTreeNavigationKey =
  "ArrowDown" | "ArrowUp" | "ArrowRight" | "ArrowLeft" | "Home" | "End";

export function fileTreeNavigationIndex(
  rows: FileBrowserRow[],
  index: number,
  key: FileTreeNavigationKey,
  expandedFolders: Record<string, FileEntry[]>,
): number | null {
  const row = rows[index];
  if (!row) return null;
  if (key === "ArrowDown") return Math.min(rows.length - 1, index + 1);
  if (key === "ArrowUp") return Math.max(0, index - 1);
  if (key === "Home") return 0;
  if (key === "End") return rows.length - 1;
  if (key === "ArrowRight") {
    return row.entry.is_dir &&
      expandedFolders[row.entry.path] &&
      rows[index + 1]?.depth === row.depth + 1
      ? index + 1
      : null;
  }
  if (row.entry.is_dir && expandedFolders[row.entry.path]) return null;
  for (let candidate = index - 1; candidate >= 0; candidate -= 1) {
    if (rows[candidate].depth < row.depth) return candidate;
  }
  return null;
}

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
      const selfMatches =
        !query ||
        entry.name.toLowerCase().includes(query) ||
        entry.path.toLowerCase().includes(query);
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
    Object.entries(expandedFolders).filter(
      ([path]) => path !== folderPath && !path.startsWith(prefix),
    ),
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
