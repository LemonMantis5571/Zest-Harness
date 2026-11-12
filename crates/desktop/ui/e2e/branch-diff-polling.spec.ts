import { expect, test, type Page } from "@playwright/test";

// The fixture counts backend calls on globalThis.__zestFixtureCalls.
async function calls(page: Page, method: string): Promise<number> {
  return page.evaluate(
    (name) => (globalThis as { __zestFixtureCalls?: Record<string, number> }).__zestFixtureCalls?.[name] ?? 0,
    method,
  );
}

test("an open branch diff asks for the clean view once, not on every Git poll", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: /^Show branch diff/ }).click();

  // The clean view is the default and comes from generateReadingDiff.
  await expect(page.getByRole("dialog", { name: "Branch changes" })).toBeVisible();
  await expect.poll(() => calls(page, "generateReadingDiff")).toBe(1);

  // The branch view re-reads Git every 2.5 s; wait out two unchanged polls.
  const polled = await calls(page, "workspaceChanges");
  await expect
    .poll(() => calls(page, "workspaceChanges"), { timeout: 10_000 })
    .toBeGreaterThanOrEqual(polled + 2);
  expect(await calls(page, "generateReadingDiff")).toBe(1);

  // Raw needs no model call, and returning to Clean reuses the first answer.
  await page.getByRole("button", { name: "Raw", exact: true }).click();
  await page.getByRole("button", { name: "Clean", exact: true }).click();
  expect(await calls(page, "generateReadingDiff")).toBe(1);
});
