import { expect, test } from "@playwright/test";

test("the context meter keeps its reading while a turn is running", async ({ page }) => {
  // split-streaming keeps a send in flight until Stop, and the fixture answers
  // context requests with `busy` during it, as the desktop does.
  await page.addInitScript(() => {
    localStorage.setItem("zest.responseBlockStreaming.v1", "false");
  });
  await page.goto("/?fixture=1&scenario=split-streaming");
  const meter = page.getByTitle("Context usage", { exact: true });
  const blank = page.getByText("Context —", { exact: true });
  await expect(meter).toContainText("left");

  const stop = page.getByRole("button", { name: "Stop", exact: true });
  await page.locator("#zest-composer-input").fill("measure me");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  // The streamed reply adds a message, which refetches the meter mid-turn.
  await expect(page.getByText("Live response to measure me", { exact: true })).toBeVisible();
  await expect(stop).toBeEnabled();
  await expect(meter).toContainText("left");
  await expect(blank).toHaveCount(0);

  await stop.click();
  await expect(stop).toHaveCount(0);
  await expect(meter).toContainText("left");
});
