import { expect, test } from "@playwright/test";

test("streaming code preserves final text, language swaps and unmount", async ({ page }) => {
  await page.goto("/e2e/fixtures/streaming.html");
  await page.waitForFunction(() => window.fixtureReady);
  await page.evaluate(() => window.renderCode("const first = 1;", true));
  await expect(page.locator("[data-slot=code-block-code]")).toHaveText("const first = 1;");
  // Once an earlier prefix is colored, growing its final line must remain readable.
  await expect(page.locator("[data-slot=code-block-token][style]").first()).toBeVisible();
  await page.evaluate(() => window.renderCode("const first = 123;", true));
  await expect(page.locator("[data-slot=code-block-code]")).toHaveText("const first = 123;");
  await page.evaluate(() => window.renderCode("const first = 12;\nconst second = 3;", false));
  await expect(page.locator("[data-slot=code-block-line-content]")).toHaveText([
    "const first = 12;", "const second = 3;",
  ]);
  await expect(page.locator("[data-slot=code-block-token][style]").first()).toBeVisible();
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.getByRole("button", { name: "Copy code", exact: true }).click();
  // Windows clipboard text uses CRLF; compare the logical source on both hosts.
  await expect.poll(() => page.evaluate(async () => (await navigator.clipboard.readText()).replace(/\r\n/g, "\n"))).toBe("const first = 12;\nconst second = 3;");
  await page.evaluate(() => window.renderCode("print('different file')", false, "python"));
  await expect(page.locator("[data-slot=code-block-code]")).toHaveText("print('different file')");
  await page.evaluate(() => {
    window.renderCode("const pending = true;", true);
    window.removeCode();
  });
  await expect(page.locator("[data-slot=code-block]")).toHaveCount(0);
  await page.evaluate(() => window.renderCode("fallback text", false, "unknown-language"));
  await expect(page.locator("[data-slot=code-block-code]")).toHaveText("fallback text");
});

test("typing remains usable while a code block grows", async ({ page }) => {
  await page.goto("/e2e/fixtures/streaming.html");
  await page.waitForFunction(() => window.fixtureReady);
  await page.evaluate(() => { window.streamRun = window.measureStream(20); });
  await page.getByRole("textbox", { name: "Typing probe" }).pressSequentially("keep typing", { delay: 20 });
  await expect(page.getByRole("textbox", { name: "Typing probe" })).toHaveValue("keep typing");
  await page.evaluate(() => window.streamRun);
});

test("measure growing fences (opt-in)", async ({ page, browser }, testInfo) => {
  test.skip(process.env.ZEST_PERF !== "1", "Run explicitly with ZEST_PERF=1 and --workers=1");
  test.setTimeout(180_000);
  await page.goto("/e2e/fixtures/streaming.html");
  await page.waitForFunction(() => window.fixtureReady);
  await page.evaluate(() => window.warmGrammar());
  const samples = [];
  for (const kib of [20, 100]) {
    for (let run = 0; run < 5; run++) {
      await page.evaluate(() => window.removeCode());
      const result = await page.evaluate((size) => window.measureStream(size), kib);
      const text = await page.locator("[data-slot=code-block-line-content]").allTextContents();
      expect(text.join("\n")).toBe(result.source);
      const { source: _source, ...metrics } = result;
      samples.push({ run, ...metrics });
    }
  }
  const report = { browser: browser.version(), cadenceMs: 25, chunks: 40, samples };
  console.log(JSON.stringify(report));
  await testInfo.attach("streaming-measurements", { body: JSON.stringify(report, null, 2), contentType: "application/json" });
});

declare global {
  interface Window {
    fixtureReady: boolean;
    renderCode(code: string, streaming?: boolean, language?: string): void;
    removeCode(): void;
    warmGrammar(): Promise<unknown>;
    streamRun: Promise<unknown>;
    measureStream(kib: number): Promise<{
      kib: number; elapsed: number; longTasks: number; longTaskMs: number;
      maxEventLoopLagMs: number; source: string;
    }>;
  }
}
