import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { expect, test, type Locator, type Page } from "@playwright/test";

function card(page: Page, title = "Offline counter"): Locator {
  return page.getByRole("region", { name: title, exact: true });
}

function documentFrame(container: Locator) {
  return container.frameLocator("iframe").frameLocator("iframe");
}

async function send(page: Page, text: string) {
  const composer = page.locator("#zest-composer-input");
  await composer.fill(`HTML example\n\n${text}`);
  await composer.press("Enter");
}

test("HTML stays inert until Open; controls reset, export and preserve the live frame", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?fixture=1&scenario=html-artifacts");
  const counter = card(page);
  await expect(counter).toBeVisible();
  await expect(page.locator("[data-slot=html-artifact] iframe")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Ran 2 lookups", exact: true })).toHaveCount(2);
  await counter.getByRole("button", { name: "Source", exact: true }).click();
  await expect(counter.locator("[data-slot=html-source]")).toContainText("id=\"increment\"");
  await expect(counter.locator("iframe")).toHaveCount(0);
  await counter.getByRole("button", { name: "Open", exact: true }).click();
  const content = documentFrame(counter);
  await expect(content.locator("#count")).toHaveText("0");
  await expect(counter.locator("iframe")).toHaveAttribute("sandbox", "allow-scripts");
  await expect(counter.locator("iframe")).not.toHaveAttribute("srcdoc", /.+/);
  await content.getByRole("button", { name: "Increment" }).click();
  await expect(content.locator("#count")).toHaveText("1");
  await content.getByLabel("Step", { exact: true }).fill("2");
  await content.getByRole("button", { name: "Increment" }).click();
  await expect(content.locator("#count")).toHaveText("3");
  await content.getByLabel("Step", { exact: true }).fill("1");
  const firstUrl = await counter.locator("iframe").getAttribute("src");
  expect(firstUrl).toBeTruthy();
  await counter.getByRole("button", { name: "Source", exact: true }).click();
  await expect(content.locator("#count")).toHaveText("3");
  await counter.getByRole("button", { name: "Expand", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Offline counter", exact: true });
  await expect(dialog).toBeVisible();
  await expect(documentFrame(dialog).locator("#count")).toHaveText("3");
  await page.keyboard.press("Escape");
  await expect(counter).toBeVisible();
  await expect(content.locator("#count")).toHaveText("3");
  await counter.getByRole("button", { name: "Reset", exact: true }).click();
  await expect(content.locator("#count")).toHaveText("0");
  await expect(counter.locator("iframe")).not.toHaveAttribute("src", firstUrl!);
  expect((await page.request.get(firstUrl!)).status()).toBe(404);
  const downloadPromise = page.waitForEvent("download");
  await counter.getByRole("button", { name: "Save HTML", exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("Offline counter.html");
  const downloadPath = await download.path();
  expect(downloadPath).toBeTruthy();
  const exported = await readFile(downloadPath!, "utf8");
  expect(exported).toContain("id=\"increment\"");
  expect(exported.indexOf("RTCPeerConnection")).toBeLessThan(exported.indexOf("id=\"increment\""));
  expect(exported).toContain("connect-src 'none'");
  expect(errors).toEqual([]);
});

test("restored source reopens after thread switching and page reload without persisted viewer tokens", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=html-artifacts");
  await card(page).getByRole("button", { name: "Open", exact: true }).click();
  const firstUrl = await card(page).locator("iframe").getAttribute("src");
  await expect(documentFrame(card(page)).locator("#count")).toHaveText("0");
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  await expect(card(page)).toHaveCount(0);
  await expect.poll(async () => (await page.request.get(firstUrl!)).status()).toBe(404);
  await page.getByRole("button", { name: /^Fixture\. Pull request/ }).first().click();
  await expect(card(page)).toBeVisible();
  await expect(card(page).locator("iframe")).toHaveCount(0);
  await page.reload();
  await expect(card(page)).toBeVisible();
  await expect(page.locator("[data-slot=html-artifact] iframe")).toHaveCount(0);
  await card(page).getByRole("button", { name: "Open", exact: true }).click();
  await expect(documentFrame(card(page)).locator("#count")).toHaveText("0");
  await expect(card(page).locator("iframe")).not.toHaveAttribute("src", firstUrl!);
});

test("native publication events create an interactive card and retain it on chat reopen", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=html-artifacts");
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  await page.locator("#zest-composer-input").fill("Publish an HTML artifact");
  await page.locator("#zest-composer-input").press("Enter");
  const published = card(page, "Published counter");
  await expect(published).toBeVisible();
  await expect(published.locator("iframe")).toHaveCount(0);
  await published.getByRole("button", { name: "Open", exact: true }).click();
  await documentFrame(published).getByRole("button", { name: "Increment" }).click();
  await expect(documentFrame(published).locator("#count")).toHaveText("1");
  await page.getByRole("button", { name: /^Fixture\. Pull request/ }).first().click();
  await expect(published).toHaveCount(0);
  // Existing chat history restores the same tool metadata, not the viewer URL.
  await page.getByRole("button", { name: /Publish an HTML artifact/ }).first().click();
  await expect(published).toBeVisible();
  await expect(published.locator("iframe")).toHaveCount(0);
});

test("only a completed dedicated zest-html fence becomes an opt-in card", async ({ page }) => {
  await page.goto("/?fixture=1&scenario=html-artifacts");
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  await send(page, "```html\n<button>ordinary</button>\n```\n\n```zest-html\n<button>partial</button>");
  await expect(page.locator("[data-slot=code-block]")).toHaveCount(2);
  await expect(page.locator("[data-slot=html-artifact]")).toHaveCount(0);
  await send(page, "```zest-html Explicit counter\n<button onclick=\"this.textContent='clicked'\">Click me</button>\n```");
  const explicit = card(page, "Explicit counter");
  await expect(explicit).toBeVisible();
  await expect(explicit).toContainText("Preview not run");
  await expect(explicit.locator("iframe")).toHaveCount(0);
  await explicit.getByRole("button", { name: "Open", exact: true }).click();
  await expect(explicit).toContainText("Running offline");
  await documentFrame(explicit).getByRole("button", { name: "Click me" }).click();
  await expect(documentFrame(explicit).getByRole("button", { name: "clicked" })).toBeVisible();
});

for (const fence of ["```", "~~~"]) {
  const markerName = fence.startsWith("`") ? "backtick" : "tilde";
  for (const newline of ["\n", "\r\n"]) {
    test(`${markerName} fence preserves raw HTML in Source, execution and Save (${JSON.stringify(newline)})`, async ({ page }) => {
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto("/?fixture=1&scenario=html-artifacts");
      await page.getByRole("button", { name: "New chat", exact: true }).first().click();
      const paths = ["C:/example/chart.png", String.raw`C:\example\chart.png`, "file:///C:/example/chart.png", "![chart](C:/example/chart.png)"];
      const html = [
        "  ",
        '<button id="show">Show paths</button><pre id="paths"></pre>',
        "<script>",
        "const paths = [",
        ...paths.map((path) => `  ${JSON.stringify(path)},`),
        "];",
        'document.getElementById("show").onclick = () => document.getElementById("paths").textContent = paths.join(" | ");',
        "</script>",
        "\t ",
        "",
      ].join(newline);
      // A textarea normalizes CRLF. Emit a raw assistant response through the fixture instead.
      await page.evaluate(async (text) => {
        const modulePath = "/src/lib/backend.ts";
        const { getBackend }: typeof import("../src/lib/backend") = await import(modulePath);
        await getBackend().sendMessage(text);
      }, `HTML example${newline}${newline}${fence}zest-html Path strings${newline}${html}${newline}${fence}`);
      const artifact = card(page, "Path strings");
      await expect(artifact).toBeVisible();
      await expect(artifact.locator("iframe")).toHaveCount(0);
      await artifact.getByRole("button", { name: "Source", exact: true }).click();
      expect(await artifact.locator("[data-slot=html-source]").textContent()).toBe(html);
      await artifact.getByRole("button", { name: "Open", exact: true }).click();
      const frame = documentFrame(artifact);
      await frame.getByRole("button", { name: "Show paths", exact: true }).click();
      await expect(frame.locator("#paths")).toHaveText(paths.join(" | "));
      const wrapperUrl = await artifact.locator("iframe").getAttribute("src");
      const response = await page.request.get(`${wrapperUrl}/document`);
      expect((await response.text()).endsWith(html)).toBe(true);
      expect(response.headers()["content-security-policy"]).toContain("sandbox allow-scripts");
      expect(await artifact.locator("[data-slot=html-source]").textContent()).toBe(html);
      const downloadPromise = page.waitForEvent("download");
      await artifact.getByRole("button", { name: "Save HTML", exact: true }).click();
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toBe("Path strings.html");
      const downloadPath = await download.path();
      if (!downloadPath) throw new Error("HTML download did not produce a file.");
      expect((await readFile(downloadPath, "utf8")).endsWith(html)).toBe(true);
      expect(errors).toEqual([]);
    });
  }
}

test("a closed fence still stays inert while its assistant turn streams", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("zest.responseBlockStreaming.v1", "false"));
  await page.goto("/?fixture=1&scenario=split-streaming");
  await send(page, "```zest-html Streaming\n<button>streamed</button>\n```");
  await expect(page.locator("[data-slot=code-block]")).toContainText("streamed");
  await expect(page.locator("[data-slot=html-artifact]")).toHaveCount(0);
  await page.getByRole("button", { name: /^Stop(?: response)?$/ }).click();
});

