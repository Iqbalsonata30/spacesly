import type { Text } from "@codemirror/state";
import type { LspPosition, LspRange, LspTextEdit } from "$lib/ipc";

export type EditorTextChange = { from: number; to: number; insert: string };

export function offsetToLspPosition(doc: Text, offset: number): LspPosition {
  const clamped = Math.max(0, Math.min(offset, doc.length));
  const line = doc.lineAt(clamped);
  return { line: line.number - 1, character: clamped - line.from };
}

export function lspPositionToOffset(doc: Text, position: LspPosition): number {
  const line = doc.line(Math.max(1, Math.min(position.line + 1, doc.lines)));
  return Math.min(line.to, line.from + Math.max(0, position.character));
}

export function lspRangeToOffsets(doc: Text, range: LspRange) {
  const from = lspPositionToOffset(doc, range.start);
  const to = lspPositionToOffset(doc, range.end);
  return { from: Math.min(from, to), to: Math.max(from, to) };
}

export function lspTextEditsToChanges(doc: Text, edits: LspTextEdit[]): EditorTextChange[] | null {
  const changes = edits
    .map((edit) => ({ ...lspRangeToOffsets(doc, edit.range), insert: edit.new_text }))
    .sort((left, right) => left.from - right.from || left.to - right.to);
  for (let index = 1; index < changes.length; index += 1) {
    if (changes[index].from < changes[index - 1].to) return null;
  }
  return changes;
}

export function completionType(kind: number | null): string | undefined {
  if (kind === 2 || kind === 3) return "function";
  if (kind === 4) return "constructor";
  if (kind === 5 || kind === 10) return "property";
  if (kind === 6) return "variable";
  if (kind === 7) return "class";
  if (kind === 8) return "interface";
  if (kind === 9) return "namespace";
  if (kind === 12) return "value";
  if (kind === 13) return "enum";
  if (kind === 14) return "keyword";
  if (kind === 15 || kind === 17) return "text";
  if (kind === 21) return "constant";
  return undefined;
}
