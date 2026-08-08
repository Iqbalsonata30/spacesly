import { expect, test } from "@playwright/test";

test("a stalled approval can be retried when the task is blocked", async ({ page }) => {
  await page.goto("/__approval");

  const approve = page.getByRole("button", { name: "Approve & Continue" });
  await expect(approve).toBeEnabled();
  await approve.click();

  await expect(page.locator("#approval-clicks")).toHaveText("1");
  await expect(page.getByText("Approval required")).toHaveCount(0);
});
