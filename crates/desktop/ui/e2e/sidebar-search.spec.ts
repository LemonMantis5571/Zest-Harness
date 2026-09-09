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
