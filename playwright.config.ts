import { defineConfig } from "@playwright/test";

const chromiumExecutablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;

export default defineConfig({
  testDir: "./tests",
  testMatch: [
    "skills-ui.spec.ts",
    "theme-ui.spec.ts",
    "icons-ui.spec.ts",
    "agent-rules-ui.spec.ts",
    "approval-ui.spec.ts",
  ],
  use: {
    baseURL: "http://127.0.0.1:1420",
    launchOptions: chromiumExecutablePath ? { executablePath: chromiumExecutablePath } : undefined,
  },
  webServer: {
    command: "bun run dev --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
  },
});
