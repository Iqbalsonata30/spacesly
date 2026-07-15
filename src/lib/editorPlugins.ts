import type { Extension } from "@codemirror/state";

export type EditorLanguagePlugin = {
  id: string;
  label: string;
  extensions: string[];
  load: () => Promise<Extension | null>;
  formatter?: "prettier" | "rustfmt" | "gofmt";
};

const plugins: EditorLanguagePlugin[] = [];

export function registerEditorLanguagePlugin(plugin: EditorLanguagePlugin) {
  const index = plugins.findIndex((entry) => entry.id === plugin.id);
  if (index === -1) {
    plugins.push(plugin);
  } else {
    plugins[index] = plugin;
  }
}

export function editorLanguagePluginForPath(path: string): EditorLanguagePlugin | null {
  const name = path.toLowerCase();
  return (
    plugins.find((plugin) => plugin.extensions.some((extension) => name.endsWith(extension))) ??
    null
  );
}

export function editorFormatterForPath(path: string): EditorLanguagePlugin["formatter"] | null {
  return editorLanguagePluginForPath(path)?.formatter ?? null;
}

export function editorLanguagePlugins(): EditorLanguagePlugin[] {
  return [...plugins];
}

registerEditorLanguagePlugin({
  id: "svelte",
  label: "Svelte",
  extensions: [".svelte"],
  load: async () => {
    const { svelte } = await import("@replit/codemirror-lang-svelte");
    return svelte();
  },
});

registerEditorLanguagePlugin({
  id: "typescript",
  label: "TypeScript",
  extensions: [".ts", ".tsx", ".mts", ".cts"],
  formatter: "prettier",
  load: async () => {
    const { javascript } = await import("@codemirror/lang-javascript");
    return javascript({ typescript: true, jsx: true });
  },
});

registerEditorLanguagePlugin({
  id: "javascript",
  label: "JavaScript",
  extensions: [".js", ".jsx", ".mjs", ".cjs"],
  formatter: "prettier",
  load: async () => {
    const { javascript } = await import("@codemirror/lang-javascript");
    return javascript({ jsx: true });
  },
});

registerEditorLanguagePlugin({
  id: "json",
  label: "JSON",
  extensions: [".json", ".jsonc"],
  formatter: "prettier",
  load: async () => {
    const { json } = await import("@codemirror/lang-json");
    return json();
  },
});

registerEditorLanguagePlugin({
  id: "html",
  label: "HTML",
  extensions: [".html", ".htm"],
  formatter: "prettier",
  load: async () => {
    const { html } = await import("@codemirror/lang-html");
    return html();
  },
});

registerEditorLanguagePlugin({
  id: "css",
  label: "CSS",
  extensions: [".css", ".scss", ".sass", ".less"],
  formatter: "prettier",
  load: async () => {
    const { css } = await import("@codemirror/lang-css");
    return css();
  },
});

registerEditorLanguagePlugin({
  id: "markdown",
  label: "Markdown",
  extensions: [".md", ".mdx"],
  formatter: "prettier",
  load: async () => {
    const { markdown } = await import("@codemirror/lang-markdown");
    return markdown();
  },
});

registerEditorLanguagePlugin({
  id: "yaml",
  label: "YAML",
  extensions: [".yml", ".yaml"],
  formatter: "prettier",
  load: async () => {
    const { yaml } = await import("@codemirror/lang-yaml");
    return yaml();
  },
});

registerEditorLanguagePlugin({
  id: "rust",
  label: "Rust",
  extensions: [".rs"],
  formatter: "rustfmt",
  load: async () => {
    const { rust } = await import("@codemirror/lang-rust");
    return rust();
  },
});

registerEditorLanguagePlugin({
  id: "go",
  label: "Go",
  extensions: [".go"],
  formatter: "gofmt",
  load: async () => {
    const { go } = await import("@codemirror/lang-go");
    return go();
  },
});
