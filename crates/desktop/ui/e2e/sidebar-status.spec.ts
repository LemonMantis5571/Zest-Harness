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

test("nests delegation jobs under the chat that started them and opens each in the Workbench", async ({ page }) => {
  const { sidebar, byStatus, errors } = await openStatusView(page);

  // Nesting is part of the row, so the project grouping has it too.
  await expect(
    sidebar
      .getByRole("list", { name: "Jobs started from Fixture" })
      .getByRole("button", { name: "Port the session cache. Delegated job, Working" })
  ).toBeVisible();

  await byStatus.click();
  const fixtureJobs = section(sidebar, "Recent").getByRole("list", { name: "Jobs started from Fixture" });
  await expect(fixtureJobs.getByRole("listitem")).toHaveCount(4);
  await expect(
    fixtureJobs.getByRole("button", { name: "Port the session cache. Delegated job, Working" })
  ).toBeVisible();
  const ready = fixtureJobs.getByRole("button", {
    name: "Write migration tests. Delegated job, Ready to apply",
  });
  await expect(ready).toBeVisible();
  const approval = fixtureJobs.getByRole("button", {
    name: "Add the fixture lifecycle. Delegated job, Needs approval",
  });
  await expect(approval).toBeVisible();
  // The applied job is past the cap of three.
  await expect(
    fixtureJobs.getByRole("button", { name: "1 more job from Fixture, in the Workbench" })
  ).toBeVisible();
  await expect(sidebar.getByText("Bump the lockfile")).toHaveCount(0);
  await expect(
    section(sidebar, "Needs input")
      .getByRole("list", { name: "Jobs started from Pick a migration path" })
      .getByRole("button", { name: "Survey table sizes. Delegated job, Queued" })
  ).toBeVisible();

  // A child opens the Workbench's Delegation tab at that job.
  await ready.click();
  await expect(page.getByRole("tab", { name: "Delegation" })).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#workbench-delegation-fixture-delegation-ready")).toBeFocused();

  // Its status follows the job live.
  await approval.click();
  const approvalCard = page.locator("#workbench-delegation-fixture-delegation-1");
  await expect(approvalCard).toBeFocused();
  await approvalCard.getByRole("button", { name: "Approve" }).click();
  await expect(
    fixtureJobs.getByRole("button", { name: "Add the fixture lifecycle. Delegated job, Ready to apply" })
  ).toBeVisible();

  // The jump focuses the card once; coming back to the tab later leaves focus
  // on the tabs, so the arrow keys keep working.
  await page.getByRole("tab", { name: "Outline" }).click();
  await page.keyboard.press("ArrowRight");
  const delegationTab = page.getByRole("tab", { name: "Delegation" });
  await expect(delegationTab).toHaveAttribute("aria-selected", "true");
  // Let any queued focus land before checking where it is.
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
  );
  await expect(delegationTab).toBeFocused();
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
  // Hover the row itself; the item also holds the row's nested jobs.
  await fixture.getByRole("button", { name: /^Fixture\./ }).hover();
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
