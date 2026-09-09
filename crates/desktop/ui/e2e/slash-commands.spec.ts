import { expect, test } from "@playwright/test";

test("slash skills can be invoked more than once from any draft position", async ({ page }) => {
  await page.goto("/?fixture=1");

  const composer = page.locator("#zest-composer-input");
  await composer.fill("review this ");
  await composer.type("/");

  const commands = page.getByRole("listbox", { name: "Commands", exact: true });
  await expect(commands).toBeVisible();
  await commands.getByRole("option", { name: /browse-x/ }).click();
  await expect(composer).toHaveValue("review this /browse-x ");

  await composer.type("/");
  await expect(commands).toBeVisible();
  await commands.getByRole("option", { name: /plan/ }).click();
  await expect(composer).toHaveValue("review this /browse-x /plan ");

  await composer.press("Home");
  await composer.type("before ");
  await expect(composer).toHaveValue("before review this /browse-x /plan ");
  await composer.type("/");
  await expect(commands).toBeVisible();
  await commands.getByRole("option", { name: /browse-x/ }).click();
  await expect(composer).toHaveValue("before /browse-x review this /browse-x /plan ");
});
