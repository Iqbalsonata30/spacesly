import { expect, test } from "@playwright/test";

test.use({ colorScheme: "light" });

test("light styling stays scoped while system follows the OS", async ({ page }) => {
  await page.goto("/", { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("tab", { name: /Theme/ }).click();

  await page.getByRole("radio", { name: /Light/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme-mode", "light");
  await expect(page.locator("html")).toHaveAttribute("data-resolved-theme", "light");
  const lightStyles = await page.evaluate(() => ({
    accent: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(),
    stageBackground: getComputedStyle(document.querySelector(".stage")!).backgroundImage,
    titlebarBackground: getComputedStyle(document.querySelector(".titlebar")!).backgroundColor,
    buttonRadius: getComputedStyle(document.querySelector(".icon-button")!).borderRadius,
  }));
  expect(lightStyles).toEqual({
    accent: "#4563bf",
    stageBackground:
      "radial-gradient(circle at 12% 15%, rgba(165, 197, 255, 0.32), rgba(0, 0, 0, 0) 36%), radial-gradient(circle at 88% 88%, rgba(242, 190, 238, 0.24), rgba(0, 0, 0, 0) 34%), linear-gradient(135deg, rgb(237, 243, 252) 0%, rgb(247, 249, 253) 52%, rgb(248, 242, 252) 100%)",
    titlebarBackground: "rgba(255, 255, 255, 0.68)",
    buttonRadius: "11px",
  });
  await page.setViewportSize({ width: 760, height: 700 });
  const compactPanel = await page.locator(".settings-panel").boundingBox();
  expect(compactPanel).not.toBeNull();
  expect(compactPanel!.x).toBeGreaterThanOrEqual(0);
  expect(compactPanel!.width).toBeLessThanOrEqual(760);
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect(page.locator(".settings-panel")).toBeVisible();

  await page.getByRole("radio", { name: /Dark/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-resolved-theme", "dark");
  const darkStyles = await page.evaluate(() => ({
    background: getComputedStyle(document.documentElement).getPropertyValue("--bg-base").trim(),
    stageBackground: getComputedStyle(document.querySelector(".stage")!).backgroundImage,
    buttonRadius: getComputedStyle(document.querySelector(".icon-button")!).borderRadius,
  }));
  expect(darkStyles).toEqual({
    background: "#111016",
    stageBackground: "none",
    buttonRadius: "8px",
  });

  await page.getByRole("radio", { name: /System/ }).check();
  await expect(page.locator("html")).toHaveAttribute("data-theme-mode", "system");
  await expect(page.locator("html")).toHaveAttribute("data-resolved-theme", "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(page.locator("html")).toHaveAttribute("data-resolved-theme", "dark");
});
