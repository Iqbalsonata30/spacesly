import {
  applySavedSnapshot,
  createDocumentSession,
  createRecoveredDocumentSession,
  documentSnapshot,
  markDocumentSaved,
  markDocumentExternalConflict,
  replaceDocument,
} from "../src/lib/editorDocument";
import { EditorState } from "@codemirror/state";
import { createEditorCommandRegistry } from "../src/lib/editorCommands";
import { fileTreeNavigationIndex } from "../src/lib/fileBrowser";
import { workspaceFileChangeIsStructural } from "../src/lib/filesFeature";
import { lspConfigForPath } from "../src/lib/lspConfig";
import { aiEditProposalIsStale, applyAiEditHunks, createAiEditProposal } from "../src/lib/aiEdit";
import {
  lspPositionToOffset,
  lspTextEditsToChanges,
  offsetToLspPosition,
} from "../src/lib/lspEditor";
import {
  canNavigateEditor,
  createEditorNavigation,
  editorNavigationTarget,
  pushEditorLocation,
} from "../src/lib/editorNavigation";

function assertEqual(actual: unknown, expected: unknown, message: string) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

assertEqual(
  applySavedSnapshot("saved plus newer edit", "saved"),
  { savedValue: "saved", dirty: true },
  "an older save must not clear newer edits",
);

assertEqual(
  applySavedSnapshot("saved", "saved"),
  { savedValue: "saved", dirty: false },
  "the persisted current snapshot should be clean",
);

const session = createDocumentSession({
  workspaceId: "workspace-one",
  path: "src/main.ts",
  name: "main.ts",
  content: "initial",
  version: "v1",
  rootRevision: 3,
  encoding: "utf8",
  lineEnding: "lf",
});
replaceDocument(session, "changed", "ai");
assertEqual(
  {
    id: session.id,
    snapshot: documentSnapshot(session),
    dirty: session.dirty,
    origin: session.lastOrigin,
  },
  {
    id: "workspace-one:3:src/main.ts",
    snapshot: { value: "changed", revision: 1 },
    dirty: true,
    origin: "ai",
  },
  "document sessions should retain identity, revisions, and transaction origin",
);

markDocumentSaved(session, "changed");
assertEqual(session.dirty, false, "saving the current document session should clear dirty state");

const recoveredSession = createRecoveredDocumentSession({
  workspaceId: "workspace-one",
  path: "src/recovered.ts",
  name: "recovered.ts",
  content: "dirty recovered",
  persistedContent: "disk baseline",
  version: "v2",
  rootRevision: 4,
  encoding: "utf8",
  lineEnding: "lf",
  revision: 6,
  scrollTop: 120,
});
assertEqual(
  {
    value: documentSnapshot(recoveredSession),
    persisted: recoveredSession.persistedValue,
    dirty: recoveredSession.dirty,
    origin: recoveredSession.lastOrigin,
    scrollTop: recoveredSession.scrollTop,
  },
  {
    value: { value: "dirty recovered", revision: 6 },
    persisted: "disk baseline",
    dirty: true,
    origin: "restore",
    scrollTop: 120,
  },
  "recovered document sessions should preserve dirty content separately from disk baseline",
);

const statefulSession = createDocumentSession({
  workspaceId: "workspace-one",
  path: "src/state.ts",
  name: "state.ts",
  content: "initial",
  version: "v1",
  rootRevision: 3,
  encoding: "utf8-bom",
  lineEnding: "crlf",
});
statefulSession.state = EditorState.create({ doc: "initial" });
statefulSession.persistedDoc = statefulSession.state.doc;
replaceDocument(statefulSession, "incremental", "format");
assertEqual(
  {
    value: documentSnapshot(statefulSession).value,
    dirty: statefulSession.dirty,
    origin: statefulSession.lastOrigin,
  },
  { value: "incremental", dirty: true, origin: "format" },
  "CodeMirror state changes should remain in the document session",
);
markDocumentExternalConflict(statefulSession);
assertEqual(
  statefulSession.externalConflict,
  true,
  "external changes should be represented without replacing local document state",
);

const commands = createEditorCommandRegistry();
let commandRuns = 0;
const unregister = commands.register("editor.save", () => {
  commandRuns += 1;
});
assertEqual(commands.execute("editor.save"), true, "registered editor commands should execute");
unregister();
assertEqual(
  { commandRuns, handledAfterDispose: commands.execute("editor.save") },
  { commandRuns: 1, handledAfterDispose: false },
  "disposed editor commands should no longer execute",
);

