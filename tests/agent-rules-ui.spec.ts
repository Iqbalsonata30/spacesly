import { expect, test } from "@playwright/test";

const settingsKey = "spacesly.settings.v1";

async function openRules(page: import("@playwright/test").Page) {
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("tab", { name: /^Rules/ }).click();
  await expect(page.getByRole("heading", { name: "Agent Rules", level: 3 })).toBeVisible();
}

test("Agent Rules supports save, preview, validation, and guarded navigation", async ({ page }) => {
  await page.goto("/", { waitUntil: "networkidle" });
  await openRules(page);

  await expect(page.getByText("Applied to every run", { exact: true })).toBeVisible();
  await expect(page.getByText("Before task instructions", { exact: true })).toBeVisible();
  await expect(page.getByText("All Agent actions", { exact: true })).toBeVisible();

  const editor = page.getByRole("textbox", { name: "Rules applied to every run" });
  const original = await editor.inputValue();
  const duplicate = original.split("\n").find((line) => line.trim())!;
  await expect(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
  await editor.fill(`${original}\n${duplicate}`);
  await expect(page.getByText("Unsaved changes", { exact: true })).toBeVisible();
  await expect(page.getByText(/is identical to rule/)).toBeVisible();

  await page.getByRole("tab", { name: "Preview" }).click();
  await expect(page.getByRole("tabpanel", { name: "Rules preview" }).locator("li")).not.toHaveCount(
    0,
  );
  await page.getByRole("tab", { name: "Edit" }).click();
  await editor.press("Control+s");
  await expect(page.getByText("Agent Rules saved", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
  await expect(page.getByRole("dialog", { name: "Agent Rules" })).toBeVisible();
  const persisted = await page.evaluate(
    (key) => JSON.parse(localStorage.getItem(key)!),
    settingsKey,
  );
  expect(persisted.aiWorker.agentRules).toBe(`${original}\n${duplicate}`);

  await editor.fill(`${original}\nAsk before publishing releases.`);
  await page.getByRole("tab", { name: /^Skills/ }).click();
  const discardDialog = page.getByRole("dialog", { name: "Discard unsaved changes?" });
  await expect(discardDialog).toBeVisible();
  await discardDialog.getByRole("button", { name: "Keep editing" }).click();
  await expect(editor).toHaveValue(`${original}\nAsk before publishing releases.`);

  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(discardDialog).toBeVisible();
  await discardDialog.getByRole("button", { name: "Keep editing" }).click();

  await page.getByRole("tab", { name: /^Skills/ }).click();
  await discardDialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.getByRole("tab", { name: /^Skills/ })).toHaveAttribute("aria-selected", "true");
});

test("Agent Rules preserves drafts after save failure and fits compact layouts", async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 700 });
  await page.goto("/", { waitUntil: "networkidle" });
  await openRules(page);

  const editor = page.getByRole("textbox", { name: "Rules applied to every run" });
  const failedDraft = `${await editor.inputValue()}\nRequire verification before completion.`;
  await editor.fill(failedDraft);
  await page.evaluate((key) => {
    const originalSetItem = Storage.prototype.setItem;
    (window as typeof window & { restoreSettingsStorage?: () => void }).restoreSettingsStorage =
      () => {
        Storage.prototype.setItem = originalSetItem;
      };
    Storage.prototype.setItem = function (name, value) {
      if (name === key) throw new Error("Storage unavailable");
      return originalSetItem.call(this, name, value);
    };
  }, settingsKey);

  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Couldn’t save Agent Rules", { exact: true })).toBeVisible();
  await expect(page.getByText("Storage unavailable", { exact: true })).toBeVisible();
  await expect(editor).toHaveValue(failedDraft);
  await expect(page.getByRole("button", { name: "Try again" })).toBeEnabled();
  await page.evaluate(() => {
    (window as typeof window & { restoreSettingsStorage?: () => void }).restoreSettingsStorage?.();
  });

  const panel = await page.locator(".settings-panel").boundingBox();
  expect(panel).not.toBeNull();
  expect(panel!.width).toBeLessThanOrEqual(760);
  await expect(page.locator(".rules-action-footer")).toBeVisible();
});
