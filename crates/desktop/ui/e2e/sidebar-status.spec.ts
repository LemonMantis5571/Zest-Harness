import { expect, test, type Page } from "@playwright/test";

/**
 * The compact list grouped by status, against `?scenario=sidebar-status`: one
 * chat waiting on an approval, one on a question, one running, and one that
 * finished in the background.
 */
async function openStatusView(page: Page) {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.addInitScript(() => localStorage.setItem("zest.chatViewMode", "compact"));
  await page.goto("/?fixture=1&scenario=sidebar-status");
  const sidebar = page.locator("aside").first();
  const byStatus = sidebar.getByRole("button", { name: "By status", exact: true });
  await expect(byStatus).toBeVisible();
  return { sidebar, byStatus, errors };
}

function section(sidebar: ReturnType<Page["locator"]>, name: string) {
  return sidebar.getByRole("region", { name, exact: true });
}

test("groups compact chats by what they are doing and moves rows live", async ({ page }) => {
  const { sidebar, byStatus, errors } = await openStatusView(page);

  // Project grouping stays the default; status is the toggle beside it.
  await expect(sidebar.getByRole("button", { name: "By project", exact: true })).toHaveAttribute(
    "aria-pressed",
    "true"
  );
  await expect(sidebar.getByRole("heading", { name: "zest-app" })).toBeVisible();
  await byStatus.click();
  await expect(byStatus).toHaveAttribute("aria-pressed", "true");
  await expect(sidebar.getByRole("heading", { name: "zest-app" })).toHaveCount(0);

  const needsInput = section(sidebar, "Needs input");
  await expect(needsInput.getByRole("button", { name: /^Review the auth flow\. Needs approval/ })).toBeVisible();
  await expect(needsInput.getByRole("button", { name: /^Pick a migration path\. Needs your answer/ })).toBeVisible();

  // Working rows shimmer; the project is a quiet label on every row.
  const working = section(sidebar, "Working");
  const running = working.getByRole("button", { name: /^Run the test suite\. Working/ });
  await expect(running).toBeVisible();
  await expect(running.getByText("Run the test suite", { exact: true })).toHaveClass(/shimmer-text/);
  await expect(running).toHaveAccessibleName(/in zest-app$/);
  await expect(running.getByText("zest-app", { exact: true })).toBeVisible();

  // Finished while another chat was open: bold until seen.
  const recent = section(sidebar, "Recent");
  const finished = recent.getByRole("button", { name: /^Draft release notes\. Finished since you last looked/ });
  await expect(finished).toBeVisible();
  await expect(finished.getByText("Draft release notes", { exact: true })).toHaveCSS("font-weight", "600");
  await expect(recent.getByRole("button", { name: /^Free chat\..*in No project/ })).toBeVisible();

  // The running turn ends: its row leaves Working for Recent, bold.
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-finish-working")));
  await expect(sidebar.getByRole("heading", { name: "Working" })).toHaveCount(0);
  const nowFinished = recent.getByRole("button", {
    name: /^Run the test suite\. Finished since you last looked/,
  });
  await expect(nowFinished).toBeVisible();
  await expect(nowFinished.getByText("Run the test suite", { exact: true })).not.toHaveClass(/shimmer-text/);

  // The choice survives a reload.
  await page.reload();
  await expect(sidebar.getByRole("button", { name: "By status", exact: true })).toHaveAttribute(
    "aria-pressed",
    "true"
  );
  expect(errors).toEqual([]);
});

test("status rows keep keyboard access, pinning, rename, and clear bold when opened", async ({ page }) => {
  const { sidebar, byStatus, errors } = await openStatusView(page);
  await byStatus.click();
  const recent = section(sidebar, "Recent");

  // Opening an unread chat is seeing it.
  const finished = recent.getByRole("button", { name: /^Draft release notes\./ });
  await finished.click();
  await expect(finished).toHaveAttribute("aria-current", "page");
  await expect(finished).not.toHaveAccessibleName(/Finished since you last looked/);
  await expect(finished.getByText("Draft release notes", { exact: true })).not.toHaveCSS(
    "font-weight",
    "600"
  );

  // Pinning moves a recent chat to the top of its section.
  await expect(recent.getByRole("listitem").first()).toContainText("Draft release notes");
  const fixture = recent.getByRole("listitem").filter({ hasText: /^Fixture/ });
  await fixture.hover();
  await fixture.getByRole("button", { name: "Pin chat" }).click();
  await expect(recent.getByRole("button", { name: /^Fixture\..*pinned$/ })).toBeVisible();
  await expect(recent.getByRole("listitem").first()).toContainText("Fixture");

  // Rename by double-click works here too. The open row is the one to use:
  // the fixture lists the open chat first, so a click on any other row
  // reorders the list between the two clicks.
  await finished.dblclick();
  const rename = sidebar.getByRole("textbox", { name: "Rename chat Draft release notes" });
  await expect(rename).toBeFocused();
  await rename.fill("Release notes, first pass");
  await rename.press("Enter");
  await expect(recent.getByRole("button", { name: /^Release notes, first pass\./ })).toHaveAttribute(
    "aria-current",
    "page"
  );

  // Tab from the toggle lands on the first chat, and Enter opens it.
  await byStatus.focus();
  await page.keyboard.press("Tab");
  const focusedName = await page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");
  expect(focusedName).toMatch(/^(Review the auth flow|Pick a migration path)\. Needs/);
  await page.keyboard.press("Enter");
  const title = focusedName.split(". ")[0];
  await expect(
    section(sidebar, "Needs input").getByRole("button", { name: `${title}. ` })
  ).toHaveAttribute("aria-current", "page");
  expect(errors).toEqual([]);
});
