import { expect, test, type Page } from "@playwright/test";
import { measureUiElements } from "../../../../scripts/zest-ui-measure.mjs";

async function measure(page: Page, selector: string) {
  await page.evaluate(() => document.fonts.ready);
  return page.evaluate(measureUiElements, selector);
}

for (const theme of ["zest", "nights", "oceanic"]) {
  for (const font of ["geist", "abc-arizona", "inter", "plus-jakarta", "jetbrains-mono", "fira-code", "system"]) {
  test(`${theme}/${font}: compact composer and context controls remain usable`, async ({ page }, testInfo) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.setViewportSize({ width: 840, height: 560 });
    await page.addInitScript(({ theme, font }) => {
      localStorage.setItem("zest.selected_theme", theme);
      localStorage.setItem("zest.selected_font", font);
    }, { theme, font });
    await page.goto("/?fixture=1&scenario=ui-stress");
    await expect(page.locator("html")).toHaveAttribute("data-font", font);
    const meter = page.getByTitle("Context usage", { exact: true });
    await expect(meter).toContainText("85%");
    const folder = page.getByRole("button", { name: "/fixture/a-project-with-a-deliberately-long-name-for-layout-checks", exact: true });
    await expect(folder).toContainText("a-project-with-a-deliberately-long-name-for-layout-checks");
    const folderBounds = await folder.boundingBox();
    if (!folderBounds) throw new Error("Folder control is not measurable");
    expect(folderBounds.height).toBeGreaterThanOrEqual(24);
    expect(folderBounds.x + folderBounds.width).toBeLessThanOrEqual(840);
    const pullRequest = page.getByRole("link", { name: /#13/ }).last();
    await expect(pullRequest).toBeInViewport();
    const linkBounds = await pullRequest.boundingBox();
    if (!linkBounds) throw new Error("Pull request control is not measurable");
    expect(linkBounds.height).toBeGreaterThanOrEqual(24);
    await meter.click();
    const dialog = page.getByRole("dialog", { name: "Context usage", exact: true });
    await expect(dialog.getByText("This conversation will be compacted automatically after this turn.")).toBeVisible();
    const elements = await measure(page, 'button[title="Context usage"], [role="dialog"] button, [role="dialog"] p');
    expect(elements).toHaveLength(4);
    for (const element of elements) {
      expect(element.inViewport, element.name).toBe(true);
      expect(element.clipping, element.name).toEqual([]);
      expect(element.contrast, element.name).toBeGreaterThanOrEqual(4.5);
    }
    for (const control of elements.slice(0, 2)) {
      expect(control.bounds.width, control.name).toBeGreaterThanOrEqual(24);
      expect(control.bounds.height, control.name).toBeGreaterThanOrEqual(24);
    }
    if (font === "geist") await page.screenshot({ path: testInfo.outputPath(`${theme}-context.png`) });
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await page.getByRole("button", { name: "Auto", exact: true }).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio")).toHaveCount(5);
    const [menuBounds] = await measure(page, '[role="menu"]');
    expect(menuBounds.inViewport).toBe(true);
    expect(menuBounds.clipping).toEqual([]);
    const menuText = await measure(page, '[role="menu"] span');
    for (const text of menuText.filter((element) => element.name !== "span")) {
      expect(text.contrast, text.name).toBeGreaterThanOrEqual(4.5);
    }
    await page.keyboard.press("1");
    await expect(page.getByRole("button", { name: "Manual", exact: true })).toBeVisible();
    await expect(menu).toHaveCount(0);
    for (const chip of [page.getByRole("button", { name: "Manual", exact: true }), meter]) {
      await expect(chip).toBeInViewport();
      const bounds = await chip.boundingBox();
      if (!bounds) throw new Error("Composer chip is not measurable");
      expect(bounds.width).toBeGreaterThanOrEqual(24);
      expect(bounds.height).toBeGreaterThanOrEqual(24);
    }
    expect(errors).toEqual([]);
  });
  }

  test(`${theme}: split model picker stays inside the window when opened and resized`, async ({ page }) => {
    await page.setViewportSize({ width: 840, height: 560 });
    await page.addInitScript((id) => localStorage.setItem("zest.selected_theme", id), theme);
    await page.goto("/?fixture=1&scenario=model-catalogue");
    await page.getByTitle("Select model", { exact: true }).click();
    await expect.poll(async () => (await measure(page, '[role="dialog"]')).map((element) => ({ inViewport: element.inViewport, clipping: element.clipping }))).toEqual([{ inViewport: true, clipping: [] }]);
    await page.getByRole("dialog").getByRole("searchbox", { name: "Search models and providers" }).fill("no model matches this");
    await expect(page.getByText("No models match your search.", { exact: true })).toBeVisible();
    await expect.poll(async () => (await measure(page, '[role="dialog"]')).map((element) => element.clipping)).toEqual([[]]);
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Open split view", exact: true }).click();
    const left = page.getByRole("region", { name: "Left chat", exact: true });
    const right = page.getByRole("region", { name: "Right chat", exact: true });
    await left.getByRole("button", { name: "Fork into other pane", exact: true }).click();
    await right.getByTitle("Select model", { exact: true }).click();
    const panel = right.getByRole("dialog", { name: "Model and provider", exact: true });
    await expect(panel).toBeVisible();
    for (const size of [{ width: 840, height: 560 }, { width: 1280, height: 800 }, { width: 720, height: 560 }]) {
      await page.setViewportSize(size);
      await expect.poll(async () => (await measure(page, '[role="dialog"]')).map((element) => ({ inViewport: element.inViewport, clipping: element.clipping }))).toEqual([{ inViewport: true, clipping: [] }]);
      await expect(panel.getByRole("button", { name: "Reset model and effort to default" })).toBeInViewport();
    }
    await panel.getByRole("option", { name: "Low", exact: true }).click();
    await expect(panel).toHaveCount(0);
    await expect(right.getByTitle("Select model", { exact: true })).toBeFocused();
  });
}

test("UI measurement composes transparency and reports unsupported contrast honestly", async ({ page }) => {
  await page.setContent('<div style="background: #fff"><p id="black" style="color:#000">Black</p><p id="gray" style="color:rgba(0,0,0,0.5)">Gray</p><div style="opacity:0.5"><p id="faded" style="background:#fff;color:#000">Faded</p></div><p id="gradient" style="background:linear-gradient(#fff,#000)">Gradient</p><p id="hidden" hidden>Hidden</p></div>');
  const elements = await measure(page, "p");
  expect(elements).toHaveLength(4);
  expect(elements[0].contrast).toBe(21);
  expect(elements[1].contrast).toBeCloseTo(4.0, 0);
  expect(elements[2].contrast).toBeNull();
  expect(elements[2].contrastUnknown).toContain("opacity");
  expect(elements[3].contrast).toBeNull();
  expect(elements[3].contrastUnknown).toContain("image");
});

test("context error recovery repositions the replacement popover", async ({ page }) => {
  await page.setViewportSize({ width: 840, height: 560 });
  await page.goto("/?fixture=1&scenario=context-recovery");
  const meter = page.getByTitle("Context usage", { exact: true });
  await expect(meter).toContainText("left");
  await meter.click();
  const dialog = page.getByRole("dialog", { name: "Context usage", exact: true });
  await expect(dialog).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-fail-context")));
  await page.locator("#zest-composer-input").fill("refresh context while unavailable");
  await page.locator("#zest-composer-input").press("Enter");
  await expect(page.getByText("Context —", { exact: true })).toBeVisible();
  await expect(dialog).toHaveCount(0);
  await page.evaluate(() => window.dispatchEvent(new Event("zest:fixture-recover-context")));
  await page.locator("#zest-composer-input").fill("refresh context after recovery");
  await page.locator("#zest-composer-input").press("Enter");
  await expect(dialog).toBeVisible();
  for (const size of [{ width: 720, height: 420 }, { width: 840, height: 560 }]) {
    await page.setViewportSize(size);
    await expect.poll(async () => (await measure(page, '[role="dialog"]')).map((element) => ({ inViewport: element.inViewport, clipping: element.clipping }))).toEqual([{ inViewport: true, clipping: [] }]);
  }
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toHaveCount(0);
});
