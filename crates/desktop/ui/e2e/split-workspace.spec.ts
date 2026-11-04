import { expect, test, type Page } from "@playwright/test";

/**
 * Two panes with a live turn each (`split-streaming` keeps a turn running until
 * it is stopped or finished through `splitTurn`). Both chats are called
 * "Fixture", so tests scope everything to a pane's region.
 */
async function openStreamingPanes(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem("zest.responseBlockStreaming.v1", "false");
  });
  await page.goto("/?fixture=1&scenario=split-streaming");
  await page.getByRole("button", { name: "Open split view" }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("left running");
  await left.getByRole("button", { name: "Send message" }).click();
  await expect(left.getByRole("button", { name: "Stop response" })).toBeEnabled();
  await right.getByRole("textbox", { name: "Message", exact: true }).fill("right running");
  await right.getByRole("button", { name: "Send message" }).click();
  await expect(left.getByText("Live response to left running", { exact: true })).toBeVisible();
  await expect(right.getByText("Live response to right running", { exact: true })).toBeVisible();
  return { left, right };
}

/** Move a live `split-streaming` turn along; the fixture finds it by the prompt that started it. */
async function splitTurn(
  page: Page,
  prompt: string,
  action: "delta" | "done" | "approval" | "question",
  text?: string
) {
  await page.evaluate(
    (detail) => window.dispatchEvent(new CustomEvent("zest:fixture-split-turn", { detail })),
    { prompt, action, text }
  );
}

const unreadThreads = (page: Page) =>
  page.evaluate(() => {
    const stored: unknown = JSON.parse(localStorage.getItem("zest.unreadThreads") ?? "[]");
    return Array.isArray(stored)
      ? stored.filter((id): id is string => typeof id === "string")
      : [];
  });

test("a chat finishing behind a split chooser stays unread until its transcript returns", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("zest.chatViewMode", "compact"));
  await page.goto("/?fixture=1&scenario=sidebar-status");
  await page.getByRole("button", { name: "Open split view", exact: true }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: /Run the test suite/ }).click();
  await expect(right.getByRole("button", { name: "Stop response", exact: true })).toBeVisible();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("activate the left pane");
  await left.getByRole("button", { name: "Send message", exact: true }).click();
  await expect(left.getByText("Fixture echo: activate the left pane", { exact: true })).toBeVisible();
  await right.getByRole("button", { name: /^Run the test suite/ }).click();
  const chooser = right.getByRole("textbox", { name: "Find a chat", exact: true });
  await expect(chooser).toBeVisible();
  await chooser.fill("migration");
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-finish-working")));
  await expect.poll(() => page.evaluate(() => localStorage.getItem("zest.unreadThreads")))
    .toBe('["fixture-finished","fixture-working"]');
  await expect(chooser).toHaveValue("migration");
  await right.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("zest.unreadThreads"))).toBe('["fixture-finished"]');
});

test("a visible split pane finishing is read even when the other pane owns the backend session", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("zest.chatViewMode", "compact"));
  await page.goto("/?fixture=1&scenario=sidebar-status");
  await page.getByRole("button", { name: "Open split view", exact: true }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: /Run the test suite/ }).click();
  await expect(right.getByRole("button", { name: "Stop response", exact: true })).toBeVisible();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("activate the left pane");
  await left.getByRole("button", { name: "Send message", exact: true }).click();
  await expect(left.getByText("Fixture echo: activate the left pane", { exact: true })).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-finish-working")));
  await expect(right.getByRole("button", { name: "Stop response", exact: true })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("zest.unreadThreads"))).toBe('["fixture-finished"]');
});

test("returning to a split group clears unread marks on every visible pane", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("zest.chatViewMode", "compact"));
  await page.goto("/?fixture=1&scenario=sidebar-status");
  const sidebar = page.locator("aside").first();
  await sidebar.getByRole("button", { name: /^Run the test suite\. Working/ }).click();
  await page.getByRole("button", { name: "Open split view", exact: true }).click();
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: /^Fixture/ }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await right.getByRole("button", { name: "Continue in single view", exact: true }).click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-finish-working")));
  await expect.poll(() => page.evaluate(() => localStorage.getItem("zest.unreadThreads"))).toBe('["fixture-finished","fixture-working"]');
  await sidebar.getByRole("button", { name: "Split view with 2 chats", exact: true }).click();
  await expect(page.getByRole("region", { name: "Left chat", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("zest.unreadThreads"))).toBe('["fixture-finished"]');
});

