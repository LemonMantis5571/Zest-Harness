import { expect, test } from "@playwright/test";

import type { DesktopBackend } from "../src/lib/backend";
import { waitForFixtureBoot } from "./fixtureBoot";

test("Off clears a configured advisor whose provider is unavailable", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=advisor-unavailable");
  const readAdvisor = () => page.evaluate(async () => {
    const backend = (globalThis as { __zestFixtureBackend?: DesktopBackend }).__zestFixtureBackend;
    if (!backend) throw new Error("The fixture backend is not available.");
    return backend.advisorSettings();
  });
  expect((await readAdvisor()).provider).toBe("fixture-removed");

  await page.getByRole("button", { name: /^Account menu for / }).click();
  await page.getByRole("menuitem", { name: "Advisor" }).click();
  const dialog = page.getByRole("dialog");
  const issue = dialog.getByRole("alert").filter({ hasText: "provider `fixture-removed` is not configured" });
  await expect(issue).toBeVisible();
  const off = dialog.getByRole("radiogroup", { name: "Advisor", exact: true })
    .getByRole("radio", { name: "Off", exact: true });
  const initiallyChecked = await off.getAttribute("aria-checked");
  await off.click();
  await expect.poll(async () => (await readAdvisor()).provider).toBe("");
  expect(initiallyChecked).toBe("false");
  await expect(off).toHaveAttribute("aria-checked", "true");
  await expect(issue).toHaveCount(0);

  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: /^Account menu for / }).click();
  await page.getByRole("menuitem", { name: "Advisor" }).click();
  await expect(off).toHaveAttribute("aria-checked", "true");
  expect((await readAdvisor()).provider).toBe("");
  await expect(issue).toHaveCount(0);
});

test("the advisor is chosen in Settings, not in zest.toml", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1");
  await waitForFixtureBoot(page);

  await page.getByRole("button", { name: /^Account menu for / }).click();
  await page.getByRole("menuitem", { name: "Advisor" }).click();
  const dialog = page.getByRole("dialog");
  const providers = dialog.getByRole("radiogroup", { name: "Advisor", exact: true });

  // Off until chosen, and a provider without a key cannot be picked.
  await expect(providers.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
  await expect(providers.getByRole("radio", { name: /^Keyless fixture/ })).toBeDisabled();
  await expect(providers.getByRole("radio", { name: /^Fixture.*same as this chat/ })).toBeVisible();

  // A second provider starts on its default model and says it will ask first.
  await providers.getByRole("radio", { name: /^Strong fixture/ }).click();
  await expect(providers.getByRole("radio", { name: /^Strong fixture/ })).toHaveAttribute(
    "aria-checked",
    "true"
  );
  const models = dialog.getByRole("radiogroup", { name: "Advisor model" });
  await expect(models.getByRole("radio", { name: "strong-large" })).toHaveAttribute(
    "aria-checked",
    "true"
  );
  await expect(dialog.getByText(/Zest asks before sending the conversation to Strong fixture/)).toBeVisible();
  await expect(dialog.getByText("Saved in User zest.toml.")).toBeVisible();

  await models.getByRole("radio", { name: "strong-medium" }).click();
  const uses = dialog.getByRole("radiogroup", { name: "Consultations per request" });
  await uses.getByRole("radio", { name: "2" }).click();
  await expect(uses.getByRole("radio", { name: "2" })).toHaveAttribute("aria-checked", "true");

  // Zest's own reviews are on by default and can be switched off.
  const autoReview = dialog.getByRole("switch", { name: "Review automatically" });
  await expect(autoReview).toHaveAttribute("aria-checked", "true");
  await autoReview.click();
  await expect(autoReview).toHaveAttribute("aria-checked", "false");
  await expect(uses.getByRole("radio", { name: "2" })).toHaveAttribute("aria-checked", "true");

  // The choice persists: reopened, the collapsed section names it.
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.keyboard.press("Control+,");
  await expect(
    page.getByRole("dialog").getByRole("button", { name: /^Advisor.*Strong fixture · strong-medium/ })
  ).toBeVisible();

  // The chat's own provider needs no approval, so there is no warning.
  await page.getByRole("dialog").getByRole("button", { name: /^Advisor/ }).click();
  const reopened = page.getByRole("dialog").getByRole("radiogroup", { name: "Advisor", exact: true });
  await reopened.getByRole("radio", { name: /^Fixture/ }).click();
  await expect(reopened.getByRole("radio", { name: /^Fixture/ })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByText(/Zest asks before sending the conversation/)).toHaveCount(0);

  await reopened.getByRole("radio", { name: "Off" }).click();
  await expect(reopened.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
  await expect(page.getByRole("dialog").getByRole("radiogroup", { name: "Advisor model" })).toHaveCount(0);
  expect(errors).toEqual([]);
});
