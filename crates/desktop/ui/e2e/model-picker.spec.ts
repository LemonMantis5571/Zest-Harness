import { expect, test } from "@playwright/test";

test("current GPT models persist with only their supported effort choices", async ({ page }) => {
  await page.goto("/?fixture=1");
  const trigger = page.getByTitle("Select model", { exact: true });
  for (const label of ["6.1 Sol", "6 Astra"]) {
    await trigger.click();
    const panel = page.getByRole("dialog", { name: "Model and effort", exact: true });
    const name = new RegExp(`^${label.replaceAll(".", "\\.")} `);
    await panel.getByRole("option", { name }).click();
    await expect(trigger).toHaveText(label);
    await expect(panel.getByRole("option", { name })).toContainText("1.1M context");
    await expect(panel.getByRole("option", { name })).toContainText("Vision");
    await expect(panel.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "High", exact: true })).toHaveAttribute("aria-selected", "true");
    await expect(panel.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "None", exact: true })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await trigger.click();
    await expect(panel.getByRole("option", { name })).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("Escape");
  }
});

test("locks every option until model save completes, then saves effort", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
  await page.goto("/?fixture=1&scenario=options-delayed");
  const trigger = page.getByTitle("Select model", { exact: true });
  await expect(trigger).toBeEnabled();
  await page.clock.pauseAt(new Date("2026-01-01T01:00:00Z"));
  await trigger.click();
  const panel = page.getByRole("dialog", { name: "Model and effort", exact: true });
  await panel.getByRole("option", { name: /^5\.6 Terra / }).click();
  await expect(panel.getByRole("status")).toHaveText("Saving selection…");
  for (const option of await panel.getByRole("option").all()) await expect(option).toBeDisabled();
  await expect(panel.getByRole("button", { name: "Reset model and effort to default" })).toBeDisabled();
  await page.clock.runFor(1000);
  await expect(panel.getByRole("option", { name: "Low", exact: true })).toBeEnabled();
  await panel.getByRole("option", { name: "Low", exact: true }).click();
  await page.clock.runFor(1000);
  await expect(trigger).toHaveText("5.6 Terra");
  await expect(trigger).toBeEnabled();
  await trigger.click();
  await expect(
    page.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "Low", exact: true })
  ).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
});

test("failed model save rolls back and allows a retry", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-failing");
  const trigger = page.getByTitle("Select model", { exact: true });
  await trigger.click();
  const panel = page.getByRole("dialog", { name: "Model and effort", exact: true });
  const terra = page.getByRole("option", { name: /^5\.6 Terra / });
  await terra.click();
  await expect(page.getByRole("status")).toHaveText("Saving selection…");
  await expect(terra).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Sol");
  await terra.click();
  await expect(terra).toBeDisabled();
  await expect(terra).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Terra");
  await expect(
    panel.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "High", exact: true })
  ).toHaveAttribute("aria-selected", "true");
});

test("dismissal during save stays dismissed and reset persists", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=options-delayed");
  const trigger = page.getByTitle("Select model", { exact: true });
  await trigger.click();
  await page.getByRole("option", { name: /^5\.6 Terra / }).click();
  await page.keyboard.press("Escape");
  await expect(trigger).toBeEnabled();
  await expect(page.getByRole("dialog", { name: "Model and effort", exact: true })).toHaveCount(0);
  await trigger.click();
  await page.getByRole("button", { name: "Reset model and effort to default" }).click();
  await expect(trigger).toBeDisabled();
  await expect(trigger).toBeEnabled();
  await expect(trigger).toHaveText("5.6 Sol");
  await trigger.click();
  await expect(
    page.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "High", exact: true })
  ).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
});
