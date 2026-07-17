import type { LspServerConfig } from "$lib/ipc";

export function lspConfigForPath(path: string): LspServerConfig | null {
  const name = path.toLowerCase();
  if (name.endsWith(".rs")) {
    return config("rust-analyzer", "rust-analyzer", [], "rust", [".rs"], ["Cargo.toml"]);
  }
  if (name.endsWith(".go")) {
    return config("gopls", "gopls", [], "go", [".go"], ["go.mod"]);
  }
  const typescript = [".ts", ".mts", ".cts"].some((extension) => name.endsWith(extension));
  const typescriptReact = name.endsWith(".tsx");
  const javascript = [".js", ".mjs", ".cjs"].some((extension) => name.endsWith(extension));
  const javascriptReact = name.endsWith(".jsx");
  if (typescript || typescriptReact || javascript || javascriptReact) {
    return config(
      "typescript-language-server",
      "typescript-language-server",
      ["--stdio"],
      typescriptReact
        ? "typescriptreact"
        : typescript
          ? "typescript"
          : javascriptReact
            ? "javascriptreact"
            : "javascript",
      [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"],
      ["tsconfig.json", "jsconfig.json", "package.json"],
    );
  }
  return null;
}

function config(
  serverId: string,
  command: string,
  args: string[],
  languageId: string,
  extensions: string[],
  projectRoots: string[],
): LspServerConfig {
  return {
    server_id: serverId,
    command,
    args,
    extensions,
    detect_files: projectRoots,
    language_id: languageId,
    install_hint: `Install ${command} and ensure it is available in PATH.`,
    project_roots: projectRoots,
  };
}
