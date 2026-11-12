import { expect, test } from "@playwright/test";

import { waitForFixtureBoot } from "./fixtureBoot";

test("compact chat list is one line per chat with marks only for exceptions", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.addInitScript(() => localStorage.setItem("zest.chatViewMode", "compact"));

  await page.goto("/?fixture=1");
  await waitForFixtureBoot(page);
  const sidebar = page.locator("aside").first();

  // One heading tier: project names, plus "No project" for projectless chats.
  await expect(sidebar.getByRole("heading", { name: "No project" })).toBeVisible();
  await expect(sidebar.getByRole("heading", { name: "Chats", exact: true })).toHaveCount(0);
  await expect(sidebar.getByText("Recent", { exact: true })).toHaveCount(0);
  await expect(sidebar.getByText("No chats yet")).toHaveCount(0);

  // The open chat is on the session's provider, so it carries no provider
  // mark; its pull request is the one thing the row says about it.
  const current = sidebar.getByRole("button", { name: /^Fixture\./ });
  await expect(current).toHaveAttribute("aria-current", "page");
  await expect(current).toHaveAccessibleName("Fixture. Pull request #13");
  const box = await current.boundingBox();
  expect(box?.height).toBeLessThanOrEqual(28);

  // A chat on another provider says so.
  await expect(sidebar.getByRole("button", { name: /^Local model chat\..*on ollama/ })).toBeVisible();

  await expect(sidebar.getByRole("button", { name: "Open a project folder" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: /^New chat in / })).toBeVisible();
  expect(errors).toEqual([]);
});