test("both panes stream together and stopping one leaves the other running", async ({ page }) => {
  // This assertion checks live text delivery, so use the text-as-it-arrives
  // preference rather than the default complete-block display mode.
  await page.addInitScript(() => {
    localStorage.setItem("zest.responseBlockStreaming.v1", "false");
  });
  await page.goto("/?fixture=1&scenario=split-streaming");
  await page.getByRole("button", { name: "Open split view" }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("left running");
  await left.getByRole("button", { name: "Send message" }).click();
  await expect(left.getByRole("button", { name: "Stop response" })).toBeEnabled();
  await right.getByRole("textbox", { name: "Message", exact: true }).fill("right running");
  await right.getByRole("button", { name: "Send message" }).click();
  await expect(left.getByText("Live response to left running", { exact: true })).toBeVisible();
  await expect(right.getByText("Live response to right running", { exact: true })).toBeVisible();
  await left.getByRole("button", { name: "Stop response" }).click();
  await expect(left.getByRole("button", { name: "Stop response" })).toHaveCount(0);
  await expect(right.getByRole("button", { name: "Stop response" })).toBeEnabled();
  await right.getByRole("button", { name: "Stop response" }).click();
  await expect(right.getByRole("button", { name: "Stop response" })).toHaveCount(0);
});

test("split chats keep drafts and messages separate, resize, and return to single view", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Open split view" }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("left independent draft");
  await right.getByRole("textbox", { name: "Message", exact: true }).fill("right independent message");
  await right.getByRole("button", { name: "Send message" }).click();
  await expect(right.getByText("right independent message", { exact: true })).toBeVisible();
  await expect(left.getByText("right independent message", { exact: true })).toHaveCount(0);
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toHaveValue("left independent draft");
  await left.getByRole("button", { name: "Send message" }).click();
  await expect(left.getByText("left independent draft", { exact: true })).toBeVisible();
  await expect(right.getByText("left independent draft", { exact: true })).toHaveCount(0);
  await expect(left.getByText("right independent message", { exact: true })).toHaveCount(0);
  const divider = page.getByRole("separator", { name: "Resize split panes" });
  await divider.focus();
  await divider.press("ArrowLeft");
  await expect(divider).toHaveAttribute("aria-valuenow", "48");
  await page.screenshot({ path: "test-results/split-desktop.png" });
  await page.setViewportSize({ width: 650, height: 900 });
  await expect(divider).toBeHidden();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await page.screenshot({ path: "test-results/split-narrow.png" });
  await right.getByRole("textbox", { name: "Message", exact: true }).fill("keep this draft");
  await right.getByRole("button", { name: "Continue in single view" }).click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  await expect(page.getByText("right independent message", { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test("a fresh split chat can send without reopening a missing thread", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Open split view" }).click();
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: "New chat in this project", exact: true }).click();
  await right.getByRole("textbox", { name: "Message", exact: true }).fill("fresh split message");
  await right.getByRole("button", { name: "Send message", exact: true }).click();
  await expect(right.getByText("fresh split message", { exact: true })).toBeVisible();
  await expect(page.getByText("Could not open project chat", { exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("adds a nested pane and keeps the split group in the sidebar", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Open split view" }).click();

  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();

  await right.getByRole("button", { name: "Add pane" }).click();
  const bottom = page.getByRole("region", { name: "Right bottom chat", exact: true });
  await expect(bottom.getByRole("button", { name: "New chat in this project", exact: true })).toBeVisible();
  await bottom.getByRole("button", { name: /Fifteen turns/ }).click();
  await expect(bottom.getByText("Turn 15 prompt", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Split view with 3 chats", exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test("preserves split groups while navigating back to a normal chat", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Open split view" }).click();
  const left = page.getByRole("region", { name: "Left chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  await expect(page.getByRole("region", { name: "Right chat", exact: true }).getByRole("textbox", { name: "Message", exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Continue in single view" }).first().click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  const savedGroup = page.getByRole("button", { name: "Split view with 2 chats", exact: true });
  await expect(savedGroup).toBeVisible();

  await savedGroup.click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Left chat", exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Right chat", exact: true })).toBeVisible();

  await page.getByRole("region", { name: "Left chat", exact: true }).getByRole("button", { name: "Continue in single view" }).click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Open split view" }).click();
  await expect(page.getByRole("button", { name: "Split view with 1 chats", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Split view with 2 chats", exact: true })).toBeVisible();
});

test("keeps the composer and add-pane controls inside a long split transcript", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Fifteen turns", exact: true }).click();
  await page.getByRole("button", { name: "Open split view" }).click();

  const left = page.getByRole("region", { name: "Left chat", exact: true });
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await left.getByRole("button", { name: "Fork into other pane" }).click();

  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await right.getByRole("button", { name: "Add pane", exact: true }).click();

  const bottom = page.getByRole("region", { name: "Right bottom chat", exact: true });
  await expect(bottom.getByRole("textbox", { name: "Find a chat", exact: true })).toBeVisible();
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test("moves a pane with a pointer drag in the split workspace", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Open split view" }).click();

  const left = page.getByRole("region", { name: "Left chat", exact: true });
  await left.getByRole("button", { name: "Fork into other pane" }).click();
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();

  const leftPaneId = await left.getAttribute("data-split-pane-id");
  const rightPaneId = await right.getAttribute("data-split-pane-id");
  const grip = left.getByRole("button", { name: /^Drag / });
  const gripBox = await grip.boundingBox();
  const rightBox = await right.boundingBox();
  if (!leftPaneId || !rightPaneId || !gripBox || !rightBox) throw new Error("Split panes were not measurable");

  await page.mouse.move(gripBox.x + gripBox.width / 2, gripBox.y + gripBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(rightBox.x + rightBox.width - 8, rightBox.y + rightBox.height / 2, { steps: 12 });
  await page.mouse.up();

  await expect.poll(async () => page.locator("[data-split-pane-id]").evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-split-pane-id")))).toEqual([rightPaneId, leftPaneId]);
});

test("changes model and effort independently in each split pane", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1&scenario=model-catalogue");
  await page.getByRole("button", { name: "Open split view" }).click();

  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await right.getByRole("button", { name: /Fifteen turns/ }).click();
  const rightTrigger = right.getByTitle("Select model", { exact: true });
  await rightTrigger.click();
  const rightPanel = right.getByRole("dialog", { name: "Model and provider", exact: true });
  await rightPanel.getByRole("option", { name: /^5\.6 Luna / }).click();
  await expect(rightTrigger).toHaveText("5.6 Luna");
  await expect(rightPanel.getByRole("listbox", { name: "Effort" }).getByRole("option", { name: "High", exact: true })).toHaveAttribute("aria-selected", "true");

  const leftTrigger = left.getByTitle("Select model", { exact: true });
  await leftTrigger.click();
  const leftPanel = left.getByRole("dialog", { name: "Model and provider", exact: true });
  await leftPanel.getByRole("option", { name: /^5\.6 Terra / }).click();
  await expect(leftTrigger).toHaveText("5.6 Terra");
  await leftPanel.getByRole("option", { name: "Low", exact: true }).click();
  await expect(leftTrigger).toHaveText("5.6 Terra");
  await expect(rightTrigger).toHaveText("5.6 Luna");
  expect(errors).toEqual([]);
});

test("removes deleted chats from the split chooser and clears an open pane", async ({ page }) => {
  await page.goto("/?fixture=1");
  await page.getByRole("button", { name: "Fifteen turns", exact: true }).click();
  await page.getByRole("button", { name: "Open split view" }).click();

  const left = page.getByRole("region", { name: "Left chat", exact: true });
  const right = page.getByRole("region", { name: "Right chat", exact: true });
  await expect(left.getByText("Turn 15 reply", { exact: true })).toBeVisible();
  await expect(right.getByRole("button", { name: /Fifteen turns/ })).toHaveCount(0);

  await page.getByTitle("Delete “Fifteen turns”", { exact: true }).click();
  const dialog = page.getByRole("alertdialog");
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();

  await expect(left.getByRole("button", { name: "Close Choose a chat", exact: true })).toBeVisible();
  await expect(right.getByRole("button", { name: /Fifteen turns/ })).toHaveCount(0);
  await left.getByRole("button", { name: "Close Choose a chat", exact: true }).click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /Ask about this project/ })).toBeVisible();
});

test("a minimized pane keeps streaming and its draft, and restoring returns the same layout", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const { left, right } = await openStreamingPanes(page);
  const divider = page.getByRole("separator", { name: "Resize split panes" });
  await divider.focus();
  await divider.press("ArrowLeft");
  await expect(divider).toHaveAttribute("aria-valuenow", "48");
  const before = await left.boundingBox();
  await left.getByRole("textbox", { name: "Message", exact: true }).fill("unsent left draft");

  await left.getByRole("button", { name: "Minimize pane" }).click();
  const strip = left.getByRole("button", { name: "Restore Fixture, working", exact: true });
  await expect(strip).toBeVisible();
  await expect(strip).toHaveAttribute("aria-expanded", "false");
  await expect(strip).toBeFocused();
  await expect(divider).toHaveCount(0);
  const stripBox = await strip.boundingBox();
  expect(stripBox!.width).toBeLessThan(60);
  expect(stripBox!.height).toBeGreaterThan(300);
  await expect(right.getByRole("button", { name: "Minimize pane" })).toBeDisabled();

  // The turn carries on in the pane behind the strip.
  await splitTurn(page, "left running", "delta", " and still going");
  await expect(left.locator("article").last()).toContainText("Live response to left running and still going");
  await expect(right.getByRole("button", { name: "Stop response" })).toBeEnabled();

  await strip.press("Enter");
  const composer = left.getByRole("textbox", { name: "Message", exact: true });
  await expect(composer).toBeFocused();
  await expect(composer).toHaveValue("unsent left draft");
  await expect(left.getByText("Live response to left running and still going", { exact: true })).toBeVisible();
  await expect(left.getByRole("button", { name: "Stop response" })).toBeEnabled();
  await expect(divider).toHaveAttribute("aria-valuenow", "48");
  const after = await left.boundingBox();
  expect(after!.width).toBeCloseTo(before!.width, 0);
  expect(errors).toEqual([]);
});

test("zooming a pane fills the split area, keeps the other pane streaming, and restores the layout", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const { left, right } = await openStreamingPanes(page);
  const divider = page.getByRole("separator", { name: "Resize split panes" });
  await divider.focus();
  await divider.press("ArrowRight");
  await expect(divider).toHaveAttribute("aria-valuenow", "52");
  const area = await page.locator(".zest-split-panes").boundingBox();
  const leftBefore = await left.boundingBox();

  const zoom = right.getByRole("button", { name: "Zoom pane" });
  await expect(zoom).toHaveAttribute("aria-pressed", "false");
  await zoom.click();
  await expect(zoom).toHaveAttribute("aria-pressed", "true");
  const strip = left.getByRole("button", { name: "Restore Fixture, working", exact: true });
  await expect(strip).toBeVisible();
  const zoomed = await right.boundingBox();
  expect(zoomed!.width).toBeGreaterThan(area!.width - 60);
  await expect(divider).toHaveCount(0);

  await splitTurn(page, "left running", "delta", " while zoomed away");
  await expect(left.locator("article").last()).toContainText("Live response to left running while zoomed away");
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();

  await zoom.click();
  await expect(zoom).toHaveAttribute("aria-pressed", "false");
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await expect(left.getByText("Live response to left running while zoomed away", { exact: true })).toBeVisible();
  await expect(divider).toHaveAttribute("aria-valuenow", "52");
  const leftAfter = await left.boundingBox();
  expect(leftAfter!.width).toBeCloseTo(leftBefore!.width, 0);
  expect(errors).toEqual([]);
});

test("a minimized pane that finishes reads unread until it is restored", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await left.getByRole("button", { name: "Minimize pane" }).click();
  await expect(left.getByRole("button", { name: "Restore Fixture, working", exact: true })).toBeVisible();

  // A pane that is on screen finishing counts as read.
  await splitTurn(page, "right running", "done");
  await expect(right.getByRole("button", { name: "Stop response" })).toHaveCount(0);
  expect(await unreadThreads(page)).toEqual([]);

  await splitTurn(page, "left running", "done");
  const strip = left.getByRole("button", { name: "Restore Fixture, unread", exact: true });
  await expect(strip).toBeVisible();
  await expect.poll(() => unreadThreads(page)).toHaveLength(1);

  await strip.click();
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await expect.poll(() => unreadThreads(page)).toEqual([]);
});

test("a pane hidden behind zoom reads unread when it finishes, and its strip restores the layout", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await left.getByRole("button", { name: "Zoom pane" }).click();
  const strip = right.getByRole("button", { name: "Restore Fixture, working", exact: true });
  await expect(strip).toBeVisible();

  await splitTurn(page, "right running", "done");
  await expect(right.getByRole("button", { name: "Restore Fixture, unread", exact: true })).toBeVisible();
  await expect.poll(() => unreadThreads(page)).toHaveLength(1);

  await right.getByRole("button", { name: "Restore Fixture, unread", exact: true }).click();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await expect(left.getByRole("button", { name: "Zoom pane" })).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => unreadThreads(page)).toEqual([]);
});

