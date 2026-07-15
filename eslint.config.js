import js from "@eslint/js";
import tseslint from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import globals from "globals";

export default [
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...svelte.configs["flat/recommended"],

  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node,
      },
    },
  },

  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
      },
    },
  },

  {
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },

  {
    files: ["**/+page.svelte"],
    rules: {
      "@typescript-eslint/no-unused-vars": "off",
      "svelte/no-navigation-without-resolve": "off",
    },
  },

  {
    files: ["**/components/TaskCard.svelte"],
    rules: {
      "svelte/no-navigation-without-resolve": "off",
    },
  },

  {
    ignores: ["build", ".svelte-kit", "dist", "node_modules", "src-tauri/target/**"],
  },
];