test("HTTP policy confines network, local access, parent, IPC, workers, popups and navigation", async ({ page, context }) => {
  const hits: string[] = [];
  const server = createServer((request, response) => {
    hits.push(request.url ?? "unknown");
    response.end("canary");
  });
  server.on("upgrade", (request, socket) => { hits.push(request.url ?? "upgrade"); socket.destroy(); });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Canary did not start.");
  const canary = `http://127.0.0.1:${address.port}`;
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  try {
    await page.goto("/?fixture=1&scenario=html-artifacts");
    await page.getByRole("button", { name: "New chat", exact: true }).first().click();
    const html = `<meta http-equiv="Content-Security-Policy" content="default-src * 'unsafe-inline' 'unsafe-eval' data: blob:">
<style>@import '${canary}/style';</style><script src="${canary}/script"></script><img src="${canary}/image"><iframe src="${canary}/frame"></iframe><object data="${canary}/object"></object>
<form action="${canary}/form"><button id="submit">Submit</button></form><pre id="results"></pre><button id="navigate">Navigate</button><script>
const results={};const publish=()=>document.getElementById('results').textContent=JSON.stringify(results);const probe=(name,fn)=>{try{results[name]=fn()===false?'blocked':'exposed'}catch{results[name]='blocked'}};
probe('parent',()=>parent.parent.document.body);probe('storage',()=>localStorage.getItem('secret'));probe('ipc',()=>typeof window.__TAURI_INTERNALS__ !== 'undefined');probe('node',()=>typeof require !== 'undefined');probe('rtc',()=>typeof RTCPeerConnection !== 'undefined');probe('popup',()=>window.open('${canary}/popup')!==null);probe('top',()=>top.location.href='${canary}/top');
const nested=document.createElement('iframe');document.body.appendChild(nested);probe('nestedRtc',()=>typeof nested.contentWindow.RTCPeerConnection !== 'undefined');nested.remove();
try{const worker=new Worker(URL.createObjectURL(new Blob(['postMessage("escaped");fetch("${canary}/worker")'])));results.worker='pending';worker.onmessage=()=>{results.worker='exposed';publish()};worker.onerror=(event)=>{event.preventDefault();results.worker='blocked';publish()}}catch{results.worker='blocked'};
fetch('${canary}/fetch').catch(()=>{});fetch('file:///C:/Windows/win.ini').catch(()=>{});fetch('ipc://localhost/invoke').catch(()=>{});navigator.sendBeacon('${canary}/beacon','x');try{new WebSocket('${canary.replace("http:", "ws:")}/socket')}catch{};
publish();document.getElementById('navigate').onclick=()=>location.href='${canary}/navigate';
</script>`;
    await send(page, `\`\`\`zest-html Confinement\n${html}\n\`\`\``);
    const confinement = card(page, "Confinement");
    await confinement.getByRole("button", { name: "Open", exact: true }).click();
    const frame = documentFrame(confinement);
    await expect(frame.locator("#results")).toContainText('"rtc":"blocked"');
    await expect(frame.locator("#results")).toContainText('"worker":"blocked"');
    const results = JSON.parse(await frame.locator("#results").innerText());
    expect(results).toEqual({ parent: "blocked", storage: "blocked", ipc: "blocked", node: "blocked", rtc: "blocked", nestedRtc: "blocked", popup: "blocked", worker: "blocked", top: "blocked" });
    const wrapperUrl = await confinement.locator("iframe").getAttribute("src");
    const documentResponse = await page.request.get(`${wrapperUrl}/document`);
    expect(documentResponse.headers()["content-security-policy"]).toContain("sandbox allow-scripts");
    expect(documentResponse.headers()["content-security-policy"]).toContain("connect-src 'none'");
    expect((await page.request.get(wrapperUrl!)).headers()["content-security-policy"]).toContain(`frame-src ${wrapperUrl}/document`);
    await frame.locator("#submit").click();
    await frame.locator("#navigate").click();
    // Give attempted loads time to reach the independent canary, if confinement failed.
    await expect.poll(() => page.frames().some((child) => child.url().startsWith(canary))).toBe(false);
    await page.waitForTimeout(300);
    expect(hits).toEqual([]);
    expect(context.pages()).toHaveLength(1);
    await expect(page.locator("#zest-composer-input")).toBeVisible();
    expect(pageErrors).toEqual([]);
    const rejected = await page.request.post("/__zest_html", { headers: { Origin: "null" }, data: { title: "Child", html: "<p>no</p>" } });
    expect(rejected.status()).toBe(403);
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});