test("a minimized pane that needs input restores itself without taking focus", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await left.getByRole("button", { name: "Minimize pane" }).click();
  await expect(left.getByRole("button", { name: "Restore Fixture, working", exact: true })).toBeVisible();
  const rightComposer = right.getByRole("textbox", { name: "Message", exact: true });
  await rightComposer.focus();

  await splitTurn(page, "left running", "approval");

  await expect(left.getByRole("button", { name: "Allow once" })).toBeVisible();
  await expect(left.getByRole("button", { name: /^Restore / })).toHaveCount(0);
  await expect(rightComposer).toBeFocused();
  // A pane waiting on you is not put away again until you answer.
  await expect(left.getByRole("button", { name: "Minimize pane" })).toBeDisabled();
  await left.getByRole("button", { name: "Allow once" }).click();
  await expect(left.getByRole("button", { name: "Minimize pane" })).toBeEnabled();

  // A question brings a pane back the same way.
  await right.getByRole("button", { name: "Minimize pane" }).click();
  await expect(right.getByRole("button", { name: "Restore Fixture, working", exact: true })).toBeVisible();
  await splitTurn(page, "right running", "question");
  await expect(right.getByText("Which branch should this split pane use?")).toBeVisible();
  await expect(right.getByRole("button", { name: /^Restore / })).toHaveCount(0);
});

