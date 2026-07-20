import { formatCode } from "$lib/ipc";
import { editorFormatterForPath } from "$lib/editorPlugins";

type PrettierModules = {
  format: (
    source: string,
    options: { parser: string; plugins: Array<string | URL | object> },
  ) => Promise<string>;
  plugins: Array<string | URL | object>;
};

type PrettierPlugin = string | URL | object;

let prettierPromise: Promise<typeof import("prettier/standalone")> | null = null;
const prettierPluginPromises = new Map<string, Promise<PrettierPlugin[]>>();

export function prettierParserForPath(path: string): string | null {
  const name = path.toLowerCase();
  if (editorFormatterForPath(path) && editorFormatterForPath(path) !== "prettier") return null;
  if (
    name.endsWith(".ts") ||
    name.endsWith(".tsx") ||
    name.endsWith(".mts") ||
    name.endsWith(".cts")
  )
    return "typescript";
  if (
    name.endsWith(".js") ||
    name.endsWith(".jsx") ||
    name.endsWith(".mjs") ||
    name.endsWith(".cjs")
  )
    return "babel";
  if (name.endsWith(".svelte")) return null;
  if (name.endsWith(".html")) return "html";
  if (
    name.endsWith(".css") ||
    name.endsWith(".scss") ||
    name.endsWith(".sass") ||
    name.endsWith(".less")
  )
    return "css";
  if (name.endsWith(".json") || name.endsWith(".jsonc")) return "json";
  if (name.endsWith(".md") || name.endsWith(".mdx")) return "markdown";
  if (name.endsWith(".yml") || name.endsWith(".yaml")) return "yaml";
  return null;
}

export async function formatEditorText(path: string, source: string): Promise<string> {
  const formatter = editorFormatterForPath(path);
  if (formatter === "rustfmt" || formatter === "gofmt") {
    return formatCode(formatter, source);
  }

  const parser = prettierParserForPath(path);
  if (!parser) throw new Error(`No Prettier parser configured for ${path}.`);

  const prettier = await loadPrettierModules(parser);
  return prettier.format(source, { parser, plugins: prettier.plugins });
}

export async function validateEditorSyntax(path: string, source: string): Promise<string | null> {
  const parser = prettierParserForPath(path);
  if (!parser || source.length > 200_000) return null;

  try {
    const prettier = await loadPrettierModules(parser);
    await prettier.format(source, { parser, plugins: prettier.plugins });
    return null;
  } catch (reason: unknown) {
    return reason instanceof Error ? reason.message : String(reason);
  }
}

export function prettierPluginGroupForParser(parser: string): string {
  if (parser === "typescript") return "typescript";
  if (parser === "babel" || parser === "json") return "babel";
  if (parser === "html") return "html";
  if (parser === "css") return "postcss";
  if (parser === "markdown") return "markdown";
  if (parser === "yaml") return "yaml";
  throw new Error(`Unsupported Prettier parser: ${parser}.`);
}

async function loadPrettierModules(parser: string): Promise<PrettierModules> {
  prettierPromise ??= import("prettier/standalone");
  const [prettier, plugins] = await Promise.all([prettierPromise, loadPrettierPlugins(parser)]);
  return {
    format: (source, options) => prettier.format(source, options),
    plugins,
  };
}

function loadPrettierPlugins(parser: string): Promise<PrettierPlugin[]> {
  const group = prettierPluginGroupForParser(parser);
  const cached = prettierPluginPromises.get(group);
  if (cached) return cached;

  let plugins: Promise<PrettierPlugin[]>;
  if (group === "typescript") {
    plugins = Promise.all([
      import("prettier/plugins/typescript"),
      import("prettier/plugins/estree"),
    ]);
  } else if (group === "babel") {
    plugins = Promise.all([import("prettier/plugins/babel"), import("prettier/plugins/estree")]);
  } else if (group === "html") {
    plugins = import("prettier/plugins/html").then((plugin) => [plugin]);
  } else if (group === "postcss") {
    plugins = import("prettier/plugins/postcss").then((plugin) => [plugin]);
  } else if (group === "markdown") {
    plugins = import("prettier/plugins/markdown").then((plugin) => [plugin]);
  } else {
    plugins = import("prettier/plugins/yaml").then((plugin) => [plugin]);
  }
  prettierPluginPromises.set(group, plugins);
  return plugins;
}
