// The long-lived half of zest-control: owns the dev server (when it started
// it), a Chromium page on the fixture UI, and a record of everything that went
// wrong. `zest-control.mjs` starts it detached and sends it one command per
// call over loopback HTTP, authenticated with a per-session token.

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { appendFileSync, mkdirSync, openSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

import { DEV_SERVER_URL, killTree, parseArgs } from "./zest-control.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const uiRoot = path.join(repoRoot, "crates", "desktop", "ui");
const IDLE_SHUTDOWN_MS = 30 * 60 * 1000;
const MAX_EVENTS = 2000;
const TOAST_PREFIX = "[zest-control:toast]";

const { options } = parseArgs(["__daemon", ...process.argv.slice(2)]);
const stateFile = options.state;
const logFile = options.log;

function log(message) {
  appendFileSync(logFile, `${new Date().toISOString()} ${message}\n`);
}

function writeState(state) {
  mkdirSync(path.dirname(stateFile), { recursive: true });
  writeFileSync(stateFile, JSON.stringify(state, null, 2));
}

// ---------------------------------------------------------------------------
// Event record

let seq = 0;
let events = [];
let currentCommand = "boot";

function record(kind, level, text, extra = {}) {
  seq += 1;
  events.push({ seq, at: new Date().toISOString(), kind, level, text: String(text).slice(0, 2000), during: currentCommand, ...extra });
  if (events.length > MAX_EVENTS) events = events.slice(-MAX_EVENTS);
}

const isError = (event) => event.level === "error";

// Reports each toast as it appears, so toasts that vanish between commands
// are still in the record.
const toastObserver = `(() => {
  const seen = new WeakSet();
  const report = (node) => {
    if (seen.has(node)) return;
    seen.add(node);
    const text = (node.innerText || "").trim();
    if (!text) return;
    console.debug(${JSON.stringify(TOAST_PREFIX)} + JSON.stringify({ text, type: node.getAttribute("data-type") || "" }));
  };
  const scan = () => document.querySelectorAll('[data-slot="toast"]').forEach(report);
  new MutationObserver(() => setTimeout(scan, 50)).observe(document, { childList: true, subtree: true });
})();`;

// ---------------------------------------------------------------------------
// Dev server and browser

let serverChild = null;
let browser = null;
let page = null;
let scenario = typeof options.scenario === "string" ? options.scenario : null;

async function serverStatus() {
  try {
    const response = await fetch(DEV_SERVER_URL, { signal: AbortSignal.timeout(2_000) });
    const body = await response.text();
    return body.includes('id="root"') ? "zest-ui" : "other";
  } catch {
    return "free";
  }
}

async function ensureServer() {
  const status = await serverStatus();
  if (status === "zest-ui") return false;
  if (status === "other") {
    throw Object.assign(new Error("port 1420 is in use by another program"), {
      hint: "Stop it; the UI dev server needs port 1420 (strictPort).",
    });
  }
  const out = openSync(logFile, "a");
  serverChild = spawn(process.execPath, [path.join(repoRoot, "node_modules", "vite", "bin", "vite.js")], {
    cwd: uiRoot,
    detached: process.platform !== "win32",
    stdio: ["ignore", out, out],
    windowsHide: true,
  });
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if ((await serverStatus()) === "zest-ui") return true;
    if (serverChild.exitCode !== null) break;
    await sleep(250);
  }
  throw Object.assign(new Error("the Vite dev server did not start"), { hint: `See ${logFile}.` });
}

