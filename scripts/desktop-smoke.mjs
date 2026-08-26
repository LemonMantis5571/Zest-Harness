import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
if (process.env.ZEST_DESKTOP_SMOKE !== "1") {
  console.log("desktop smoke: SKIPPED (set ZEST_DESKTOP_SMOKE=1 to opt in)");
  process.exit(0);
}

const binary =
  process.env.ZEST_DESKTOP_BINARY ||
  path.join(
    root,
    "target",
    process.env.ZEST_DESKTOP_PROFILE === "release" ? "release" : "debug",
    process.platform === "win32" ? "zest-desktop.exe" : "zest-desktop"
  );

if (!fs.existsSync(binary)) {
  console.log("desktop smoke: SKIPPED (binary not found at " + binary + ")");
  process.exit(0);
}

const child = spawn(binary, [], {
  cwd: root,
  env: { ...process.env, ZEST_DESKTOP_SMOKE: "1" },
  stdio: "ignore",
  windowsHide: true,
});

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let exited = false;
let exitCode = null;
child.once("exit", (code) => {
  exited = true;
  exitCode = code;
});

await wait(4_000);
if (exited) {
  throw new Error(
    "desktop smoke: process exited during startup (" + (exitCode ?? "unknown") + ")"
  );
}

if (process.platform === "win32") {
  try {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$process = Get-Process -Id " +
          child.pid +
          " -ErrorAction Stop; if (-not $process.Responding) { exit 1 }",
      ],
      { stdio: "ignore", windowsHide: true }
    );
  } catch {
    child.kill();
    throw new Error("desktop smoke: the native window did not become responsive");
  }
}

child.kill();
await wait(1_000);
if (!exited) {
  child.kill("SIGKILL");
  await wait(250);
}
if (!exited) {
  throw new Error("desktop smoke: process did not shut down cleanly");
}
console.log("desktop smoke: started, became responsive, and shut down");
