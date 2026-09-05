import { expect, test } from "@playwright/test";

test("locks every option until model save completes, then saves effort", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-delayed");
  await page.getByTitle("Model and effort", { exact: true }).click();
  const panel = page.getByRole("dialog", { name: "Model and effort", exact: true });
  await panel.getByRole("option", { name: /^5\.6 Terra / }).click();
  await expect(panel.getByRole("status")).toHaveText("Saving selection…");
  for (const option of await panel.getByRole("option").all()) await expect(option).toBeDisabled();
  await expect(panel.getByRole("button", { name: "Reset model and effort to default" })).toBeDisabled();
  await expect(panel.getByRole("option", { name: "Low", exact: true })).toBeEnabled();
  await panel.getByRole("option", { name: "Low", exact: true }).click();
  await expect(page.getByTitle("Model and effort", { exact: true })).toHaveText("5.6 Terra · Low");
  await expect(page.getByTitle("Model and effort", { exact: true })).toBeEnabled();
});

test("failed model save rolls back and allows a retry", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-failing");
  const trigger = page.getByTitle("Model and effort", { exact: true });
  await trigger.click();
  const terra = page.getByRole("option", { name: /^5\.6 Terra / });
  await terra.click();
  await expect(page.getByRole("status")).toHaveText("Saving selection…");
  await expect(terra).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Sol · High");
  await terra.click();
  await expect(terra).toBeDisabled();
  await expect(terra).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Terra · High");
});

test("dismissal during save stays dismissed and reset persists", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-delayed");
  const trigger = page.getByTitle("Model and effort", { exact: true });
  await trigger.click();
  await page.getByRole("option", { name: /^5\.6 Terra / }).click();
  await page.keyboard.press("Escape");
  await expect(trigger).toBeEnabled();
  await expect(page.getByRole("dialog", { name: "Model and effort", exact: true })).toHaveCount(0);
  await trigger.click();
  await page.getByRole("button", { name: "Reset model and effort to default" }).click();
  await expect(trigger).toBeDisabled();
  await expect(trigger).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Sol · High");
});
