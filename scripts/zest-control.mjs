// Drive and inspect the Zest UI from the command line.
//
// One live session (Vite dev server + Chromium on the offline fixture UI) is
// started once and reused by every later command, so an agent can act, look,
// and debug in single commands instead of writing throwaway Playwright
// scripts. The session records console errors, page errors, failed requests,
// and toasts the whole time; every command reports the errors it caused.
//
//   node ./scripts/zest-control.mjs doctor                    what is installed and running
//   node ./scripts/zest-control.mjs check                     smoke-test the core flows
//   node ./scripts/zest-control.mjs start [--scenario NAME] [--headed]
//   node ./scripts/zest-control.mjs send "hello"              waits for the turn to settle
//   node ./scripts/zest-control.mjs inspect state             what the UI is doing right now
//   node ./scripts/zest-control.mjs errors                    everything that went wrong
//   node ./scripts/zest-control.mjs stop
//
// Run `node ./scripts/zest-control.mjs help` for every command. Output is JSON
// (snapshot prints YAML); the exit code is 0 on success, 1 when the command
// failed, 2 for bad usage, and 3 when no session is running.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const DEV_SERVER_URL = "http://127.0.0.1:1420";
const DAEMON_SCRIPT = path.join(repoRoot, "scripts", "zest-control-daemon.mjs");
const FIXTURE_BACKEND = path.join(repoRoot, "crates", "desktop", "ui", "src", "lib", "fixtureBackend.ts");

export const EXIT = { ok: 0, failed: 1, usage: 2, noSession: 3 };

export const COMMANDS = {
  doctor: "Report what is installed and running, with a fix for each problem",
  check: "Smoke-test boot, send, highlighting, context meter, palette, and stopping a turn; starts a session if needed",
  start: "Start a session: [--scenario NAME] [--headed]; reuses one that is already running",
  stop: "Close the browser, stop the dev server if the session started it",
  status: "Session details and a summary of the UI state",
  open: "Reload the fixture fresh: [--scenario NAME]",
  scenario: "Reload with a fixture scenario, or `none`",
  pref: "Set a localStorage preference and reload: KEY VALUE, or KEY --delete",
  "new-chat": "Open an empty chat",
  send: "Send a message and wait for the turn: TEXT [--no-wait] [--allow-queue]",
  "stop-turn": "Press Stop on the running turn",
  "wait-settle": "Wait until no turn is running and the transcript stops changing: [--timeout MS]",
  press: "Press keys, e.g. Mod+K (Control on Windows/Linux, Meta on macOS)",
  click: "Click by accessible name: NAME [--role button] [--exact] [--nth N]",
  type: "Type text into the focused element",
  snapshot: "Accessibility tree as YAML: [--selector CSS] [--json]",
  screenshot: "Save a PNG: [PATH] [--full] [--selector CSS]",
  inspect: "Structured probe: state | messages | code-blocks | meter | composer | toasts",
  errors: "Console errors, page errors, failed requests, and error toasts: [--since SEQ] [--clear]",
  console: "All captured console output: [--level error|warning|log] [--since SEQ] [--clear]",
  eval: "Evaluate a JavaScript expression in the page and return the result",
};

/** Parse argv into a command, positional args, and --options. */
export function parseArgs(argv) {
  const [command = "help", ...rest] = argv;
  const args = [];
  const options = {};
  for (let index = 0; index < rest.length; index += 1) {
    const token = rest[index];
    if (token === "--") {
      args.push(...rest.slice(index + 1));
      break;
    }
    const flag = /^--([a-z][a-z-]*)(?:=(.*))?$/.exec(token);
    if (!flag) {
      args.push(token);
      continue;
    }
    const [, name, inline] = flag;
    const key = name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    if (inline !== undefined) {
      options[key] = inline;
    } else if (VALUE_FLAGS.has(name) && index + 1 < rest.length) {
      options[key] = rest[(index += 1)];
    } else {
      options[key] = true;
    }
  }
  return { command, args, options };
}

const VALUE_FLAGS = new Set(["scenario", "timeout", "role", "nth", "selector", "since", "level"]);

