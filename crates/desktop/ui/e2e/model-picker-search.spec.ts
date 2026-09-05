import { expect, test } from "@playwright/test";

test("search preserves selection, exposes capabilities and has a clear empty state", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=model-catalogue");
  const trigger = page.getByTitle("Model and provider", { exact: true });
  await trigger.click();
  const models = page.getByRole("listbox", { name: "Model", exact: true });
  await expect(models.getByRole("option")).toHaveCount(61);
  const search = page.getByRole("searchbox");
  await search.fill("RESEARCH-MODEL-02");
  await expect(models.getByRole("option")).toHaveCount(1);
  await expect(models).toContainText("128k context");
  await expect(models).toContainText("Vision");
  await expect(trigger).toHaveText("5.6 Sol · High");
  await search.fill("research provider");
  await expect(models.getByRole("option")).toHaveCount(1);
  await expect(models).toContainText("1.0M context");
  await search.fill("nothing-matches");
  await expect(models.getByRole("option")).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveText("No models match your search.");
  await page.getByRole("button", { name: "Clear", exact: true }).click();
  await expect(search).toHaveValue("");
  await expect(models.getByRole("option")).toHaveCount(61);
  await expect(trigger).toHaveText("5.6 Sol · High");
});

test("search arrows select a no-effort model without inventing unknown context", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=model-catalogue");
  const trigger = page.getByTitle("Model and provider", { exact: true });
  await trigger.click();
  await page.getByRole("searchbox").fill("research-model-01");
  const option = page.getByRole("listbox", { name: "Model", exact: true }).getByRole("option");
  await expect(option).toContainText("Text only");
  await expect(option).not.toContainText("context");
  await page.keyboard.press("ArrowDown");
  await expect(option).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("searchbox")).toHaveCount(0);
  await expect(trigger).toHaveText("research-model-01-with-a-long-descriptive-name");
  await trigger.click();
  await expect(page.getByRole("listbox", { name: "Effort" })).toHaveCount(0);
});

for (const width of [1280, 720]) {
  test(`long catalogue stays bounded at ${width}px and effort remains visible`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 720 });
    await page.goto("/?fixture=1&scenario=model-catalogue");
    await page.getByTitle("Model and provider", { exact: true }).click();
    const panel = page.getByRole("dialog", { name: "Model and provider", exact: true });
    await expect(page.getByRole("listbox", { name: "Effort" })).toBeInViewport();
    await expect(page.getByRole("button", { name: "Reset model and effort to default" })).toBeInViewport();
    expect(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    const bounds = await panel.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    expect(await page.getByRole("listbox", { name: "Model", exact: true }).evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`model-picker-${width}.png`) });
  });
}
