<script lang="ts">
  import { onMount } from "svelte";
  import type { Extension } from "@codemirror/state";
  import type { EditorView as EditorViewType } from "@codemirror/view";
  import {
    documentSnapshot,
    documentValue,
    editorIsDirty,
    markDocumentSaved,
    updateDocumentState,
    type CodeEditorHandle,
    type DocumentSession,
    type EditorTransactionOrigin,
    type EditorSelectionSnapshot,
  } from "$lib/editorDocument";
  import { editorLanguagePluginForPath } from "$lib/editorPlugins";
  import type { EditorCommandId } from "$lib/editorCommands";
  import type { LspDiagnostic } from "$lib/ipc";
  import type { LspCompletionResult, LspTextEdit } from "$lib/ipc";
  import {
    completionType,
    lspRangeToOffsets,
    lspTextEditsToChanges,
    offsetToLspPosition,
  } from "$lib/lspEditor";

  type Runtime = {
    basicSetup: Extension;
    EditorState: typeof import("@codemirror/state").EditorState;
    StateEffect: typeof import("@codemirror/state").StateEffect;
    EditorView: typeof import("@codemirror/view").EditorView;
    keymap: typeof import("@codemirror/view").keymap;
    hoverTooltip: typeof import("@codemirror/view").hoverTooltip;
    indentWithTab: typeof import("@codemirror/commands").indentWithTab;
    HighlightStyle: typeof import("@codemirror/language").HighlightStyle;
    syntaxHighlighting: typeof import("@codemirror/language").syntaxHighlighting;
    tags: typeof import("@lezer/highlight").tags;
    vim: typeof import("@replit/codemirror-vim").vim;
    lintGutter: typeof import("@codemirror/lint").lintGutter;
    setDiagnostics: typeof import("@codemirror/lint").setDiagnostics;
    autocompletion: typeof import("@codemirror/autocomplete").autocompletion;
    snippet: typeof import("@codemirror/autocomplete").snippet;
  };

  type Props = {
    session: DocumentSession;
    onDirtyChange?: (dirty: boolean) => void;
    onChange?: (revision: number) => void;
    onReady?: (handle: CodeEditorHandle | null) => void;
    onCommand?: (command: EditorCommandId) => void;
    vimMode?: boolean;
    diagnostics?: LspDiagnostic[];
    onHover?: (position: { line: number; character: number }) => Promise<string | null>;
    onCompletion?: (position: {
      line: number;
      character: number;
    }) => Promise<LspCompletionResult | null>;
  };

  let {
    session,
    onDirtyChange = () => {},
    onChange = () => {},
    onReady = () => {},
    onCommand = () => {},
    vimMode = false,
    diagnostics = [],
    onHover = async () => null,
    onCompletion = async () => null,
  }: Props = $props();

  let host: HTMLDivElement | null = $state(null);
  let fallback: HTMLTextAreaElement | null = $state(null);
  let view: EditorViewType | null = null;
  let fallbackValue = $state("");
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let focusWhenReady = false;
  let dispatchOrigin: EditorTransactionOrigin = "user";
  let diagnosticsRuntime: Pick<Runtime, "setDiagnostics"> | null = null;

  $effect(() => {
    applyDiagnostics(diagnostics);
  });

  onMount(() => {
    let cancelled = false;

    fallbackValue = documentValue(session);

    async function initialize() {
      if (!host) return;

      try {
        const [
          {
            basicSetup,
            EditorState,
            StateEffect,
            EditorView,
            keymap,
            hoverTooltip,
            indentWithTab,
            HighlightStyle,
            syntaxHighlighting,
            tags,
            vim,
            lintGutter,
            setDiagnostics,
            autocompletion,
            snippet,
          },
          language,
        ] = await Promise.all([loadRuntime(), loadLanguage(session.path)]);
        if (cancelled || !host) return;

        const extensions = [
          basicSetup,
          ...(vimMode ? [vim({ status: true })] : []),
          lintGutter(),
          ...(language ? [language] : []),
          hoverTooltip(
            async (tooltipView, position) => {
              const line = tooltipView.state.doc.lineAt(position);
              const text = await onHover({
                line: line.number - 1,
                character: position - line.from,
              });
              if (!text) return null;
              return {
                pos: position,
                above: true,
                create: () => {
                  const dom = document.createElement("div");
                  dom.className = "cm-lsp-hover";
                  dom.textContent = text;
                  return { dom };
                },
              };
            },
            { hoverTime: 400 },
          ),
          autocompletion({
            override: [
              async (context) => {
                const word = context.matchBefore(/[\w$]*/);
                if (!context.explicit && word?.from === context.pos) return null;
                const revision = session.revision;
                const result = await onCompletion(
                  offsetToLspPosition(context.state.doc, context.pos),
                );
                if (!result || context.aborted || session.revision !== revision) return null;
                const from = word?.from ?? context.pos;
                return {
                  from,
                  options: result.items.map((item) => {
                    const insert = item.insert_text ?? item.label;
                    const primary = item.text_edit;
                    const additional = item.additional_text_edits;
                    const apply = (editorView: EditorViewType, completion: unknown) => {
                      if (item.insert_text_format === 2 && additional.length === 0) {
                        const range = primary
                          ? lspRangeToOffsets(editorView.state.doc, primary.range)
                          : { from, to: context.pos };
                        snippet(insert)(editorView, completion as never, range.from, range.to);
                        return;
                      }
                      const edits: LspTextEdit[] = [
                        ...(primary
                          ? [primary]
                          : [
                              {
                                range: {
                                  start: offsetToLspPosition(editorView.state.doc, from),
                                  end: offsetToLspPosition(editorView.state.doc, context.pos),
                                },
                                new_text: stripSnippetPlaceholders(insert),
                              },
                            ]),
                        ...additional,
                      ];
                      const changes = lspTextEditsToChanges(editorView.state.doc, edits);
                      if (changes) editorView.dispatch({ changes });
                    };
                    return {
                      label: item.label,
                      detail: item.detail ?? undefined,
                      info: item.documentation?.text,
                      type: completionType(item.kind),
                      boost: item.sort_text ? undefined : item.kind === 14 ? -1 : 0,
                      apply,
                    };
                  }),
                  filter: true,
                };
              },
            ],
          }),
          softDraculaHighlight({ HighlightStyle, syntaxHighlighting, tags }),
          keymap.of([
            indentWithTab,
            {
              key: "Mod-s",
              run: () => {
                onCommand("editor.save");
                return true;
              },
            },
            {
              key: "Mod-Shift-f",
              run: () => {
                onCommand("editor.format");
                return true;
              },
            },
            {
              key: "F12",
              run: () => {
                onCommand("editor.goToDefinition");
                return true;
              },
            },
            {
              key: "Shift-F12",
              run: () => {
                onCommand("editor.findReferences");
                return true;
              },
            },
            {
              key: "Mod-.",
              run: () => {
                onCommand("editor.quickFix");
                return true;
              },
            },
            {
              key: "Alt-ArrowLeft",
              run: () => {
                onCommand("editor.navigateBack");
                return true;
              },
            },
            {
              key: "Alt-ArrowRight",
              run: () => {
                onCommand("editor.navigateForward");
                return true;
              },
            },
          ]),
          EditorView.domEventHandlers({
            mousedown(event, editorView) {
              if (event.button !== 0 || (!event.metaKey && !event.ctrlKey)) return false;
              const position = editorView.posAtCoords({ x: event.clientX, y: event.clientY });
              if (position === null) return false;
              event.preventDefault();
              editorView.dispatch({ selection: { anchor: position } });
              onCommand("editor.goToDefinition");
              return true;
            },
          }),
          EditorView.updateListener.of((update) => {
            if (!update.docChanged) return;
            const previousDirty = session.dirty;
            updateDocumentState(session, update.state, dispatchOrigin);
            if (previousDirty !== session.dirty) onDirtyChange(session.dirty);
            onChange(session.revision);
          }),
          editorTheme(EditorView),
        ];
        const state = session.state
          ? session.state.update({ effects: StateEffect.reconfigure.of(extensions) }).state
          : EditorState.create({ doc: documentValue(session), extensions });
        session.state = state;
        diagnosticsRuntime = { setDiagnostics };
        if (!session.dirty && !session.persistedDoc) session.persistedDoc = state.doc;
        view = new EditorView({
          parent: host,
          state,
        });
        view.scrollDOM.scrollTop = session.scrollTop;
        applyDiagnostics(diagnostics);

        loading = false;
        onReady(editorHandle());
        if (focusWhenReady) focus();
      } catch (reason) {
        fallbackValue = documentValue(session);
        session.state = null;
        loading = false;
        loadError = reason instanceof Error ? reason.message : String(reason);
        onReady(editorHandle());
      }
    }

    void initialize();

    return () => {
      cancelled = true;
      if (view) {
        session.state = view.state;
        session.scrollTop = view.scrollDOM.scrollTop;
      }
      view?.destroy();
      view = null;
      diagnosticsRuntime = null;
      onReady(null);
    };
  });

  export function getValue(): string {
    return view?.state.doc.toString() ?? documentValue(session);
  }

  export function getSnapshot(): { value: string; revision: number } {
    return documentSnapshot(session);
  }

  export function setValue(value: string, origin: EditorTransactionOrigin = "disk") {
    if (view && view.state.doc.toString() !== value) {
      dispatchOrigin = origin;
      try {
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
      } finally {
        dispatchOrigin = "user";
      }
      return;
    }
    if (!view && documentValue(session) !== value) {
      const previousDirty = session.dirty;
      session.initialValue = value;
      session.persistedDoc = null;
      session.revision += 1;
      session.lastOrigin = origin;
      session.dirty = editorIsDirty(value, session.persistedValue);
      fallbackValue = value;
      if (previousDirty !== session.dirty) onDirtyChange(session.dirty);
      onChange(session.revision);
    }
  }

  export function markSaved(value = getValue()): boolean {
    const previousDirty = session.dirty;
    const dirty = markDocumentSaved(session, value);
    if (previousDirty !== dirty) onDirtyChange(dirty);
    return dirty;
  }

  export function focus() {
    if (!view) {
      if (fallback) fallback.focus();
      else focusWhenReady = true;
      return;
    }

    focusWhenReady = false;
    view.focus();
  }

  export function getCursorPosition(): { line: number; character: number } {
    if (!view) return { line: 0, character: 0 };
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return { line: line.number - 1, character: head - line.from };
  }

  export function setCursorPosition(lineNumber: number, character: number) {
    if (!view) return;
    const line = view.state.doc.line(Math.min(view.state.doc.lines, Math.max(1, lineNumber + 1)));
    const anchor = Math.min(line.to, line.from + Math.max(0, character));
    view.dispatch({ selection: { anchor }, scrollIntoView: true });
    view.focus();
  }

  export function applyTextEdits(edits: LspTextEdit[]): boolean {
    if (!view) return false;
    const changes = lspTextEditsToChanges(view.state.doc, edits);
    if (!changes) return false;
    view.dispatch({ changes });
    return true;
  }

  export function getSelectionSnapshot(): EditorSelectionSnapshot | null {
    if (view) {
      const selection = view.state.selection.main;
      if (selection.empty) return null;
      const start = offsetToLspPosition(view.state.doc, selection.from);
      const end = offsetToLspPosition(view.state.doc, selection.to);
      return {
        start_line: start.line,
        start_character: start.character,
        end_line: end.line,
        end_character: end.character,
        text: view.state.sliceDoc(selection.from, selection.to),
      };
    }
    if (!fallback || fallback.selectionStart === fallback.selectionEnd) return null;
    const start = stringOffsetPosition(fallbackValue, fallback.selectionStart);
    const end = stringOffsetPosition(fallbackValue, fallback.selectionEnd);
    return {
      start_line: start.line,
      start_character: start.character,
      end_line: end.line,
      end_character: end.character,
      text: fallbackValue.slice(fallback.selectionStart, fallback.selectionEnd),
    };
  }

  function editorHandle(): CodeEditorHandle {
    return {
      getValue,
      getSnapshot,
      setValue,
      markSaved,
      focus,
      getCursorPosition,
      setCursorPosition,
      applyTextEdits,
      getSelectionSnapshot,
    };
  }

  let runtimePromise: Promise<Runtime> | null = null;

  function loadRuntime(): Promise<Runtime> {
    runtimePromise ??= Promise.all([
      import("codemirror"),
      import("@codemirror/state"),
      import("@codemirror/view"),
      import("@codemirror/commands"),
      import("@codemirror/language"),
      import("@lezer/highlight"),
      import("@replit/codemirror-vim"),
      import("@codemirror/lint"),
      import("@codemirror/autocomplete"),
    ]).then(
      ([codemirror, state, view, commands, language, highlight, vim, lint, autocomplete]) => ({
        basicSetup: codemirror.basicSetup,
        EditorState: state.EditorState,
        StateEffect: state.StateEffect,
        EditorView: view.EditorView,
        keymap: view.keymap,
        hoverTooltip: view.hoverTooltip,
        indentWithTab: commands.indentWithTab,
        HighlightStyle: language.HighlightStyle,
        syntaxHighlighting: language.syntaxHighlighting,
        tags: highlight.tags,
        vim: vim.vim,
        lintGutter: lint.lintGutter,
        setDiagnostics: lint.setDiagnostics,
        autocompletion: autocomplete.autocompletion,
        snippet: autocomplete.snippet,
      }),
    );
    return runtimePromise;
  }

  function softDraculaHighlight(
    runtime: Pick<Runtime, "HighlightStyle" | "syntaxHighlighting" | "tags">,
  ): Extension {
    const { HighlightStyle, syntaxHighlighting, tags } = runtime;

    return syntaxHighlighting(
      HighlightStyle.define([
        { tag: tags.keyword, color: "#b89adf" },
        { tag: [tags.atom, tags.bool, tags.null], color: "#c9a7e8" },
        { tag: [tags.string, tags.special(tags.string)], color: "#9dbb83" },
        { tag: [tags.number, tags.integer, tags.float], color: "#d8a789" },
        {
          tag: [tags.comment, tags.lineComment, tags.blockComment],
          color: "#6f7386",
          fontStyle: "italic",
        },
        { tag: tags.docComment, color: "#858aa0", fontStyle: "italic" },
        { tag: tags.variableName, color: "#d8d2e4" },
        { tag: tags.definition(tags.variableName), color: "#e2ddee" },
        { tag: tags.function(tags.variableName), color: "#8db9d6" },
        { tag: [tags.propertyName, tags.attributeName], color: "#91c3d0" },
        { tag: [tags.className, tags.typeName, tags.namespace], color: "#d9c58d" },
        {
          tag: [tags.operator, tags.compareOperator, tags.logicOperator, tags.arithmeticOperator],
          color: "#aaa4bb",
        },
        { tag: [tags.punctuation, tags.separator, tags.bracket], color: "#8f89a0" },
        { tag: tags.link, color: "#8db9d6", textDecoration: "underline" },
        { tag: tags.heading, color: "#b89adf", fontWeight: "700" },
        { tag: tags.invalid, color: "#e58a8a", backgroundColor: "rgba(229, 138, 138, 0.12)" },
      ]),
    );
  }

  function applyDiagnostics(values: LspDiagnostic[]) {
    if (!view || !diagnosticsRuntime) return;
    const mapped = values.flatMap((diagnostic) => {
      const startLine = Math.min(view!.state.doc.lines, diagnostic.range.start.line + 1);
      const endLine = Math.min(view!.state.doc.lines, diagnostic.range.end.line + 1);
      const start = view!.state.doc.line(startLine);
      const end = view!.state.doc.line(endLine);
      const from = Math.min(start.to, start.from + diagnostic.range.start.character);
      const to = Math.max(from, Math.min(end.to, end.from + diagnostic.range.end.character));
      return [
        {
          from,
          to,
          severity:
            diagnostic.severity === 1
              ? ("error" as const)
              : diagnostic.severity === 2
                ? ("warning" as const)
                : diagnostic.severity === 4
                  ? ("hint" as const)
                  : ("info" as const),
          message: diagnostic.message,
          source: diagnostic.source ?? undefined,
        },
      ];
    });
    view.dispatch(diagnosticsRuntime.setDiagnostics(view.state, mapped));
  }

  async function loadLanguage(filePath: string): Promise<Extension | null> {
    return editorLanguagePluginForPath(filePath)?.load() ?? null;
  }

  function editorTheme(EditorView: Runtime["EditorView"]): Extension {
    return EditorView.theme(
      {
        "&": {
          height: "100%",
          backgroundColor: "#191820",
          color: "#d8d2e4",
          fontSize: "13px",
        },
        ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55" },
        ".cm-content": { padding: "18px 0" },
        ".cm-line": { padding: "0 20px" },
        ".cm-gutters": {
          backgroundColor: "#15141b",
          borderRight: "1px solid #282631",
          color: "#706a84",
        },
        ".cm-activeLine": { backgroundColor: "rgba(80, 74, 96, 0.22)" },
        ".cm-activeLineGutter": {
          backgroundColor: "rgba(80, 74, 96, 0.28)",
          color: "#a99bd6",
        },
        ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
          backgroundColor: "rgba(153, 131, 196, 0.26)",
        },
        "&.cm-focused": { outline: "none" },
        ".cm-panels-bottom": {
          borderTop: "1px solid #282631",
          backgroundColor: "#111016",
          color: "#a99bd6",
          fontFamily: "var(--font-mono)",
          fontSize: "11px",
          fontWeight: "800",
        },
        ".cm-vim-panel input": {
          backgroundColor: "transparent",
          color: "#d8d2e4",
          font: "inherit",
        },
      },
      { dark: true },
    );
  }

  function insertFallbackIndent(textarea: HTMLTextAreaElement, outdent: boolean) {
    if (outdent) return;

    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    textarea.setRangeText("  ", start, end, "end");
    setValue(textarea.value);
  }

  function stripSnippetPlaceholders(value: string): string {
    return value
      .replace(/\$\{\d+:([^}]*)\}/g, "$1")
      .replace(/\$\{\d+\}/g, "")
      .replace(/\$\d+/g, "");
  }

  function stringOffsetPosition(value: string, offset: number) {
    const clamped = Math.max(0, Math.min(value.length, offset));
    const prefix = value.slice(0, clamped);
    const line = prefix.split("\n").length - 1;
    const lineStart = prefix.lastIndexOf("\n") + 1;
    return { line, character: clamped - lineStart };
  }
