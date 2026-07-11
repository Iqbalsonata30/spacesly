type PrettierModules = {
  format: (source: string, options: { parser: string; plugins: Array<string | URL | object> }) => Promise<string>;
  plugins: Array<string | URL | object>;
};

let prettierModulesPromise: Promise<PrettierModules> | null = null;

export function prettierParserForPath(path: string): string | null {
  const name = path.toLowerCase();
  if (name.endsWith(".ts") || name.endsWith(".tsx") || name.endsWith(".mts") || name.endsWith(".cts")) return "typescript";
  if (name.endsWith(".js") || name.endsWith(".jsx") || name.endsWith(".mjs") || name.endsWith(".cjs")) return "babel";
  if (name.endsWith(".svelte")) return null;
  if (name.endsWith(".html")) return "html";
  if (name.endsWith(".css") || name.endsWith(".scss") || name.endsWith(".sass") || name.endsWith(".less")) return "css";
  if (name.endsWith(".json") || name.endsWith(".jsonc")) return "json";
  if (name.endsWith(".md") || name.endsWith(".mdx")) return "markdown";
  if (name.endsWith(".yml") || name.endsWith(".yaml")) return "yaml";
  return null;
}

export async function formatEditorText(path: string, source: string): Promise<string> {
  const parser = prettierParserForPath(path);
  if (!parser) throw new Error(`No Prettier parser configured for ${path}.`);

  const prettier = await loadPrettierModules();
  return prettier.format(source, { parser, plugins: prettier.plugins });
}

export async function validateEditorSyntax(path: string, source: string): Promise<string | null> {
  const parser = prettierParserForPath(path);
  if (!parser || source.length > 200_000) return null;

  try {
    const prettier = await loadPrettierModules();
    await prettier.format(source, { parser, plugins: prettier.plugins });
    return null;
  } catch (reason: unknown) {
    return reason instanceof Error ? reason.message : String(reason);
  }
}

function loadPrettierModules(): Promise<PrettierModules> {
  if (prettierModulesPromise) return prettierModulesPromise;
  prettierModulesPromise = Promise.all([
    import("prettier/standalone"),
    import("prettier/plugins/babel"),
    import("prettier/plugins/estree"),
    import("prettier/plugins/html"),
    import("prettier/plugins/markdown"),
    import("prettier/plugins/postcss"),
    import("prettier/plugins/typescript"),
    import("prettier/plugins/yaml"),
  ]).then(([prettier, babel, estree, html, markdown, postcss, typescript, yaml]) => ({
    format: (source, options) => prettier.default.format(source, options),
    plugins: [babel, estree, html, markdown, postcss, typescript, yaml] as Array<string | URL | object>,
  }));
  return prettierModulesPromise;
}
