import { expect, type Page } from "@playwright/test";

/**
 * The seeded fixture chat streams a canned turn on load. A send before it ends
 * is queued, and its end re-renders the chat, so a spec that types into that
 * chat must wait it out. The canned turn is wall-clock paced and a slow runner
 * stretches it well past the default 5 s expect timeout.
 */
export async function waitForFixtureBoot(page: Page) {
  await expect(page.getByText("streams tool calls and text over Tauri events.")).toBeVisible({
    timeout: 25_000,
  });
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toBeHidden({
    timeout: 10_000,
  });
}
