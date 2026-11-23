#!/usr/bin/env node
// Drive the Zest desktop app from the command line and get JSON back. It launches the release
// app with its output in a log, then reads and drives the window through Windows UI Automation
// (scripts/gpui-control.ps1), never with simulated keystrokes.
//
//   node scripts/gpui-control.mjs start [--project DIR] [--build]   (default: offline --demo)
//   node scripts/gpui-control.mjs send "Demo: write"
//   node scripts/gpui-control.mjs click "Allow once"
//   node scripts/gpui-control.mjs inspect | messages | snapshot | errors | screenshot out.png
//   node scripts/gpui-control.mjs stop
import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const binary = path.join(repo, "crates/desktop/target/release", process.platform === "win32" ? "zest-desktop.exe" : "zest-desktop");
const driver = path.join(repo, "scripts/gpui-control.ps1");
const stateDir = path.join(os.tmpdir(), "zest-gpui-control");
const stateFile = path.join(stateDir, "state.json");

const USAGE = `Usage: node scripts/gpui-control.mjs <command>
  doctor                      what is installed and running
  start [--project DIR] [--build]
                              launch the app (offline demo unless --project)
  status | stop
  inspect                     turn state, messages, composer, tabs
  messages                    transcript rows as screen readers hear them
  snapshot                    every named control with its role and state
  click NAME [--nth N]        press a button, tab, switch or radio by accessible name
  type TEXT [--into NAME]     put TEXT in the message box, or in the field named NAME
  send TEXT [--timeout S]     type and send, then wait for the reply or a card
  wait-settle [--timeout S]   wait until no turn is running
  screenshot FILE             capture the window
  errors [--clear]            panics and errors from the app's output since start (or last --clear)`;

function reply(value, code = 0) {
  console.log(JSON.stringify(value, null, 2));
  process.exit(code);
}

function fail(message, code = 1) {
  reply({ ok: false, error: message }, code);
}

