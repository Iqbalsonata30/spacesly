import { expect, test } from "@playwright/test";

test("approving hides the card and projects the resumed task as running", async ({ page }) => {
  await page.goto("/__approval");

  const approve = page.getByRole("button", { name: "Approve & Continue" });
  await expect(approve).toBeEnabled();
  await approve.click();

  await expect(page.locator("#approval-clicks")).toHaveText("1");
  await expect(page.getByText("Approval required")).toHaveCount(0);
  await expect(page.locator("#task-status")).toHaveText("running");
  await expect(page.getByText("Working", { exact: true })).toBeVisible();
  await expect(page.getByText("Needs your attention", { exact: true })).toHaveCount(0);
});
