import type { EditorState, Text } from "@codemirror/state";
import type { LineEnding, TextEncoding } from "$lib/ipc/files";
import type { LspTextEdit } from "$lib/ipc";

export type EditorTransactionOrigin = "user" | "disk" | "format" | "ai" | "restore";

export type EditorSelectionSnapshot = {
  start_line: number;
  start_character: number;
  end_line: number;
  end_character: number;
  text: string;
};

export type CodeEditorHandle = {
  getValue: () => string;
  getSnapshot: () => { value: string; revision: number };
  setValue: (value: string, origin?: EditorTransactionOrigin) => void;
  markSaved: (value?: string) => boolean;
  focus: () => void;
  getCursorPosition: () => { line: number; character: number };
  setCursorPosition: (line: number, character: number) => void;
  applyTextEdits: (edits: LspTextEdit[]) => boolean;
  getSelectionSnapshot: () => EditorSelectionSnapshot | null;
};

export type DocumentSession = {
  id: string;
  workspaceId: string;
  path: string;
  name: string;
  state: EditorState | null;
  initialValue: string;
  persistedValue: string;
  persistedDoc: Text | null;
  version: string;
  rootRevision: number;
  encoding: TextEncoding;
  lineEnding: LineEnding;
  revision: number;
  dirty: boolean;
  externalConflict: boolean;
  scrollTop: number;
  lastOrigin: EditorTransactionOrigin;
};

export function createDocumentSession(values: {
  workspaceId: string;
  path: string;
  name: string;
  content: string;
  version: string;
  rootRevision: number;
  encoding: TextEncoding;
  lineEnding: LineEnding;
}): DocumentSession {
  return {
    id: `${values.workspaceId}:${values.rootRevision}:${values.path}`,
    workspaceId: values.workspaceId,
    path: values.path,
    name: values.name,
    state: null,
    initialValue: values.content,
    persistedValue: values.content,
    persistedDoc: null,
    version: values.version,
    rootRevision: values.rootRevision,
    encoding: values.encoding,
    lineEnding: values.lineEnding,
    revision: 0,
    dirty: false,
    externalConflict: false,
    scrollTop: 0,
    lastOrigin: "restore",
  };
}

export function documentValue(session: DocumentSession): string {
  return session.state?.doc.toString() ?? session.initialValue;
}

export function documentSnapshot(session: DocumentSession): { value: string; revision: number } {
  return { value: documentValue(session), revision: session.revision };
}

export function editorIsDirty(currentValue: string, savedValue: string): boolean {
  return currentValue !== savedValue;
}

export function applySavedSnapshot(currentValue: string, savedValue: string) {
  return {
    savedValue,
    dirty: editorIsDirty(currentValue, savedValue),
  };
}

export function markDocumentSaved(session: DocumentSession, savedValue: string): boolean {
  const currentValue = documentValue(session);
  const saved = applySavedSnapshot(currentValue, savedValue);
  session.persistedValue = saved.savedValue;
  session.persistedDoc = !saved.dirty && session.state ? session.state.doc : null;
  session.dirty = saved.dirty;
  session.externalConflict = false;
  return session.dirty;
}

export function markDocumentExternalConflict(session: DocumentSession) {
  session.externalConflict = true;
}

export function updateDocumentState(
  session: DocumentSession,
  state: EditorState,
  origin: EditorTransactionOrigin,
) {
  session.state = state;
  session.revision += 1;
  session.lastOrigin = origin;
  session.dirty = session.persistedDoc
    ? !state.doc.eq(session.persistedDoc)
    : editorIsDirty(state.doc.toString(), session.persistedValue);
}

export function replaceDocument(
  session: DocumentSession,
  value: string,
  origin: EditorTransactionOrigin,
) {
  if (session.state) {
    const transaction = session.state.update({
      changes: { from: 0, to: session.state.doc.length, insert: value },
    });
    updateDocumentState(session, transaction.state, origin);
    return;
  }
  session.initialValue = value;
  session.persistedDoc = null;
  session.revision += 1;
  session.lastOrigin = origin;
  session.dirty = editorIsDirty(value, session.persistedValue);
}
