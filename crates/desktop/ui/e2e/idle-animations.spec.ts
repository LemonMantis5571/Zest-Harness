import { expect, test, type Page } from "@playwright/test";

import type { DesktopBackend } from "../src/lib/backend";
import { waitForFixtureBoot } from "./fixtureBoot";

/**
 * Every CSS animation the page is running right now, labelled by name and
 * target; `only` keeps the labels that start with that name. A settled chat
 * must have none: a hidden `animate-spin` still ticks, and 250 of them kept the
 * renderer's main thread ~95% busy at idle with 500 messages.
 */
function runningAnimations(page: Page, only?: string) {
  return page.evaluate((only) =>
    document
      .getAnimations()
      .filter((animation) => animation.playState === "running")
      .map((animation) => {
        const target = animation.effect instanceof KeyframeEffect ? animation.effect.target : null;
        const name = animation instanceof CSSAnimation
          ? animation.animationName
          : animation instanceof CSSTransition ? `transition ${animation.transitionProperty}` : animation.id;
        return `${name} on ${target instanceof Element ? target.tagName.toLowerCase() : "?"}`;
      })
      .filter((label) => !only || label.startsWith(`${only} `)), only);
}

async function openFifteenTurns(page: Page) {
  await page.goto("/?fixture=1");
  await waitForFixtureBoot(page);
  await page.getByRole("button", { name: "Fifteen turns", exact: true }).click();
  await expect(page.getByText("Turn 15 reply")).toBeVisible();
}

test("settled messages run no animations", async ({ page }) => {
  await openFifteenTurns(page);

  // One Save button per settled answer, so the check below is not vacuous. The
  // backend windows a thread to its last 10 user turns, hence 10 of the 15.
  await expect(page.getByRole("button", { name: /^Save Markdown as / })).toHaveCount(10);
  // Entrance animations and hover transitions end by themselves; a hidden spinner never does.
  // The sidebar's working dots for the boot turn take several seconds to clear.
  await expect.poll(() => runningAnimations(page), { timeout: 20_000 }).toEqual([]);
});

test("the Save spinner animates only while the save is pending", async ({ page }) => {
  await openFifteenTurns(page);

  // The fixture saves instantly, so hold the call to look at the pending state.
  await page.evaluate(() => {
    const backend = (globalThis as { __zestFixtureBackend?: DesktopBackend }).__zestFixtureBackend;
    if (!backend) throw new Error("The fixture backend is not available.");
    backend.saveMarkdown = () =>
      new Promise((resolve) => {
        window.addEventListener("zest:release-save", () => resolve(null), { once: true });
      });
  });

  const save = page.getByRole("button", { name: /^Save Markdown as / }).last();
  await save.click();
  await expect(save).toBeDisabled();
  await expect.poll(() => runningAnimations(page, "spin")).toEqual(["spin on svg"]);

  await page.evaluate(() => window.dispatchEvent(new Event("zest:release-save")));
  await expect(save).toBeEnabled();
  await expect.poll(() => runningAnimations(page, "spin")).toEqual([]);
});