function readState() {
  try {
    return JSON.parse(readFileSync(stateFile, "utf8"));
  } catch {
    return null;
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function session() {
  const state = readState();
  if (!state || !alive(state.pid)) fail("No running session. Start one with: node scripts/gpui-control.mjs start", 3);
  return state;
}

function option(args, name) {
  const at = args.indexOf(name);
  if (at < 0) return undefined;
  const value = args[at + 1];
  args.splice(at, 2);
  return value;
}

function flag(args, name) {
  const at = args.indexOf(name);
  if (at < 0) return false;
  args.splice(at, 1);
  return true;
}

function drive(state, command, extra = {}) {
  if (process.platform !== "win32") {
    fail("gpui-control drives the app through Windows UI Automation. On Linux, use the GPUI tests: npm run gpui:test and cargo test --manifest-path crates/desktop/Cargo.toml.");
  }
  const params = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", driver, "-Command", command, "-AppPid", String(state.pid)];
  for (const [key, value] of Object.entries(extra)) if (value !== undefined) params.push(`-${key}`, String(value));
  const result = spawnSync("powershell.exe", params, { encoding: "utf8", windowsHide: true });
  const text = (result.stdout || "").trim().split(/\r?\n/).filter(Boolean).pop() ?? "";
  try {
    const parsed = JSON.parse(text);
    return { ...parsed, newErrors: newErrors(state).slice(-5) };
  } catch {
    const message = (result.stderr || result.stdout || "no output").trim().split(/\r?\n/).slice(0, 4).join(" ");
    fail(`UI Automation failed: ${message}`);
  }
}

const ERROR_LINE = /panicked|\berror\b|\bfailed\b|exception/i;

function newErrors(state) {
  try {
    const log = readFileSync(state.log, "utf8").slice(state.errorsOffset ?? 0);
    return log.split(/\r?\n/).filter((line) => ERROR_LINE.test(line));
  } catch {
    return [];
  }
}

const args = process.argv.slice(2);
const command = args.shift();
switch (command) {
  case undefined:
  case "help":
  case "--help":
    console.log(USAGE);
    process.exit(command ? 0 : 2);
    break;
  case "doctor": {
    const state = readState();
    reply({
      ok: true,
      platform: process.platform,
      automation: process.platform === "win32" ? "Windows UI Automation" : "unavailable (Windows only)",
      binary: existsSync(binary) ? binary : null,
      fix: existsSync(binary) ? undefined : "Build it: cargo build --locked --release --manifest-path crates/desktop/Cargo.toml (or start --build)",
      session: state && alive(state.pid) ? state : null,
    });
    break;
  }
  case "start": {
    const existing = readState();
    if (existing && alive(existing.pid)) reply({ ok: true, reused: true, ...existing });
    const project = option(args, "--project");
    if (flag(args, "--build") || !existsSync(binary)) {
      const build = spawnSync("cargo", ["build", "--locked", "--release", "--manifest-path", path.join(repo, "crates/desktop/Cargo.toml")], { stdio: "inherit", cwd: repo });
      if (build.status !== 0) fail("cargo build failed");
    }
    mkdirSync(stateDir, { recursive: true });
    const log = path.join(stateDir, `zest-${Date.now()}.log`);
    const fd = openSync(log, "a");
    // --no-focus keeps the window from taking focus away from whatever the user is doing.
    const appArgs = [...(project ? ["--project", path.resolve(project)] : ["--demo"]), "--no-focus"];
    const child = spawn(binary, appArgs, { cwd: repo, detached: true, stdio: ["ignore", fd, fd], windowsHide: false });
    child.unref();
    closeSync(fd);
    const state = { pid: child.pid, log, args: appArgs, startedAt: new Date().toISOString(), errorsOffset: 0 };
    writeFileSync(stateFile, JSON.stringify(state, null, 2));
    // Wait for the window, then report its first state.
    const ready = drive(state, "inspect");
    reply({ ...ready, session: state });
    break;
  }
  case "status": {
    const state = readState();
    reply({ ok: true, running: Boolean(state && alive(state.pid)), session: state });
    break;
  }
  case "stop": {
    const state = readState();
    if (state && alive(state.pid)) {
      if (process.platform === "win32") spawnSync("taskkill", ["/PID", String(state.pid), "/T", "/F"], { windowsHide: true });
      else process.kill(state.pid);
    }
    rmSync(stateFile, { force: true });
    reply({ ok: true, stopped: Boolean(state) });
    break;
  }
  case "inspect":
  case "messages":
  case "snapshot":
  case "wait-settle": {
    const state = session();
    const result = drive(state, command, { TimeoutSeconds: option(args, "--timeout") });
    reply(result, result.ok ? 0 : 1);
    break;
  }
  case "click": {
    const state = session();
    const nth = option(args, "--nth");
    if (!args[0]) fail("click needs an accessible name", 2);
    const result = drive(state, "click", { Name: args[0], Nth: nth });
    reply(result, result.ok ? 0 : 1);
    break;
  }
  case "type":
  case "send": {
    const state = session();
    const timeout = option(args, "--timeout");
    const into = command === "type" ? option(args, "--into") : undefined;
    if (args[0] === undefined) fail(`${command} needs text`, 2);
    const result = drive(state, command, { Text: args[0], TimeoutSeconds: timeout, Name: into });
    reply(result, result.ok ? 0 : 1);
    break;
  }
  case "screenshot": {
    const state = session();
    if (!args[0]) fail("screenshot needs a file path", 2);
    const result = drive(state, "screenshot", { Out: path.resolve(args[0]) });
    reply(result, result.ok ? 0 : 1);
    break;
  }
  case "errors": {
    const state = session();
    const errors = newErrors(state);
    if (flag(args, "--clear")) {
      state.errorsOffset = statSync(state.log).size;
      writeFileSync(stateFile, JSON.stringify(state, null, 2));
    }
    reply({ ok: true, count: errors.length, errors, log: state.log });
    break;
  }
  default:
    console.error(`Unknown command: ${command}\n\n${USAGE}`);
    process.exit(2);
}
