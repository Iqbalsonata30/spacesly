import { test, expect } from "@playwright/test";

test("new and edit skill through rendered UI", async ({ page }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.goto("/", { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("tab", { name: /Skills/ }).click({ noWaitAfter: true });
  await page.waitForTimeout(500);

  const newButton = page.getByRole("button", { name: /New Skill/ });
  await expect(newButton).toBeVisible();
  await newButton.click();

  const createDialog = page.getByRole("dialog", { name: "Create skill" });
  await expect(createDialog).toBeVisible();
  await createDialog.getByLabel("Name").fill("UI Verification Skill");
  await createDialog.getByLabel("Description").fill("Created through browser automation.");
  await createDialog
    .getByLabel("Instructions")
    .fill("Verify that New Skill creates and persists an editable Skill.");
  await createDialog.getByRole("button", { name: "Create skill" }).click();

  const skillCard = page.locator("article").filter({ hasText: "UI Verification Skill" });
  await expect(skillCard).toBeVisible();
  await skillCard.getByRole("button", { name: "Edit configuration" }).click();

  const editDialog = page.getByRole("dialog", { name: "Edit UI Verification Skill" });
  await expect(editDialog).toBeVisible();
  await editDialog.getByLabel("Description").fill("Edited through browser automation.");
  await editDialog.getByRole("button", { name: "Save changes" }).click();
  await expect(skillCard).toContainText("Edited through browser automation.");

  expect(pageErrors).toEqual([]);
});