async function launchBrowser() {
  browser = await chromium.launch({ headless: !options.headed });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.addInitScript(toastObserver);
  page = await context.newPage();
  page.on("console", (message) => {
    const text = message.text();
    if (text.startsWith(TOAST_PREFIX)) {
      const toast = JSON.parse(text.slice(TOAST_PREFIX.length));
      record("toast", toast.type === "error" ? "error" : "info", toast.text, { toastType: toast.type });
      return;
    }
    const level = message.type() === "error" ? "error" : message.type() === "warning" ? "warning" : "log";
    const where = message.location()?.url;
    record("console", level, text, where ? { source: where.replace(DEV_SERVER_URL, "") } : {});
  });
  page.on("pageerror", (error) => record("pageerror", "error", error.stack || error.message));
  page.on("crash", () => record("crash", "error", "the page crashed"));
  page.on("requestfailed", (request) => {
    const reason = request.failure()?.errorText ?? "failed";
    if (reason === "net::ERR_ABORTED") return; // navigations and cancelled fetches
    record("request", "error", `${request.method()} ${request.url()} ${reason}`);
  });
  page.on("response", (response) => {
    if (response.status() >= 400 && !response.url().endsWith("/favicon.ico")) {
      record("request", "error", `${response.status()} ${response.request().method()} ${response.url()}`);
    }
  });
}

// ---------------------------------------------------------------------------
// UI helpers

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const composer = () => page.locator("#zest-composer-input");
const stopButton = () => page.getByRole("button", { name: /^Stop( response)?$/ });

async function turnRunning() {
  return (await stopButton().count()) > 0;
}

function fixtureUrl(name) {
  return `${DEV_SERVER_URL}/?fixture=1${name ? `&scenario=${encodeURIComponent(name)}` : ""}`;
}

async function signature() {
  return page.evaluate(() =>
    [
      document.body.innerText.length,
      document.querySelectorAll('[data-slot="message"]').length,
      document.querySelectorAll('[data-slot="code-block-token"][style]').length,
      document.querySelectorAll('[role="dialog"]').length,
    ].join(":"),
  );
}

/** No turn running, and two samples 300 ms apart look the same. */
async function settle(timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let previous = null;
  while (Date.now() < deadline) {
    if (await turnRunning()) {
      previous = null;
    } else {
      const current = await signature();
      if (current === previous) return { settled: true };
      previous = current;
    }
    await sleep(300);
  }
  const running = await turnRunning();
  return {
    settled: false,
    turnRunning: running,
    hint: running
      ? "A turn is still running (the split-streaming scenario keeps sends live). Use stop-turn, or wait longer with --timeout."
      : "The page kept changing; take a snapshot to see why.",
  };
}