/** One session per checkout, kept outside the repo. */
export function statePath(root = repoRoot) {
  const id = createHash("sha256").update(path.resolve(root).toLowerCase()).digest("hex").slice(0, 12);
  return path.join(os.tmpdir(), "zest-control", `${id}.json`);
}

/** Fixture scenarios, read from the fixture backend so the list never drifts. */
export function fixtureScenarios(source = safeRead(FIXTURE_BACKEND)) {
  const union = /type FixtureScenario\s*=([^;]+);/.exec(source ?? "");
  if (!union) return null;
  return [...union[1].matchAll(/"([a-z-]+)"/g)].map((match) => match[1]);
}

export function validateScenario(name, scenarios = fixtureScenarios()) {
  if (name === undefined || name === true || name === "none") return null;
  if (scenarios && !scenarios.includes(name)) {
    return `unknown scenario "${name}". Known: ${scenarios.join(", ")}`;
  }
  return null;
}

function safeRead(file) {
  try {
    return readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

function readState() {
  const text = safeRead(statePath());
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

async function callDaemon(state, command, args, options, timeoutMs = 120_000) {
  const response = await fetch(`http://127.0.0.1:${state.port}/`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-zest-control-token": state.token },
    body: JSON.stringify({ command, args, options }),
    signal: AbortSignal.timeout(timeoutMs),
  });
  return response.json();
}

async function ping(state) {
  if (!state) return false;
  try {
    const reply = await callDaemon(state, "ping", [], {}, 3_000);
    return reply.ok === true;
  } catch {
    return false;
  }
}

async function liveState() {
  const state = readState();
  return (await ping(state)) ? state : null;
}

function print(result) {
  if (typeof result === "string") {
    process.stdout.write(result.endsWith("\n") ? result : `${result}\n`);
  } else {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  }
}

async function startSession(options) {
  const existing = await liveState();
  if (existing) {
    return { ok: true, reused: true, ...existing, token: undefined };
  }
  const file = statePath();
  mkdirSync(path.dirname(file), { recursive: true });
  rmSync(file, { force: true });
  const logFile = file.replace(/\.json$/, ".log");
  const daemonArgs = [DAEMON_SCRIPT, `--state=${file}`, `--log=${logFile}`];
  if (typeof options.scenario === "string" && options.scenario !== "none") {
    daemonArgs.push(`--scenario=${options.scenario}`);
  }
  if (options.headed) daemonArgs.push("--headed");
  const child = spawn(process.execPath, daemonArgs, {
    cwd: repoRoot,
    detached: true,
    stdio: "ignore",
    windowsHide: true,
  });
  child.unref();

  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const state = readState();
    if (state?.status === "failed") {
      return { ok: false, error: state.error, hint: state.hint, log: logFile };
    }
    if (state?.status === "ready" && (await ping(state))) {
      return { ok: true, reused: false, ...state, token: undefined };
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return { ok: false, error: "session did not become ready within 90 s", log: logFile };
}

async function doctor() {
  const checks = [];
  const add = (name, ok, detail, hint) => checks.push({ name, ok, detail, ...(ok ? {} : { hint }) });

  const major = Number(process.versions.node.split(".")[0]);
  add("node", major >= 22, process.versions.node, "Use the Node version in .nvmrc (24.16.0).");

  let chromiumPath = null;
  try {
    const { chromium } = await import("@playwright/test");
    chromiumPath = chromium.executablePath();
    add("playwright", true, "installed");
  } catch (error) {
    add("playwright", false, error.message, "Run `npm ci` at the repository root.");
  }
  if (chromiumPath !== null) {
    add("chromium", existsSync(chromiumPath), chromiumPath, "Run `npx playwright install chromium` once.");
  }

  const vite = path.join(repoRoot, "node_modules", "vite", "bin", "vite.js");
  add("vite", existsSync(vite), vite, "Run `npm ci` at the repository root.");

  let server = "free";
  try {
    const response = await fetch(DEV_SERVER_URL, { signal: AbortSignal.timeout(2_000) });
    const body = await response.text();
    server = body.includes('id="root"') ? "zest-ui" : "other";
  } catch {
    server = "free";
  }
  add(
    "port 1420",
    server !== "other",
    server === "free" ? "free; the session will start Vite" : server === "zest-ui" ? "serving the Zest UI" : "in use by another program",
    "Stop whatever is using port 1420; the UI dev server needs it (strictPort).",
  );

  const state = readState();
  const alive = await ping(state);
  if (state && !alive) {
    add("session", false, "a stale session file exists", `Run \`node scripts/zest-control.mjs stop\` or delete ${statePath()}.`);
  } else {
    add("session", true, alive ? `running (pid ${state.pid}, ${state.url})` : "not running");
  }

  let ui = null;
  if (alive) {
    ui = await callDaemon(state, "inspect", ["state"], {});
    const errorCount = ui.state?.errorCount ?? 0;
    add("ui errors", errorCount === 0, `${errorCount} captured`, "Run `node scripts/zest-control.mjs errors` to see them.");
  }
  return { ok: checks.every((check) => check.ok), checks, ...(ui?.state ? { ui: ui.state } : {}) };
}

function help() {
  const width = Math.max(...Object.keys(COMMANDS).map((name) => name.length));
  const lines = Object.entries(COMMANDS).map(([name, text]) => `  ${name.padEnd(width)}  ${text}`);
  const scenarios = fixtureScenarios();
  return [
    "zest-control: drive and inspect the Zest UI (offline fixture backend).",
    "",
    "Usage: node scripts/zest-control.mjs <command> [args] [--options]",
    "",
    ...lines,
    "",
    scenarios ? `Scenarios: ${scenarios.join(", ")}` : "",
    "Typical loop: start -> send/click/press -> inspect/snapshot/errors -> stop.",
  ].join("\n");
}

async function main() {
  const { command, args, options } = parseArgs(process.argv.slice(2));

  if (command === "help" || options.help) {
    print(help());
    return EXIT.ok;
  }
  if (!(command in COMMANDS)) {
    print({ ok: false, error: `unknown command "${command}"`, hint: "Run `node scripts/zest-control.mjs help`." });
    return EXIT.usage;
  }
  const scenario = command === "scenario" ? args[0] : options.scenario;
  const scenarioError = validateScenario(scenario);
  if (scenarioError) {
    print({ ok: false, error: scenarioError });
    return EXIT.usage;
  }

  if (command === "doctor") {
    const result = await doctor();
    print(result);
    return result.ok ? EXIT.ok : EXIT.failed;
  }
  if (command === "start") {
    const result = await startSession(options);
    print(result);
    return result.ok ? EXIT.ok : EXIT.failed;
  }

  let state = await liveState();
  let ephemeral = false;
  if (!state && command === "check") {
    const started = await startSession({});
    if (!started.ok) {
      print(started);
      return EXIT.failed;
    }
    state = readState();
    ephemeral = true;
  }
  if (!state) {
    if (command === "stop") {
      rmSync(statePath(), { force: true });
      print({ ok: true, stopped: false, detail: "no session was running" });
      return EXIT.ok;
    }
    print({ ok: false, error: "no session is running", hint: "Run `node scripts/zest-control.mjs start` first." });
    return EXIT.noSession;
  }

  let result;
  try {
    result = await callDaemon(state, command, args, options);
  } catch (error) {
    result = { ok: false, error: `session did not answer: ${error.message}`, hint: `See the session log: ${state.log}` };
  }
  if (ephemeral) await callDaemon(state, "stop", [], {}).catch(() => {});

  if (command === "snapshot" && result.ok && !options.json) {
    print(result.yaml);
  } else {
    print(result);
  }
  return result.ok ? EXIT.ok : EXIT.failed;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      print({ ok: false, error: error.message });
      process.exitCode = EXIT.failed;
    },
  );
}

// Exported for the daemon, which kills the dev server tree it started.
export function killTree(pid) {
  if (!pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  } else {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      // Already gone.
    }
  }
}
