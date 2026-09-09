import { expect, test } from "@playwright/test";

test("conversation scroller shows the messages below the viewport", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Fifteen turns", exact: true }).click();

  const viewport = page.locator('[data-slot="message-scroller-viewport"]');
  await expect(viewport).toBeVisible();
  await viewport.hover();
  await page.mouse.wheel(0, -500);
  await page.waitForTimeout(150);

  const latestPill = page.getByRole("button", {
    name: /Scroll to latest messages, \d+ messages below/,
  });
  await expect(latestPill).toBeVisible();
  await expect(latestPill).toContainText(/\d+ messages/);

  await latestPill.click();
  await expect(latestPill).toBeHidden();
});