async function openFixture(name) {
  scenario = name ?? null;
  await page.goto(fixtureUrl(scenario), { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => (document.getElementById("root")?.childElementCount ?? 0) > 0, null, { timeout: 30_000 });
  // The seeded chat streams a canned turn on load; sends before it ends are queued.
  return settle(20_000);
}

async function messages() {
  return page.evaluate(() =>
    [...document.querySelectorAll('[data-slot="message"]')].map((node) => ({
      role: node.getAttribute("data-align") === "end" ? "user" : "assistant",
      text: node.innerText.trim().replace(/\s+\n/g, "\n").slice(0, 400),
    })),
  );
}

async function queuedMessages() {
  // Composer.tsx: an aria-live panel whose header reads "Queued messages" and
  // whose last child holds one element per queued message.
  return page.evaluate(() => {
    const heading = [...document.querySelectorAll("span")].find(
      (node) => node.textContent?.trim().toLowerCase() === "queued messages",
    );
    const panel = heading?.closest('[aria-live="polite"]');
    return [...(panel?.lastElementChild?.children ?? [])]
      .map((node) => node.innerText.trim().replace(/\s+/g, " ").replace(/^\d+\.\s*/, ""))
      .filter(Boolean)
      .slice(0, 20);
  });
}

async function hasQueuedPanel() {
  return (await page.getByText("Queued messages", { exact: true }).count()) > 0;
}

async function codeBlocks() {
  return page.evaluate(() =>
    [...document.querySelectorAll('[data-slot="code-block"]')].map((block) => {
      const label = block.querySelector('[data-slot="code-block-language"]');
      const lines = [...block.querySelectorAll('[data-slot="code-block-line-content"]')];
      const code = block.querySelector('[data-slot="code-block-code"]')?.textContent ?? "";
      const colored = block.querySelectorAll('[data-slot="code-block-token"][style]').length;
      return {
        label: label?.textContent?.trim().toLowerCase() ?? null,
        grammar: label ? !label.hasAttribute("data-unsupported") : null,
        lines: lines.length || (code ? code.split("\n").length : 0),
        coloredTokens: colored,
        firstLine: (lines[0]?.textContent ?? code.split("\n")[0]).slice(0, 80),
      };
    }),
  );
}

async function meter() {
  const reading = page.getByTitle("Context usage", { exact: true });
  if ((await reading.count()) > 0) return { shown: true, text: (await reading.first().innerText()).trim() };
  const blank = (await page.getByText("Context —", { exact: true }).count()) > 0;
  return { shown: false, text: blank ? "Context —" : null };
}

async function toasts() {
  return page.evaluate(() =>
    [...document.querySelectorAll('[data-slot="toast"]')].map((node) => ({
      type: node.getAttribute("data-type") || null,
      text: node.innerText.trim(),
    })),
  );
}

async function uiState() {
  const all = await messages();
  const dialogs = await page.evaluate(() =>
    [...document.querySelectorAll('[role="dialog"]')].map(
      (node) => node.getAttribute("aria-label") || node.querySelector("h1,h2,h3")?.textContent?.trim() || "dialog",
    ),
  );
  return {
    url: page.url().replace(DEV_SERVER_URL, ""),
    scenario,
    turnRunning: await turnRunning(),
    composer: (await composer().count()) > 0,
    messageCount: all.length,
    lastMessage: all.at(-1) ?? null,
    queued: (await hasQueuedPanel()) ? await queuedMessages() : [],
    meter: await meter(),
    dialogs,
    toasts: await toasts(),
    errorCount: events.filter(isError).length,
    lastEventSeq: seq,
  };
}

function modKey(keys) {
  return keys.replace(/\bMod\b/g, process.platform === "darwin" ? "Meta" : "Control");
}

async function roleNames(role) {
  const yaml = await page.locator("body").ariaSnapshot();
  const names = [...yaml.matchAll(new RegExp(`- ${role} "([^"]+)"`, "g"))].map((match) => match[1]);
  return [...new Set(names)].slice(0, 25);
}

const fail = (message, hint) => Object.assign(new Error(message), { hint });

// ---------------------------------------------------------------------------
// Commands

async function send(text, opts = {}) {
  if (typeof text !== "string" || !text) throw fail("send needs the message text", 'send "hello"');
  if ((await composer().count()) === 0) {
    throw fail("no composer on screen", "A picker, panel, or dialog is open. Take a snapshot, or run open to reload.");
  }
  const users = async () => (await messages()).filter((message) => message.role === "user").length;
  const beforeUsers = await users();
  const beforeQueued = (await queuedMessages()).length;
  await composer().fill(text);
  await composer().press("Enter");

  const deadline = Date.now() + 5_000;
  let outcome = "none";
  while (Date.now() < deadline) {
    if ((await users()) > beforeUsers) {
      outcome = "sent";
      break;
    }
    if ((await queuedMessages()).length > beforeQueued) {
      outcome = "queued";
      break;
    }
    await sleep(100);
  }
  if (outcome === "queued" && !opts.allowQueue) {
    throw fail(
      "the message was queued, not sent: this chat already has a turn running",
      "Run wait-settle or stop-turn first, or new-chat. Pass --allow-queue to queue on purpose.",
    );
  }
  if (outcome === "none") throw fail("the message did not appear in the transcript", "Take a snapshot and check errors.");
  const settled = opts.noWait || outcome === "queued" ? { settled: false, skipped: true } : await settle(30_000);
  const reply = (await messages()).filter((message) => message.role === "assistant").at(-1) ?? null;
  return { outcome, ...settled, reply: outcome === "sent" ? reply?.text ?? null : null };
}

async function newChat() {
  await page.getByRole("button", { name: "New chat", exact: true }).first().click();
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline && (await messages()).length > 0) await sleep(100);
  const count = (await messages()).length;
  if (count > 0) throw fail("the new chat did not open empty", "Take a snapshot; a turn may still be streaming.");
  return settle(5_000);
}

