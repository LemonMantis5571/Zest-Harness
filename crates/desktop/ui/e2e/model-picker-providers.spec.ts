import { expect, test } from "@playwright/test";

test("Customize > Models keeps an unchecked provider out of the model picker", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1&scenario=model-catalogue");
  const trigger = page.getByTitle("Select model", { exact: true });
  const models = page.getByRole("listbox", { name: "Model", exact: true });
  await trigger.click();
  await expect(models.getByRole("option")).toHaveCount(63);
  await expect(models.getByRole("option", { name: /research-vision/ })).toHaveCount(1);
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: "Customize", exact: true }).click();
  await page.getByRole("button", { name: "Models", exact: true }).click();
  const providers = page.getByRole("list", { name: "Providers in the model picker" });
  // Only providers the picker could list: the unconfigured one is not offered.
  await expect(providers.getByRole("checkbox")).toHaveCount(2);
  const current = providers.getByRole("checkbox", { name: /always listed/ });
  await expect(current).toBeChecked();
  await expect(current).toBeDisabled();

  const research = providers.getByRole("checkbox", { name: /Research provider/ });
  await expect(research).toBeChecked();
  await research.uncheck();
  expect(
    await page.evaluate(() => localStorage.getItem("zest.pickerHiddenProviders.v1"))
  ).toBe('["fixture-research"]');

  await page.getByRole("button", { name: "Back to chat", exact: true }).click();
  await trigger.click();
  await expect(models.getByRole("option")).toHaveCount(62);
  await expect(models.getByRole("option", { name: /research-vision/ })).toHaveCount(0);
  await page.keyboard.press("Escape");

  // The choice survives a reload, and checking the box brings the models back.
  await page.reload();
  await page.getByRole("button", { name: "Customize", exact: true }).click();
  await page.getByRole("button", { name: "Models", exact: true }).click();
  await expect(research).not.toBeChecked();
  await research.check();
  await page.getByRole("button", { name: "Back to chat", exact: true }).click();
  await trigger.click();
  await expect(models.getByRole("option")).toHaveCount(63);

  expect(errors).toEqual([]);
});