test("an approval behind a zoomed pane toasts, shows on its strip, and Open brings the pane back", async ({ page }) => {
  // Zest only toasts while it is the focused window; a headless page need not report focus.
  await page.addInitScript(() => {
    Object.defineProperty(Document.prototype, "hasFocus", { configurable: true, value: () => true });
  });
  const { left, right } = await openStreamingPanes(page);
  const zoom = right.getByRole("button", { name: "Zoom pane" });
  await zoom.click();
  await expect(left.getByRole("button", { name: "Restore Fixture, working", exact: true })).toBeVisible();

  await splitTurn(page, "left running", "approval");

  // Zoom does not undo itself: the strip shows the attention state instead.
  await expect(left.getByRole("button", { name: "Restore Fixture, awaiting approval", exact: true })).toBeVisible();
  await expect(zoom).toHaveAttribute("aria-pressed", "true");
  const toast = page.locator('[data-slot="toast"]').filter({ hasText: "Approval needed" });
  await expect(toast).toBeVisible();
  await toast.getByRole("button", { name: "Open", exact: true }).click();
  await expect(left.getByRole("button", { name: "Allow once" })).toBeVisible();
  await expect(zoom).toHaveAttribute("aria-pressed", "false");
  await expect(toast).toHaveCount(0);
});

