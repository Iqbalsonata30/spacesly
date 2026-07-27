import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore optional Node types may be present through tooling dependencies
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// Vendor chunk definitions for manual code splitting.
// These are the heavy libraries that should only be loaded when their pane is opened.
const VENDOR_EDITOR = new Set([
  "codemirror",
  "@codemirror/state",
  "@codemirror/view",
  "@codemirror/commands",
  "@codemirror/language",
  "@codemirror/lint",
  "@codemirror/autocomplete",
  "@codemirror/lang-javascript",
  "@codemirror/lang-css",
  "@codemirror/lang-html",
  "@codemirror/lang-json",
  "@codemirror/lang-markdown",
  "@codemirror/lang-rust",
  "@codemirror/lang-go",
  "@codemirror/lang-yaml",
  "@replit/codemirror-lang-svelte",
  "@replit/codemirror-vim",
]);

const VENDOR_TERMINAL = new Set(["@xterm/xterm", "@xterm/addon-fit"]);

/** @param {string} id */
function manualChunks(id) {
  if (id.includes("node_modules")) {
    const pkg = id.split("node_modules/").pop()?.split("/")[0] ?? "";
    const scoped = id.includes("node_modules/@")
      ? "@" + (id.split("node_modules/@").pop()?.split("/")[0] ?? "")
      : null;
    const name = scoped ?? pkg;

    if (VENDOR_EDITOR.has(name)) return "vendor-editor";
    if (VENDOR_TERMINAL.has(name)) return "vendor-terminal";
    if (name === "lucide-svelte") return "vendor-icons";
  }
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [sveltekit()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // Target modern JS — reduces output size and avoids legacy polyfills.
    // Tauri's embedded WebKit supports all ES2022+ features.
    target: "esnext",

    // Split CSS per chunk — editor and terminal CSS only loads when those panels open.
    cssCodeSplit: true,

    // Skip the gzip size report — irrelevant for a Tauri desktop app (no network transfer).
    reportCompressedSize: false,

    rollupOptions: {
      output: {
        // CodeMirror (~600 KB) and xterm.js (~400 KB) are only needed when the editor or
        // terminal pane opens. Splitting them into lazy chunks reduces the initial parse
        // and evaluation cost for users who only use the board view.
        manualChunks,
      },
    },
  },
}));
