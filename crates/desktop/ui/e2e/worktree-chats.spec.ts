import { expect, test, type Page } from "@playwright/test";

// The fixture gives the first worktree chat 2 ahead, 1 behind and 3 uncommitted
// entries, and every later one a clean, level checkout.
const FIRST_STATUS = "2 commits ahead, 1 commit behind, 3 uncommitted changes";
const ANY_STATUS = /commits? (ahead|behind)|uncommitted change/;

function watchErrors(page: Page) {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  return errors;
}

async function newWorktreeChat(page: Page) {
  const sidebar = page.locator("aside").first();
  const original = sidebar.getByRole("button", { name: "Project options for zest-app", exact: true });
  await original.click();
  await sidebar.getByRole("button", { name: "New chat in worktree" }).click();
  return sidebar;
}

for (const view of ["project", "compact"] as const) {
  test(`creates separate worktree chats from the ${view} sidebar`, async ({ page }) => {
    const errors = watchErrors(page);

    await page.addInitScript((mode) => localStorage.setItem("zest.chatViewMode", mode), view);
    await page.goto("/?fixture=1&scenario=sidebar-status");
    const sidebar = page.locator("aside").first();
    const original = sidebar.getByRole("button", { name: "Project options for zest-app", exact: true });
    await expect(original).toBeVisible();
    await original.click();
    await sidebar.getByRole("button", { name: "New chat in worktree" }).click();

    await expect(sidebar.getByText(/zest-app · zest\/chat-/)).toBeVisible();
    await expect(original).toBeVisible();
    await expect(page.getByText(/zest\/chat-/).first()).toBeVisible();

    await original.click();
    await sidebar.getByRole("button", { name: "New chat in worktree" }).click();
    await expect(sidebar.getByText(/zest-app · zest\/chat-/)).toHaveCount(2);
    expect(errors).toEqual([]);
  });

  test(`shows git status only on a worktree chat that has something to report in the ${view} sidebar`, async ({ page }) => {
    const errors = watchErrors(page);

    await page.addInitScript((mode) => localStorage.setItem("zest.chatViewMode", mode), view);
    await page.goto("/?fixture=1&scenario=sidebar-status");
    const sidebar = page.locator("aside").first();
    await expect(sidebar.getByRole("button", { name: "Project options for zest-app", exact: true })).toBeVisible();
    await expect(sidebar.getByRole("img", { name: ANY_STATUS })).toHaveCount(0);

    await newWorktreeChat(page);
    const badge = sidebar.getByRole("img", { name: FIRST_STATUS });
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute("title", FIRST_STATUS);
    // The arrows and the dirty dot are icons, so the visible text is the counts.
    await expect(badge).toHaveText("213");

    // A second chat starts level and clean, so its row adds no badge.
    await newWorktreeChat(page);
    await expect(sidebar.getByText(/zest-app · zest\/chat-/)).toHaveCount(2);
    await expect(sidebar.getByRole("img", { name: ANY_STATUS })).toHaveCount(1);
    await expect(badge).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`a finished turn refreshes the git counts in the ${view} sidebar`, async ({ page }) => {
    const errors = watchErrors(page);

    await page.addInitScript((mode) => localStorage.setItem("zest.chatViewMode", mode), view);
    await page.goto("/?fixture=1&scenario=sidebar-status");
    const sidebar = page.locator("aside").first();
    await expect(sidebar.getByRole("button", { name: "Project options for zest-app", exact: true })).toBeVisible();
    await newWorktreeChat(page);
    await expect(sidebar.getByRole("img", { name: FIRST_STATUS })).toBeVisible();

    // The fixture's turn edits a file in the worktree it ran in.
    const composer = page.locator("#zest-composer-input");
    await expect(composer).toBeVisible();
    await composer.fill("change a file");
    await composer.press("Enter");
    await expect(page.getByText("Fixture echo: change a file", { exact: true })).toBeVisible();
    await expect(
      sidebar.getByRole("img", { name: "2 commits ahead, 1 commit behind, 4 uncommitted changes" })
    ).toBeVisible();
    expect(errors).toEqual([]);
  });

  test(`a git failure leaves the ${view} sidebar without counts and without an error`, async ({ page }) => {
    const errors = watchErrors(page);

    await page.addInitScript((mode) => localStorage.setItem("zest.chatViewMode", mode), view);
    await page.goto("/?fixture=1&scenario=sidebar-status&worktreeStatus=fail");
    const sidebar = page.locator("aside").first();
    await expect(sidebar.getByRole("button", { name: "Project options for zest-app", exact: true })).toBeVisible();
    await newWorktreeChat(page);

    await expect(sidebar.getByText(/zest-app · zest\/chat-/)).toBeVisible();
    await expect(sidebar.getByRole("img", { name: ANY_STATUS })).toHaveCount(0);
    // The scenario seeds unrelated toasts for other chats; none may be about Git.
    await expect(page.locator('[data-slot="toast"]').filter({ hasText: /git|worktree|unavailable/i })).toHaveCount(0);
    await expect(sidebar.getByText(/could not load/i)).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
