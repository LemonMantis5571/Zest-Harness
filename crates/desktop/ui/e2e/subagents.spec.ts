import { expect, test, type Page } from "@playwright/test";

function collectErrors(page: Page) {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  return errors;
}

function subagent(page: Page, title: string) {
  return page.locator("[data-slot=subagent]").filter({ hasText: title });
}

test("every subagent gets its own line with its steps nested under it", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto("/?fixture=1&scenario=subagents");

  // Three kinds of subagent, none folded into the "Ran 2 lookups" run.
  await expect(page.locator("[data-slot=subagent]")).toHaveCount(3);
  await expect(page.getByRole("button", { name: "Ran 2 lookups", exact: true })).toBeVisible();

  const worker = subagent(page, "Ran Codex as a subagent");
  await expect(worker.getByText("gpt-5.1-codex")).toBeVisible();
  await expect(worker.getByText("48s")).toBeVisible();
  await worker.getByRole("button", { name: /Ran Codex as a subagent/ }).click();
  await expect(worker.getByText("The cache key ignores the locale")).toBeVisible();
  await expect(worker.getByRole("button", { name: "Open changes" })).toBeVisible();

  const agent = subagent(page, "Ran subagent: Map the cacheKey callers");
  await expect(agent.getByText("Grep cacheKey in src")).toBeHidden();
  await agent.getByRole("button", { name: /Ran subagent: Map the cacheKey callers/ }).click();
  for (const step of ["Grep cacheKey in src", "Read src/images.ts", "Bash npm test -- cache"]) {
    await expect(agent.getByText(step, { exact: true })).toBeVisible();
  }

  // Steps keep the order they happened in: the second thought sits between
  // the worker and the feature card, not above every tool.
  const lines = await page
    .locator("[data-slot=assistant-trace]")
    .first()
    .locator(":scope > *")
    .allInnerTexts();
  const order = lines.map((text) => text.split("\n")[0]);
  expect(order.findIndex((text) => text.includes("Ran Codex"))).toBeLessThan(
    order.findLastIndex((text) => text.startsWith("Thought"))
  );
  expect(order.findLastIndex((text) => text.startsWith("Thought"))).toBeLessThan(
    order.findIndex((text) => text.includes("Add the fixture lifecycle"))
  );
  expect(errors).toEqual([]);
});

test("a feature card line follows its board job and opens it", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto("/?fixture=1&scenario=subagents");

  const card = subagent(page, "Add the fixture lifecycle");
  await expect(card).toContainText("Waiting for approval: Add the fixture lifecycle");
  await card.getByRole("button", { name: /Add the fixture lifecycle/ }).click();
  await card.getByRole("button", { name: "Open in Delegation" }).click();

  await expect(page.getByRole("tab", { name: "Delegation", exact: true })).toHaveAttribute(
    "aria-selected",
    "true"
  );
  const job = page.locator("#delegation-job-fixture-delegation-1");
  await expect(job).toHaveAttribute("data-focused", "true");
  await job.getByRole("button", { name: "Approve", exact: true }).click();

  // The chat line moves with the job; it is not frozen at "card created".
  await expect(card).toContainText("Ready to apply: Add the fixture lifecycle");
  await expect(card.getByText("Reviewed with Claude Code")).toBeVisible();
  await expect(card.getByText("Worker created a deterministic fixture change.")).toBeVisible();
  expect(errors).toEqual([]);
});

test("a live subagent streams its steps under it and settles", async ({ page }) => {
  const errors = collectErrors(page);
  await page.goto("/?fixture=1&scenario=subagents");
  await expect(page.locator("[data-slot=subagent]")).toHaveCount(3);

  const composer = page.locator("#zest-composer-input");
  await composer.fill("Map the callers again");
  await composer.press("Enter");

  const live = subagent(page, "subagent: Map the callers");
  await expect(live).toContainText("Ran subagent: Map the callers", { timeout: 10_000 });
  await expect(page.getByText("The agent mapped three callers of cacheKey.")).toBeVisible();
  // It ran open and folds once it settled; opening it shows its own steps.
  await live.getByRole("button", { name: /Ran subagent: Map the callers/ }).click();
  await expect(live.getByText("Grep cacheKey in src", { exact: true })).toBeVisible();
  await expect(live.getByText("Read src/images.ts", { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});
