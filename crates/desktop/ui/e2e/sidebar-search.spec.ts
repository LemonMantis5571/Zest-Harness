import { expect, test } from "@playwright/test";

test("sidebar search opens the shared palette on chat history", async ({ page }) => {
  await page.goto("/?fixture=1");

  const searchButton = page.getByRole("button", { name: "Search chats", exact: true });
  await expect(searchButton).toBeVisible();
  await searchButton.click();

  const palette = page.getByRole("dialog", { name: "Search", exact: true });
  await expect(palette).toBeVisible();
  await expect(palette.getByRole("tab", { name: "Chats", exact: true })).toHaveAttribute(
    "aria-selected",
    "true"
  );
  await expect(palette.getByRole("textbox")).toBeFocused();
});

test("project actions close when focus moves outside the menu", async ({ page }) => {
  await page.goto("/?fixture=1");

  const projectOptions = page.locator('button[aria-label^="Project options for"]').first();
  await expect(projectOptions).toBeVisible();
  await projectOptions.click();
  await expect(page.getByText("Project actions", { exact: true })).toBeVisible();

  await page.locator("header").last().click({ position: { x: 12, y: 12 } });
  await expect(page.getByText("Project actions", { exact: true })).toHaveCount(0);
});
