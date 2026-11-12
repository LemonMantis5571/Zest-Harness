import { expect, test } from "@playwright/test";

import { waitForFixtureBoot } from "./fixtureBoot";

test("the sidebar account menu opens Settings at the chosen section", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1");
  await waitForFixtureBoot(page);

  // Settings moved off the chat header.
  await expect(page.getByRole("button", { name: "Open settings" })).toHaveCount(0);

  const trigger = page.getByRole("button", { name: /^Account menu for / });
  await trigger.click();
  const menu = page.getByRole("menu", { name: "Account" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitem")).toHaveText([
    "Your profile",
    "Name and photo",
    "Provider",
    "Advisor",
    "CLI delegation",
    "Usage",
    /^Settings/,
  ]);
  await expect(menu.getByRole("menuitem", { name: "Your profile" })).toBeFocused();

  // Escape closes the menu and hands focus back without dismissing anything else.
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(trigger).toBeFocused();

  await trigger.click();
  await menu.getByRole("menuitem", { name: "Provider" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: /^Provider/ })).toHaveAttribute(
    "aria-expanded",
    "true"
  );
  await expect(dialog.getByRole("button", { name: /^User/ })).toHaveAttribute(
    "aria-expanded",
    "false"
  );
  expect(errors).toEqual([]);
});