const treeRows = [
  { entry: { name: "src", path: "src", is_dir: true, size: 0 }, depth: 0 },
  { entry: { name: "main.ts", path: "src/main.ts", is_dir: false, size: 10 }, depth: 1 },
  { entry: { name: "README.md", path: "README.md", is_dir: false, size: 10 }, depth: 0 },
];
const expandedTree = { src: [treeRows[1].entry] };
assertEqual(
  {
    child: fileTreeNavigationIndex(treeRows, 0, "ArrowRight", expandedTree),
    parent: fileTreeNavigationIndex(treeRows, 1, "ArrowLeft", expandedTree),
    end: fileTreeNavigationIndex(treeRows, 0, "End", expandedTree),
  },
  { child: 1, parent: 0, end: 2 },
  "file tree navigation should follow hierarchical keyboard semantics",
);

assertEqual(
  {
    modified: workspaceFileChangeIsStructural("modified"),
    created: workspaceFileChangeIsStructural("created"),
    removed: workspaceFileChangeIsStructural("removed"),
    renamed: workspaceFileChangeIsStructural("renamed"),
  },
  { modified: false, created: true, removed: true, renamed: true },
  "only structural workspace changes should rescan the explorer",
);

assertEqual(
  {
    rust: lspConfigForPath("src/main.rs")?.server_id,
    tsx: lspConfigForPath("src/App.tsx")?.language_id,
    unsupported: lspConfigForPath("README.md"),
  },
  { rust: "rust-analyzer", tsx: "typescriptreact", unsupported: null },
  "language server selection should be deterministic by document path",
);

const aiProposal = createAiEditProposal({
  documentId: "doc-1",
  path: "src/main.ts",
  baseRevision: 4,
  baseValue: "start\nold one\nmiddle\nold two\nend",
  proposedValue: "start\nnew one\nmiddle\nnew two\nend",
  summary: "Update both values",
});
assertEqual(
  aiProposal.hunks.length,
  2,
  "AI proposals should preserve independently reviewable hunks",
);
assertEqual(
  applyAiEditHunks(aiProposal.baseValue, aiProposal.hunks, new Set([aiProposal.hunks[1].id])),
  "start\nold one\nmiddle\nnew two\nend",
  "only selected AI hunks should be applied",
);
assertEqual(
  {
    current: aiEditProposalIsStale(aiProposal, "doc-1", 4),
    changed: aiEditProposalIsStale(aiProposal, "doc-1", 5),
  },
  { current: false, changed: true },
  "AI proposals should become stale when document revisions change",
);
const contextualProposal = createAiEditProposal({
  documentId: "doc-1",
  path: "src/main.ts",
  baseRevision: 4,
  baseValue: "old",
  proposedValue: "new",
  summary: "Use pinned context",
  contextRevisions: { "doc-context": 7 },
});
assertEqual(
  {
    current: aiEditProposalIsStale(contextualProposal, "doc-1", 4, { "doc-context": 7 }),
    changed: aiEditProposalIsStale(contextualProposal, "doc-1", 4, { "doc-context": 8 }),
    closed: aiEditProposalIsStale(contextualProposal, "doc-1", 4, {}),
  },
  { current: false, changed: true, closed: true },
  "AI proposals should become stale when pinned context changes or closes",
);

const unicodeDoc = EditorState.create({ doc: "a😀b\nnext" }).doc;
assertEqual(
  {
    position: offsetToLspPosition(unicodeDoc, 3),
    offset: lspPositionToOffset(unicodeDoc, { line: 0, character: 3 }),
  },
  { position: { line: 0, character: 3 }, offset: 3 },
  "LSP positions should use CodeMirror UTF-16 offsets",
);
assertEqual(
  lspTextEditsToChanges(unicodeDoc, [
    {
      range: { start: { line: 1, character: 0 }, end: { line: 1, character: 4 } },
      new_text: "done",
    },
    {
      range: { start: { line: 0, character: 3 }, end: { line: 0, character: 3 } },
      new_text: "!",
    },
  ]),
  [
    { from: 3, to: 3, insert: "!" },
    { from: 5, to: 9, insert: "done" },
  ],
  "LSP edits should be normalized into ordered atomic editor changes",
);
assertEqual(
  lspTextEditsToChanges(unicodeDoc, [
    {
      range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } },
      new_text: "first",
    },
    {
      range: { start: { line: 0, character: 2 }, end: { line: 0, character: 4 } },
      new_text: "second",
    },
  ]),
  null,
  "overlapping LSP edits should be rejected instead of partially applied",
);

let navigation = createEditorNavigation();
navigation = pushEditorLocation(navigation, { path: "src/main.ts", line: 4, character: 2 });
navigation = pushEditorLocation(navigation, { path: "src/item.ts", line: 8, character: 0 });
const navigationBack = editorNavigationTarget(navigation, -1);
assertEqual(
  {
    canBack: canNavigateEditor(navigation, -1),
    canForward: canNavigateEditor(navigation, 1),
    target: navigationBack?.location,
    backCanForward: navigationBack ? canNavigateEditor(navigationBack.state, 1) : false,
  },
  {
    canBack: true,
    canForward: false,
    target: { path: "src/main.ts", line: 4, character: 2 },
    backCanForward: true,
  },
  "editor definition navigation should preserve back and forward history",
);