async function stopTurn() {
  if (!(await turnRunning())) return { stopped: false, detail: "no turn was running" };
  await stopButton().first().click();
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline && (await turnRunning())) await sleep(100);
  if (await turnRunning()) throw fail("the turn is still running after Stop", "Check errors; cancelTurn may have failed.");
  return { stopped: true, ...(await settle(5_000)) };
}

async function click(name, opts) {
  if (!name) throw fail("click needs an accessible name", 'click "New chat" --role button');
  const role = opts.role ?? "button";
  const locator = page.getByRole(role, { name, exact: Boolean(opts.exact) });
  const count = await locator.count();
  if (count === 0) {
    throw fail(`no ${role} named "${name}"`, `Visible ${role}s: ${(await roleNames(role)).join(" | ") || "none"}`);
  }
  const index = opts.nth === undefined ? 0 : Number(opts.nth);
  await locator.nth(index).click();
  return {
    clicked: `${role} "${name}"`,
    matches: count,
    ...(count > 1 && opts.nth === undefined ? { note: "several matched; clicked the first (use --exact or --nth)" } : {}),
    ...(await settle(3_000)),
  };
}

async function inspect(probe = "state") {
  switch (probe) {
    case "state":
      return { state: await uiState() };
    case "messages":
      return { messages: await messages() };
    case "code-blocks":
      return { codeBlocks: await codeBlocks() };
    case "meter":
      return { meter: await meter() };
    case "composer":
      return {
        composer: {
          present: (await composer().count()) > 0,
          value: (await composer().count()) > 0 ? await composer().inputValue() : null,
          turnRunning: await turnRunning(),
          queued: (await hasQueuedPanel()) ? await queuedMessages() : [],
        },
      };
    case "toasts":
      return { toasts: await toasts() };
    case "backend-calls":
      // Counted by the fixture backend per page load (recordFixtureCalls).
      return { backendCalls: await page.evaluate(() => globalThis.__zestFixtureCalls ?? {}) };
    default:
      throw fail(`unknown probe "${probe}"`, "Probes: state, messages, code-blocks, meter, composer, toasts, backend-calls.");
  }
}

function eventsSince(opts, filter) {
  const since = opts.since === undefined ? 0 : Number(opts.since);
  const selected = events.filter((event) => event.seq > since && filter(event));
  if (opts.clear) events = [];
  return { events: selected, lastSeq: seq };
}

