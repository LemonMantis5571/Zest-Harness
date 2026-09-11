import { expect, test } from "@playwright/test";

test("both panes stream together and stopping one leaves the other running", async ({ page }) => {
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
