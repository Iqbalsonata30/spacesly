// @ts-nocheck -- this file is executed by Bun's test runtime, not the app TypeScript environment.
import { test } from "bun:test";

test("themes", async () => {
  await import("./themes");
});

test("icons", async () => {
  await import("./icons");
});

test("agent rules", async () => {
  await import("./agentRules");
});

test("agent skills", async () => {
  await import("./agentSkills");
});

test("editor document", async () => {
  await import("./editorDocument");
});

test("task sessions", async () => {
  await import("./taskSessions");
});