/** Core flows, each checked for what a user would see. */
async function check() {
  const steps = [];
  const startSeq = seq;
  const step = async (name, run) => {
    const started = Date.now();
    try {
      const detail = await run();
      steps.push({ name, ok: true, ms: Date.now() - started, ...(detail ? { detail } : {}) });
      return true;
    } catch (error) {
      // Evidence for the failure; CI points ZEST_CONTROL_ARTIFACTS at a directory it uploads.
      const dir = process.env.ZEST_CONTROL_ARTIFACTS || path.join(os.tmpdir(), "zest-control");
      const screenshot = path.resolve(dir, `check-${name}-${Date.now()}.png`);
      try {
        mkdirSync(dir, { recursive: true });
        await page.screenshot({ path: screenshot });
      } catch {
        // A crashed page cannot be captured; the error still reports.
      }
      steps.push({
        name,
        ok: false,
        ms: Date.now() - started,
        error: error.message.split("\n")[0],
        ...(error.hint ? { hint: error.hint } : {}),
        screenshot,
      });
      return false;
    }
  };

  const booted = await step("boot", async () => {
    const result = await openFixture(null);
    if (!result.settled) throw fail("the fixture did not settle after loading", result.hint);
    if ((await composer().count()) === 0) throw fail("no composer after boot");
    return `${(await messages()).length} messages in the seeded chat`;
  });
  if (booted) {
    await step("send", async () => {
      await newChat();
      const result = await send("zest-control check");
      if (!result.reply?.includes("Fixture echo: zest-control check")) {
        throw fail(`unexpected reply: ${JSON.stringify(result.reply)}`);
      }
      return "echo received";
    });
    await step("highlight", async () => {
      await send("highlight\n\n```ruby\ndef greet(name)\n  puts name\nend\n```");
      const block = (await codeBlocks()).at(-1);
      if (!block || block.label !== "ruby" || block.coloredTokens === 0) {
        throw fail(`ruby block not highlighted: ${JSON.stringify(block)}`, "See docs/features/message-rendering.md.");
      }
      return `${block.coloredTokens} colored tokens`;
    });
    await step("meter", async () => {
      const reading = await meter();
      if (!reading.shown) throw fail(`the context meter shows ${JSON.stringify(reading.text)}`);
      return reading.text;
    });
    await step("palette", async () => {
      await page.keyboard.press(modKey("Mod+K"));
      const palette = page.getByRole("dialog", { name: "Search", exact: true });
      await palette.waitFor({ state: "visible", timeout: 5_000 });
      await page.keyboard.press("Escape");
      await palette.waitFor({ state: "hidden", timeout: 5_000 });
      return "opened with Mod+K, closed with Escape";
    });
    await step("stop-turn", async () => {
      await openFixture("split-streaming");
      await send("keep running", { noWait: true });
      if (!(await turnRunning())) throw fail("the turn did not stay running");
      await sleep(500);
      const during = await meter();
      if (!during.shown) throw fail("the context meter went blank during the turn", "See docs/features/context-budget.md.");
      const stopped = await stopTurn();
      if (!stopped.stopped) throw fail("nothing was stopped");
      return "stopped; meter kept its reading";
    });
    await openFixture(null);
  }
  const errors = events.filter((event) => event.seq > startSeq && isError(event));
  steps.push({
    name: "no errors",
    ok: errors.length === 0,
    ...(errors.length ? { error: `${errors.length} error(s) during the check`, hint: "They are listed under errors." } : {}),
  });
  return { passed: steps.every((entry) => entry.ok), steps, ...(errors.length ? { errors } : {}) };
}

async function run(command, args, opts) {
  switch (command) {
    case "ping":
      return {};
    case "status":
      return { session: { pid: process.pid, scenario, url: page.url(), ownsServer: serverChild !== null }, state: await uiState() };
    case "open":
      return openFixture(typeof opts.scenario === "string" && opts.scenario !== "none" ? opts.scenario : null);
    case "scenario":
      return openFixture(args[0] && args[0] !== "none" ? args[0] : null);
    case "pref": {
      const [key, value] = args;
      if (!key || (value === undefined && !opts.delete)) throw fail("pref needs KEY VALUE or KEY --delete");
      await page.evaluate(([k, v]) => (v === null ? localStorage.removeItem(k) : localStorage.setItem(k, v)), [key, opts.delete ? null : value]);
      return { ...(await openFixture(scenario)), note: "reloaded; fixture state starts fresh" };
    }
    case "new-chat":
      return newChat();
    case "send":
      return send(args.join(" "), opts);
    case "stop-turn":
      return stopTurn();
    case "wait-settle": {
      const result = await settle(opts.timeout ? Number(opts.timeout) : 15_000);
      if (!result.settled) throw fail(result.turnRunning ? "a turn is still running" : "the page did not settle", result.hint);
      return result;
    }
    case "press":
      if (!args[0]) throw fail("press needs keys", "press Mod+K");
      await page.keyboard.press(modKey(args[0]));
      return settle(3_000);
    case "click":
      return click(args.join(" "), opts);
    case "type":
      await page.keyboard.type(args.join(" "));
      return {};
    case "snapshot": {
      const target = opts.selector ? page.locator(opts.selector).first() : page.locator("body");
      return { yaml: await target.ariaSnapshot() };
    }
    case "screenshot": {
      const file = path.resolve(args[0] ?? path.join(os.tmpdir(), "zest-control", `shot-${Date.now()}.png`));
      mkdirSync(path.dirname(file), { recursive: true });
      if (opts.selector) await page.locator(opts.selector).first().screenshot({ path: file });
      else await page.screenshot({ path: file, fullPage: Boolean(opts.full) });
      return { path: file };
    }
    case "inspect":
      return inspect(args[0]);
    case "errors":
      return eventsSince(opts, isError);
    case "console":
      return eventsSince(opts, (event) => event.kind === "console" && (!opts.level || event.level === opts.level));
    case "eval": {
      if (!args.length) throw fail("eval needs an expression", 'eval "document.title"');
      return { result: await page.evaluate(args.join(" ")) };
    }
    case "check":
      return check();
    default:
      throw fail(`unknown command "${command}"`);
  }
}

