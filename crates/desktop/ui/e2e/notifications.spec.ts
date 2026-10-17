import { expect, test, type Page } from "@playwright/test";

import { waitForFixtureBoot } from "./fixtureBoot";

const toasts = '[data-slot="toast"]';

/**
 * Zest toasts only while it is the focused window; otherwise the alert goes
 * to the OS. A headless page need not report focus, so pin it.
 */
async function forceFocus(page: Page) {
  await page.addInitScript(() => {
    Object.defineProperty(Document.prototype, "hasFocus", {
      configurable: true,
      value: () => true,
    });
  });
}

async function send(page: Page, text: string) {
  const composer = page.locator("#zest-composer-input");
  await expect(composer).toBeVisible();
  await composer.fill(text);
  await composer.press("Enter");
}

test("an approval in another chat toasts with the chat's name and an Open action", async ({
  page,
}) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await send(page, "hello");

  const toast = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(toast).toBeVisible();
  await expect(toast).toContainText("Run the fixture tool in the background chat");
  await toast.getByRole("button", { name: "Open", exact: true }).click();

  await expect(toast).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Free chat\./ })).toHaveAttribute(
    "aria-current",
    "page"
  );
  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
});

test("an approval toast waits for the user and closes when its chat is opened", async ({
  page,
}) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=background-approval");
  await send(page, "hello");

  const toast = page.locator(toasts).filter({ hasText: "Approval needed · Free chat" });
  await expect(toast).toBeVisible();
  // Past Base UI's 5 s default: an approval holds the agent until answered.
  await page.waitForTimeout(6_000);
  await expect(toast).toBeVisible();

  await page.getByRole("button", { name: /^Free chat\./ }).click();
  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
  await expect(toast).toHaveCount(0);
});

test("an approval in the chat on screen shows only its card", async ({ page }) => {
  await forceFocus(page);
  await page.goto("/?fixture=1&scenario=approval");
  await waitForFixtureBoot(page);
  await send(page, "exercise the boundary");

  await expect(page.getByRole("region", { name: "Approval needed" })).toBeVisible();
  // The decision awaits a focus check, so give a wrong toast time to appear.
  await page.waitForTimeout(500);
  await expect(page.locator(toasts)).toHaveCount(0);
});

test("automatic compaction leaves a divider in the transcript instead of a toast", async ({
  page,
}) => {
  // ui-stress reports a context window past the auto-compaction threshold, so
  // the turn's end compacts.
  await page.goto("/?fixture=1&scenario=ui-stress");
  const composer = page.locator("#zest-composer-input");
  await expect(composer).toBeVisible();
  await composer.fill("hello");
  await composer.press("Enter");

  const transcript = page.locator('[data-slot="message-scroller-viewport"]');
  await expect(transcript.getByText("Fixture echo: hello")).toBeVisible();
  const divider = transcript.getByRole("note");
  await expect(divider).toHaveText(
    "Conversation compacted automatically · restore point saved"
  );
  await expect(page.locator(toasts)).toHaveCount(0);
});
