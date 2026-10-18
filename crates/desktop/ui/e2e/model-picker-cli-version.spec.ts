import { expect, test } from "@playwright/test";

import type { DesktopBackend } from "../src/lib/backend";

test("the picker updates a newly resolved alias after a turn without reopening the chat", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=claude-alias-resolution");
  const trigger = page.getByTitle("Select model", { exact: true });
  await expect(trigger).toHaveAccessibleName("Select model, currently opus (alias)");
  await trigger.click();
  const models = page.getByRole("listbox", { name: "Model", exact: true });
  await expect(models.getByRole("option", { name: /^opus \(alias\)/ })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");

  const composer = page.locator("#zest-composer-input");
  await composer.fill("Resolve this alias");
  await composer.press("Enter");
  await expect(page.getByText("Fixture echo: Resolve this alias", { exact: true })).toBeVisible();
  const resolved = await page.evaluate(async () => {
    const backend = (globalThis as { __zestFixtureBackend?: DesktopBackend }).__zestFixtureBackend;
    if (!backend) throw new Error("The fixture backend is not available.");
    const info = await backend.sessionInfo();
    return info?.models.find((model) => model.id === "opus")?.resolvesTo;
  });
  expect(resolved).toBe("claude-opus-5");
  await expect(trigger).toHaveAccessibleName("Select model, currently opus → Opus 5");
  await trigger.click();
  await expect(models.getByRole("option", { name: /^opus → Opus 5 / })).toHaveAttribute("aria-selected", "true");
});

test("the picker marks models the installed Claude Code is too old for, and still lets you pick one", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/?fixture=1&scenario=claude-cli-outdated");
  const trigger = page.getByTitle("Select model", { exact: true });
  await expect(trigger).toHaveAccessibleName("Select model, currently Sonnet 5.5");
  await trigger.click();

  const picker = page.getByRole("dialog", { name: "Model and effort" });
  const models = picker.getByRole("listbox", { name: "Model", exact: true });
  await expect(models.getByRole("option")).toHaveCount(4);
  const opus = models.getByRole("option", { name: /Opus 5\.5/ });
  await expect(opus).toContainText("Needs Claude Code 2.1.280+ · run claude update");
  await expect(models.getByRole("option", { name: /Fable 5\.1/ })).toContainText(
    "Needs Claude Code 2.1.251+ · run claude update"
  );
  await expect(models.getByRole("option", { name: /Sonnet 5\.5/ })).not.toContainText("Needs");
  await expect(models.getByRole("option", { name: /Haiku 5\.5/ })).not.toContainText("Needs");
  await expect(picker.getByRole("note")).toHaveCount(0);

  // Warned, not disabled: the version check can be wrong.
  await expect(opus).toBeEnabled();
  await opus.click();
  await expect(opus).toHaveAttribute("aria-selected", "true");
  // Opus takes effort, so the picker stays open and says why the choice may fail.
  await expect(picker.getByRole("note")).toHaveText(
    "Claude Code 2.1.220 is installed. This model needs 2.1.280 or newer, so its turns may be refused. Run claude update, then restart Zest."
  );

  await page.keyboard.press("Escape");
  await expect(trigger).toHaveAccessibleName(
    "Select model, currently Opus 5.5. Needs Claude Code 2.1.280+ · run claude update"
  );

  expect(errors).toEqual([]);
});
