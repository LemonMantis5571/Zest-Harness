import { expect, test } from "@playwright/test";

for (const view of ["project", "compact"] as const) {
  test(`creates separate worktree chats from the ${view} sidebar`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      if (message.type() === "error") errors.push(message.text());
    });

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
}