test("strips are short rows in the narrow layout", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await page.setViewportSize({ width: 650, height: 900 });
  await left.getByRole("button", { name: "Minimize pane" }).click();
  const strip = left.getByRole("button", { name: "Restore Fixture, working", exact: true });
  await expect(strip).toBeVisible();

  const area = await page.locator(".zest-split-panes").boundingBox();
  const row = await strip.boundingBox();
  expect(row!.height).toBeLessThanOrEqual(40);
  expect(row!.width).toBeGreaterThan(area!.width - 8);
  const below = await right.boundingBox();
  expect(below!.y).toBeGreaterThanOrEqual(row!.y + row!.height - 1);
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();

  await right.getByRole("button", { name: "Zoom pane" }).click();
  await expect(strip).toBeVisible();
  await strip.click();
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await expect(right.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
});

test("adding a pane brings minimized panes back, and the last visible pane cannot be minimized", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await left.getByRole("button", { name: "Minimize pane" }).click();
  const lastVisible = right.getByRole("button", { name: "Minimize pane" });
  await expect(lastVisible).toBeDisabled();
  await expect(lastVisible).toHaveAttribute("title", "Keep at least one pane visible");

  await right.getByRole("button", { name: "Add pane" }).click();

  await expect(page.locator("[data-split-strip]")).toHaveCount(0);
  await expect(left.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  const bottom = page.getByRole("region", { name: "Right bottom chat", exact: true });
  await expect(bottom.getByRole("button", { name: "New chat in this project", exact: true })).toBeVisible();
});

