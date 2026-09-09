import { expect, test } from "@playwright/test";

test("linked pull requests can be found, filtered, and reviewed", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Pull requests", exact: true }).click();
  const panel = page.getByRole("region", { name: "Pull requests", exact: true });
  await expect(panel.getByRole("link", { name: "Bot CI", exact: true })).toHaveCount(1);
  await panel.getByRole("button", { name: "Merged", exact: true }).click();
  await expect(panel.getByText("No matching pull requests")).toBeVisible();
  await panel.getByRole("button", { name: "Open", exact: true }).click();
  await panel.getByRole("textbox").fill("missing title");
  await expect(panel.getByText("No matching pull requests")).toBeVisible();
  await panel.getByRole("textbox").fill("13");
  await expect(panel.getByRole("link", { name: "Open pull request #13 on GitHub" })).toHaveAttribute("href", "https://github.com/zest/app/pull/13");
  await panel.getByRole("link", { name: "Bot CI", exact: true }).click();
  await expect(page.getByRole("button", { name: "Close changes", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Close changes", exact: true }).click();
  await page.getByRole("button", { name: "Back", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await page.getByRole("button", { name: "Forward", exact: true }).click();
  await expect(panel).toBeVisible();
});

test("clicking the active chat from pull requests reveals its transcript", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Pull requests", exact: true }).click();
  const panel = page.getByRole("region", { name: "Pull requests", exact: true });
  await expect(panel.getByRole("link", { name: "Bot CI", exact: true })).toBeVisible();
  const activeChat = page.locator('button[aria-current="page"]').filter({ hasText: "Fixture" });
  await expect(activeChat).toBeVisible();
  await activeChat.click();
  await expect(panel).toHaveCount(0);
  await expect(page.locator("#zest-composer-input")).toBeVisible();
});

test("pull request review shows progress before a slow diff fetch completes", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=pull-request-delayed");
  await page.getByRole("button", { name: "Pull requests", exact: true }).click();
  const panel = page.getByRole("region", { name: "Pull requests", exact: true });
  await panel.getByRole("link", { name: "Bot CI", exact: true }).click();
  const diff = page.getByRole("dialog");
  await expect(diff).toBeVisible();
  await expect(diff.getByRole("status").filter({ hasText: "Fetching the pull request diff" })).toBeVisible();
  await expect(diff.getByText("src/example.ts", { exact: true })).toBeVisible({ timeout: 2_000 });
});
