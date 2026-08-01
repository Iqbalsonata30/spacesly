import { expect, test } from "@playwright/test";

test("major navigation uses labeled, consistent icon controls", async ({ page }) => {
  await page.goto("/", { waitUntil: "networkidle" });

  const rawActionGlyphs = await page
    .locator("button")
    .evaluateAll((buttons) =>
      buttons
        .map((button) => button.textContent?.trim() ?? "")
        .filter((text) => /^[×⌄←→↻▾▸＋]/u.test(text)),
    );
  expect(rawActionGlyphs).toEqual([]);

  await page.getByRole("button", { name: "Settings" }).click();
  for (const tab of ["Agent", "Rules", "Skills", "MCP", "Jira", "Global Environment", "Theme"]) {
    await page.getByRole("tab", { name: new RegExp(`^${tab}`) }).click();
    await expect(page.getByRole("tab", { name: new RegExp(`^${tab}`) })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  }

  await page.getByRole("tab", { name: /^Skills/ }).click();
  await page.getByRole("button", { name: "New Skill" }).click();
  const closeSkill = page.getByRole("button", { name: "Close skill editor" });
  await expect(closeSkill.locator("svg")).toHaveCount(1);
  await closeSkill.click();

  const unnamedIconButtons = await page
    .locator("button:has(svg)")
    .evaluateAll((buttons) =>
      buttons
        .filter((button) => !(button.getAttribute("aria-label") || button.textContent?.trim()))
        .map((button) => button.outerHTML),
    );
  expect(unnamedIconButtons).toEqual([]);
});