// ---------------------------------------------------------------------------
// Control server

const token = randomBytes(24).toString("hex");
let queue = Promise.resolve();
let idleTimer = null;

function resetIdle() {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    log("idle for 30 minutes; shutting down");
    void shutdown(0);
  }, IDLE_SHUTDOWN_MS);
}

async function handle(command, args, opts) {
  const startSeq = seq;
  currentCommand = command;
  try {
    const result = await run(command, args, opts);
    const newErrors = events.filter((event) => event.seq > startSeq && isError(event));
    const ok = command === "check" ? result.passed : true;
    return { ok, command, ...result, ...(newErrors.length && command !== "errors" && command !== "check" ? { newErrors } : {}) };
  } catch (error) {
    const newErrors = events.filter((event) => event.seq > startSeq && isError(event));
    return {
      ok: false,
      command,
      error: error.message.split("\n")[0],
      ...(error.hint ? { hint: error.hint } : {}),
      ...(newErrors.length ? { newErrors } : {}),
    };
  } finally {
    currentCommand = "idle";
  }
}

let shuttingDown = false;
async function shutdown(code) {
  if (shuttingDown) return;
  shuttingDown = true;
  clearTimeout(idleTimer);
  try {
    await browser?.close();
  } catch {
    // Closing a crashed browser can throw.
  }
  if (serverChild) killTree(serverChild.pid);
  rmSync(stateFile, { force: true });
  log(`stopped (exit ${code})`);
  process.exit(code);
}

async function main() {
  mkdirSync(path.dirname(logFile), { recursive: true });
  log(`starting (pid ${process.pid})`);
  try {
    const ownsServer = await ensureServer();
    await launchBrowser();
    const boot = await openFixture(scenario);
    const server = http.createServer((request, response) => {
      let body = "";
      request.on("data", (chunk) => (body += chunk));
      request.on("end", () => {
        const reply = (status, value) => {
          response.writeHead(status, { "content-type": "application/json" });
          response.end(JSON.stringify(value));
        };
        if (request.headers["x-zest-control-token"] !== token) return reply(403, { ok: false, error: "bad token" });
        let message;
        try {
          message = JSON.parse(body || "{}");
        } catch {
          return reply(400, { ok: false, error: "bad JSON" });
        }
        resetIdle();
        if (message.command === "stop") {
          reply(200, { ok: true, command: "stop", stopped: true });
          void shutdown(0);
          return;
        }
        // One command at a time, in arrival order.
        queue = queue.then(async () => reply(200, await handle(message.command, message.args ?? [], message.options ?? {})));
      });
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const port = server.address().port;
    writeState({
      status: "ready",
      pid: process.pid,
      port,
      token,
      url: fixtureUrl(scenario),
      scenario,
      ownsServer,
      headed: Boolean(options.headed),
      bootSettled: boot.settled,
      log: logFile,
      startedAt: new Date().toISOString(),
    });
    currentCommand = "idle";
    resetIdle();
    log(`ready on 127.0.0.1:${port}`);
  } catch (error) {
    log(`failed: ${error.stack || error.message}`);
    writeState({ status: "failed", error: error.message, hint: error.hint, log: logFile });
    try {
      await browser?.close();
    } catch {
      // Nothing to clean up.
    }
    if (serverChild) killTree(serverChild.pid);
    process.exit(1);
  }
}

process.on("SIGTERM", () => void shutdown(0));
process.on("SIGINT", () => void shutdown(0));
await main();
