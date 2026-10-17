import { expect, test } from "@playwright/test";

const toasts = '[data-slot="toast"]';

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
