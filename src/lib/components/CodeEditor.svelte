<script lang="ts">
  import { onMount } from "svelte";
  import type { Extension } from "@codemirror/state";
  import type { EditorView as EditorViewType } from "@codemirror/view";
  import { editorLanguagePluginForPath } from "$lib/editorPlugins";

  type Runtime = {
    basicSetup: Extension;
    EditorState: typeof import("@codemirror/state").EditorState;
    EditorView: typeof import("@codemirror/view").EditorView;
    keymap: typeof import("@codemirror/view").keymap;
    indentWithTab: typeof import("@codemirror/commands").indentWithTab;
    HighlightStyle: typeof import("@codemirror/language").HighlightStyle;
    syntaxHighlighting: typeof import("@codemirror/language").syntaxHighlighting;
    tags: typeof import("@lezer/highlight").tags;
    vim: typeof import("@replit/codemirror-vim").vim;
  };

  type Props = {
    path: string;
    initialValue: string;
    onDirtyChange?: (dirty: boolean) => void;
    onSave?: () => void;
    onFormat?: () => void;
  };

  let {
    path,
    initialValue,
    onDirtyChange = () => {},
    onSave = () => {},
    onFormat = () => {},
  }: Props = $props();

  let host: HTMLDivElement | null = $state(null);
  let view: EditorViewType | null = null;
  let currentValue = $state("");
  let savedValue = $state("");
  let dirty = $state(false);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let focusWhenReady = false;

  onMount(() => {
    let cancelled = false;

    currentValue = initialValue;
    savedValue = initialValue;
    dirty = false;

    async function initialize() {
      if (!host) return;

      try {
        const [
          {
            basicSetup,
            EditorState,
            EditorView,
            keymap,
            indentWithTab,
            HighlightStyle,
            syntaxHighlighting,
            tags,
            vim,
          },
          language,
        ] = await Promise.all([loadRuntime(), loadLanguage(path)]);
        if (cancelled || !host) return;

        view = new EditorView({
          parent: host,
          state: EditorState.create({
            doc: currentValue,
            extensions: [
              basicSetup,
              vim({ status: true }),
              ...(language ? [language] : []),
              softDraculaHighlight({ HighlightStyle, syntaxHighlighting, tags }),
              keymap.of([
                indentWithTab,
                {
                  key: "Mod-s",
                  run: () => {
                    onSave();
                    return true;
                  },
                },
                {
                  key: "Mod-Shift-f",
                  run: () => {
                    onFormat();
                    return true;
                  },
                },
              ]),
              EditorView.updateListener.of((update) => {
                if (!update.docChanged) return;
                currentValue = update.state.doc.toString();
                updateDirty();
              }),
              EditorView.theme(
                {
                  "&": {
                    height: "100%",
                    backgroundColor: "#191820",
                    color: "#d8d2e4",
                    fontSize: "13px",
                  },
                  ".cm-scroller": {
                    fontFamily: "var(--font-mono)",
                    lineHeight: "1.55",
                  },
                  ".cm-content": {
                    padding: "18px 0",
                  },
                  ".cm-line": {
                    padding: "0 20px",
                  },
                  ".cm-gutters": {
                    backgroundColor: "#15141b",
                    borderRight: "1px solid #282631",
                    color: "#706a84",
                  },
                  ".cm-activeLine": {
                    backgroundColor: "rgba(80, 74, 96, 0.22)",
                  },
                  ".cm-activeLineGutter": {
                    backgroundColor: "rgba(80, 74, 96, 0.28)",
                    color: "#a99bd6",
                  },
                  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
                    backgroundColor: "rgba(153, 131, 196, 0.26)",
                  },
                  "&.cm-focused": {
                    outline: "none",
                  },
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
              ),
            ],
          }),
        });

        loading = false;
        if (focusWhenReady) focus();
      } catch (reason) {
        loading = false;
        loadError = reason instanceof Error ? reason.message : String(reason);
      }
    }

    void initialize();

    return () => {
      cancelled = true;
      view?.destroy();
      view = null;
    };
  });

  export function getValue(): string {
    return view?.state.doc.toString() ?? currentValue;
  }

  export function setValue(value: string) {
    currentValue = value;
    if (view && view.state.doc.toString() !== value) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
    }
    updateDirty();
  }

  export function markSaved(value = getValue()) {
    savedValue = value;
    currentValue = value;
    updateDirty();
  }

  export function focus() {
    if (!view) {
      focusWhenReady = true;
      return;
    }

    focusWhenReady = false;
    view.focus();
  }

  function updateDirty() {
    const nextDirty = currentValue !== savedValue;
    if (dirty === nextDirty) return;
    dirty = nextDirty;
    onDirtyChange(dirty);
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
    ]).then(([codemirror, state, view, commands, language, highlight, vim]) => ({
      basicSetup: codemirror.basicSetup,
      EditorState: state.EditorState,
      EditorView: view.EditorView,
      keymap: view.keymap,
      indentWithTab: commands.indentWithTab,
      HighlightStyle: language.HighlightStyle,
      syntaxHighlighting: language.syntaxHighlighting,
      tags: highlight.tags,
      vim: vim.vim,
    }));
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

  async function loadLanguage(filePath: string): Promise<Extension | null> {
    return editorLanguagePluginForPath(filePath)?.load() ?? null;
  }

  function insertFallbackIndent(textarea: HTMLTextAreaElement, outdent: boolean) {
    if (outdent) return;

    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    textarea.setRangeText("  ", start, end, "end");
    setValue(textarea.value);
  }
</script>

<div class="code-editor-shell" aria-busy={loading} aria-label={`Code editor for ${path}`}>
  <div class:is-hidden={Boolean(loadError)} class="code-editor-host" bind:this={host}></div>
  {#if loading}
    <div class="code-editor-loading">Loading syntax highlighter...</div>
  {:else if loadError}
    <textarea
      aria-label={`Plain editor fallback for ${path}`}
      class="code-editor-fallback"
      spellcheck="false"
      value={currentValue}
      oninput={(event) => setValue(event.currentTarget.value)}
      onkeydown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          onSave();
        } else if (
          (event.metaKey || event.ctrlKey) &&
          event.shiftKey &&
          event.key.toLowerCase() === "f"
        ) {
          event.preventDefault();
          onFormat();
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
</style>
