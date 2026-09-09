import { expect, test } from "@playwright/test";

test("Wallpaper is visible in Customize and filters update it", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Customize", exact: true }).click();
  await page.getByRole("button", { name: "Extras", exact: true }).click();

  await page.getByRole("button", { name: "Turn on", exact: true }).last().click();
  await page.getByRole("button", { name: "Choose image", exact: true }).click();

  const wallpaper = page.locator(".zest-wallpaper");
  await expect(wallpaper).toHaveCount(1);
  await expect(wallpaper).toHaveAttribute("data-filter", "none");
  await expect(page.locator("html")).toHaveClass(/has-wallpaper/);
  await expect(wallpaper).toHaveCSS("background-image", /url\(/);

  await page.getByText("Sepia", { exact: true }).click();
  await expect(page.getByRole("radio", { name: "Sepia", exact: true })).toBeChecked();
  await expect(wallpaper).toHaveAttribute("data-filter", "sepia");
  await expect(page.locator("html")).toHaveAttribute("data-wallpaper-filter", "sepia");
  expect(errors).toEqual([]);
});