test("closing the zoomed pane gives the other pane its space back", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await right.getByRole("button", { name: "Zoom pane" }).click();
  await expect(left.getByRole("button", { name: /^Restore Fixture/ })).toBeVisible();

  await right.getByRole("button", { name: /^Close / }).click();

  await expect(page.locator("[data-split-strip]")).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
});

test("a minimized pane stays minimized when split view is left and reopened", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  await left.getByRole("button", { name: "Minimize pane" }).click();
  await expect(left.getByRole("button", { name: /^Restore Fixture/ })).toBeVisible();

  await right.getByRole("button", { name: "Continue in single view" }).click();
  await expect(page.getByRole("region", { name: "Split workspace", exact: true })).toHaveCount(0);
  await page.locator("aside").first().getByRole("button", { name: "Split view with 2 chats", exact: true }).click();

  await expect(left.getByRole("button", { name: /^Restore Fixture/ })).toBeVisible();
});

test("pane shortcuts and palette commands zoom, minimize, and restore", async ({ page }) => {
  const { left, right } = await openStreamingPanes(page);
  const zoom = right.getByRole("button", { name: "Zoom pane" });

  await right.getByRole("textbox", { name: "Message", exact: true }).focus();
  await page.keyboard.press("Control+Shift+Enter");
  await expect(zoom).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Control+Shift+Enter");
  await expect(zoom).toHaveAttribute("aria-pressed", "false");

  await left.getByRole("textbox", { name: "Message", exact: true }).focus();
  await page.keyboard.press("Control+Shift+H");
  await expect(left.getByRole("button", { name: /^Restore Fixture/ })).toBeVisible();

  // The palette acts on the pane that had focus when it opened, here the strip.
  await page.keyboard.press("Control+K");
  const palette = page.getByRole("dialog", { name: "Search", exact: true });
  await palette.getByRole("option", { name: /^Zoom pane/ }).click();
  await expect(left.getByRole("button", { name: "Zoom pane" })).toHaveAttribute("aria-pressed", "true");
  await expect(right.getByRole("button", { name: /^Restore Fixture/ })).toBeVisible();

  await page.keyboard.press("Control+K");
  await palette.getByRole("option", { name: /^Restore all panes/ }).click();
  await expect(page.locator("[data-split-strip]")).toHaveCount(0);
  await expect(zoom).toHaveAttribute("aria-pressed", "false");
  await expect(left.getByRole("button", { name: "Zoom pane" })).toHaveAttribute("aria-pressed", "false");
});

test("a rebound zoom shortcut replaces the default", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("zest.keybindings", JSON.stringify({ "split.zoom": "Mod+Shift+Y" }));
  });
  const { right } = await openStreamingPanes(page);
  const zoom = right.getByRole("button", { name: "Zoom pane" });
  await right.getByRole("textbox", { name: "Message", exact: true }).focus();

  await page.keyboard.press("Control+Shift+Enter");
  await expect(zoom).toHaveAttribute("aria-pressed", "false");
  await page.keyboard.press("Control+Shift+Y");
  await expect(zoom).toHaveAttribute("aria-pressed", "true");
});