</script>

<div class="code-editor-shell" aria-busy={loading} aria-label={`Code editor for ${session.path}`}>
  <div class:is-hidden={Boolean(loadError)} class="code-editor-host" bind:this={host}></div>
  {#if loading}
    <div class="code-editor-loading">Loading syntax highlighter...</div>
  {:else if loadError}
    <textarea
      bind:this={fallback}
      aria-label={`Plain editor fallback for ${session.path}`}
      class="code-editor-fallback"
      spellcheck="false"
      value={fallbackValue}
      oninput={(event) => setValue(event.currentTarget.value)}
      onkeydown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          onCommand("editor.save");
        } else if (
          (event.metaKey || event.ctrlKey) &&
          event.shiftKey &&
          event.key.toLowerCase() === "f"
        ) {
          event.preventDefault();
          onCommand("editor.format");
        } else if (event.key === "Tab") {
          event.preventDefault();
          insertFallbackIndent(event.currentTarget, event.shiftKey);
        }
      }}></textarea>
  {/if}
</div>

<style>
  .code-editor-shell {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: #191820;
  }

  .code-editor-host {
    width: 100%;
    height: 100%;
    min-height: 0;
  }

  .code-editor-host.is-hidden {
    display: none;
  }

  .code-editor-loading {
    position: absolute;
    inset: 0;
    display: grid;
    height: 100%;
    place-items: center;
    color: #b89adf;
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 800;
  }

  .code-editor-fallback {
    width: 100%;
    height: 100%;
    border: 0;
    padding: 18px 20px;
    resize: none;
    background: #191820;
    color: #d8d2e4;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1.55;
    outline: none;
    tab-size: 2;
    white-space: pre;
  }

  :global(.cm-tooltip .cm-lsp-hover) {
    max-width: min(620px, 70vw);
    max-height: 320px;
    overflow: auto;
    padding: 10px 12px;
    white-space: pre-wrap;
    color: #e6e0ee;
    background: #17151d;
    border: 1px solid #3a3448;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
  }
</style>
