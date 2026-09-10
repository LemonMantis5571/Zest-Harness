import { expect, test } from "@playwright/test";

test("Workbench opens and its views remain usable", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.goto("/?fixture=1");
  await expect(page.getByRole("button", { name: "Open command palette", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open Workbench", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Search", exact: true }).click();
  const palette = page.getByRole("dialog", { name: "Search", exact: true });
  await palette.getByRole("textbox").fill("Open workbench");
  await palette.getByRole("option", { name: /Open workbench/i }).click();
  await expect(page.getByRole("heading", { name: "Workbench", exact: true }), JSON.stringify(errors)).toBeVisible();
  for (const name of ["Outline", "Delegation", "Files", "Activity"]) {
    await page.getByRole("tab", { name, exact: true }).click();
    await expect(page.getByRole("tabpanel")).toBeVisible();
  }
  const closeWorkbench = page.locator("#workbench-panel").getByRole("button", { name: "Close Workbench", exact: true });
  await expect(closeWorkbench.locator("svg")).toHaveClass(/lucide-panel-right-close/);
  await closeWorkbench.click();
  await expect(page.locator("#workbench-panel")).toHaveCount(0);
  expect(errors).toEqual([]);
});
