import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  testMatch: "skills-ui.spec.ts",
  use: { baseURL: "http://127.0.0.1:1420" },
  webServer: {
    command: "bun run dev --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
  },
});
