import { expect, test } from "@playwright/test";

test("search, model and effort lists support keyboard navigation and restore focus", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-delayed");
  const trigger = page.getByTitle("Model and effort", { exact: true });
  await trigger.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("searchbox")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  const models = page.getByRole("listbox", { name: "Model", exact: true }).getByRole("option");
  await expect(models.nth(0)).toBeFocused();
  await page.keyboard.press("End");
  await expect(models.last()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(models.first()).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(models.last()).toBeFocused();
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowDown");
  await expect(models.nth(1)).toBeFocused();
  await expect(trigger).toHaveText("5.6 Sol · High");
  await page.keyboard.press("Enter");
  await expect(models.nth(1)).toBeDisabled();
  const efforts = page.getByRole("listbox", { name: "Effort" }).getByRole("option");
  await expect(efforts.getByText("High", { exact: true })).toBeVisible();
  await expect(efforts.nth(2)).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(efforts.nth(3)).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(efforts.nth(2)).toBeFocused();
  await page.keyboard.press("Home");
  await expect(efforts.first()).toBeFocused();
  await page.keyboard.press(" ");
  await expect(trigger).toHaveText("5.6 Terra · Low");
  await expect(trigger).toBeEnabled();
  await expect(trigger).toBeFocused();
  await page.keyboard.press(" ");
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  await expect(page.getByRole("searchbox")).toHaveCount(0);
});

test("Tab reaches effort and reset; outside dismissal preserves clicked focus", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-delayed");
  await page.getByTitle("Model and effort", { exact: true }).click();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "High", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Reset model and effort to default" })).toBeFocused();
  const composer = page.getByRole("textbox").first();
  // The popup legitimately covers the left half of the composer. Click its
  // exposed right edge, as a pointer user would, rather than forcing through it.
  const bounds = await composer.boundingBox();
  await composer.click({ position: { x: bounds!.width - 12, y: 12 } });
  await expect(composer).toBeFocused();
  await expect(page.getByRole("searchbox")).toHaveCount(0);
});

test("provider list moves focus without selection and preserves Continue flow", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=provider-picker");
  await expect(page.getByRole("heading", { name: "Choose a provider" })).toBeVisible();
  const providers = page.getByRole("listbox", { name: "Providers" }).getByRole("option");
  await providers.first().focus();
  await page.keyboard.press("End");
  await expect(providers.last()).toBeFocused();
  await expect(providers.first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowUp");
  await expect(providers.last()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press(" ");
  await expect(providers.nth(1)).toHaveAttribute("aria-selected", "true");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Choose a provider" })).toHaveCount(0);
});
