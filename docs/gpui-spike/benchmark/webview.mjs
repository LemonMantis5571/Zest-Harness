// Build an offline copy of the release UI or drive it in a dedicated WebView2 instance.
import fs from "node:fs/promises";
import path from "node:path";
import http from "node:http";
import { fileURLToPath } from "node:url";
import { build } from "vite";
import { chromium } from "playwright";

const here = path.dirname(fileURLToPath(import.meta.url));
const ui = path.resolve(here, "../../../crates/desktop/ui");
const [action, dist, casePath, port, readyPath] = process.argv.slice(2);
if (action === "build") {
  await build({ root: ui, configFile: path.join(ui, "vite.config.ts"), logLevel: "warn",
    resolve: { alias: [{ find: /^@\/lib\/backend$/, replacement: path.join(here, "renderer.mjs") }] },
    build: { outDir: dist, emptyOutDir: false } });
} else if (action === "run") {
  const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".png": "image/png" };
  const server = http.createServer(async (req, res) => {
    try {
      const route = new URL(req.url, "http://localhost").pathname;
      const name = route === "/" ? "/index.html" : route;
      const asset = path.resolve(dist, "." + name);
      if (!asset.startsWith(path.resolve(dist) + path.sep)) throw new Error("Unknown asset");
      const data = await fs.readFile(route === "/workload.json" ? casePath : asset);
      res.writeHead(200, { "Content-Type": types[path.extname(name)] || "application/octet-stream" });
      res.end(data);
    } catch { res.writeHead(404); res.end(); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.connectOverCDP("http://127.0.0.1:" + port, { timeout: 10_000 });
    const context = browser.contexts()[0];
    const page = context.pages()[0] || await context.waitForEvent("page");
    await page.setViewportSize({ width: 1000, height: 900 });
    const pageErrors = [];
    page.on("pageerror", error => pageErrors.push(String(error)));
    await page.goto("http://127.0.0.1:" + server.address().port + "/", { timeout: 30_000 });
    await page.waitForFunction(() => typeof window.runDesktopBenchmark === "function", null, { timeout: 20_000 });
    await page.locator('[data-slot="message-scroller-viewport"]').waitFor();
    await page.waitForTimeout(3000);
    await fs.writeFile(readyPath, "ready");
    const result = await page.evaluate(() => window.runDesktopBenchmark());
    if (pageErrors.length) throw new Error(pageErrors.join("\n"));
    console.log(JSON.stringify(result));
    if (result.shell) {
      // Keep the measured app open for UI Automation after process sampling finishes.
      const until = Date.now() + 30000;
      while (!(await fs.stat(readyPath + ".done").catch(() => false))) {
        if (Date.now() > until) throw new Error("Shell accessibility probe timed out");
        await new Promise(resolve => setTimeout(resolve, 150));
      }
    }
  } finally {
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
} else {
  throw new Error("Use build or run");
}
