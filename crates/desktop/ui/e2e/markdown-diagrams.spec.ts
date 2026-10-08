import { expect, test } from "@playwright/test";
import { waitForFixtureBoot } from "./fixtureBoot";

test("existing Mermaid diagrams and math labels still render and expand", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1");
  await waitForFixtureBoot(page);
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  await page.locator("#zest-composer-input").fill('Diagram example\n\n```mermaid\ngraph LR\n  A["$$E = mc^2$$"] --> B["Verified"]\n```');
  await page.locator("#zest-composer-input").press("Enter");
  const diagram = page.getByRole("button", { name: "Expand Mermaid diagram", exact: true });
  await expect(diagram).toBeVisible();
  await expect(diagram.locator(".mermaid-diagram svg")).toBeVisible();
  await expect(diagram).toContainText("Verified");
  await expect(diagram.locator(".katex").first()).toBeVisible();
  await diagram.click();
  const expanded = page.getByRole("dialog", { name: "Mermaid diagram", exact: true });
  await expect(expanded.locator(".katex").first()).toBeVisible();
  await page.getByRole("button", { name: "Close diagram", exact: true }).last().click();
  await expect(expanded).toHaveCount(0);
  expect(errors).toEqual([]);
});
