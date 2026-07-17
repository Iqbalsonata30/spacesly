export type AiEditHunk = {
  id: string;
  oldStart: number;
  oldLines: string[];
  newStart: number;
  newLines: string[];
};

export type AiEditProposal = {
  documentId: string;
  path: string;
  baseRevision: number;
  baseValue: string;
  proposedValue: string;
  summary: string;
  hunks: AiEditHunk[];
};

const ANCHOR_WINDOW = 80;

export function createAiEditProposal(values: Omit<AiEditProposal, "hunks">): AiEditProposal {
  return { ...values, hunks: diffAiEditLines(values.baseValue, values.proposedValue) };
}

export function diffAiEditLines(previous: string, current: string): AiEditHunk[] {
  const oldLines = previous.split("\n");
  const newLines = current.split("\n");
  const hunks: AiEditHunk[] = [];
  let oldIndex = 0;
  let newIndex = 0;

  while (oldIndex < oldLines.length || newIndex < newLines.length) {
    if (oldLines[oldIndex] === newLines[newIndex]) {
      oldIndex += 1;
      newIndex += 1;
      continue;
    }

    const oldStart = oldIndex;
    const newStart = newIndex;
    const anchor = nextAnchor(oldLines, newLines, oldIndex, newIndex);
    const oldEnd = anchor?.oldIndex ?? oldLines.length;
    const newEnd = anchor?.newIndex ?? newLines.length;
    hunks.push({
      id: `${oldStart}:${newStart}:${hunks.length}`,
      oldStart,
      oldLines: oldLines.slice(oldStart, oldEnd),
      newStart,
      newLines: newLines.slice(newStart, newEnd),
    });
    oldIndex = oldEnd;
    newIndex = newEnd;
  }

  return hunks;
}

export function applyAiEditHunks(
  baseValue: string,
  hunks: AiEditHunk[],
  selectedHunkIds: ReadonlySet<string>,
): string {
  const lines = baseValue.split("\n");
  const selected = hunks
    .filter((hunk) => selectedHunkIds.has(hunk.id))
    .sort((left, right) => right.oldStart - left.oldStart);
  for (const hunk of selected) {
    lines.splice(hunk.oldStart, hunk.oldLines.length, ...hunk.newLines);
  }
  return lines.join("\n");
}

export function aiEditProposalIsStale(
  proposal: AiEditProposal,
  documentId: string,
  revision: number,
): boolean {
  return proposal.documentId !== documentId || proposal.baseRevision !== revision;
}

function nextAnchor(
  oldLines: string[],
  newLines: string[],
  oldStart: number,
  newStart: number,
): { oldIndex: number; newIndex: number } | null {
  let best: { oldIndex: number; newIndex: number; distance: number } | null = null;
  const oldLimit = Math.min(oldLines.length, oldStart + ANCHOR_WINDOW);
  const newLimit = Math.min(newLines.length, newStart + ANCHOR_WINDOW);
  for (let oldIndex = oldStart; oldIndex < oldLimit; oldIndex += 1) {
    for (let newIndex = newStart; newIndex < newLimit; newIndex += 1) {
      if (oldIndex === oldStart && newIndex === newStart) continue;
      if (oldLines[oldIndex] !== newLines[newIndex]) continue;
      const distance = oldIndex - oldStart + (newIndex - newStart);
      if (!best || distance < best.distance) best = { oldIndex, newIndex, distance };
    }
  }
  return best;
}
