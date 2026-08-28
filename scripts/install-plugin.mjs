import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const PLUGINS = {
  "now-playing": {
    package: "zest-now-playing-plugin",
    bin: "zest-now-playing",
    source: "crates/plugins/now-playing",
    windowsOnly: true,
  },
  wallpaper: {
    package: "zest-wallpaper-plugin",
    bin: "zest-wallpaper",
    source: "crates/plugins/wallpaper",
    windowsOnly: false,
  },
};

function usage() {
  console.log(`Build a Zest add-on and copy it into the local plugin folder.

Usage:
  npm run plugin:install -- <id>
  npm run plugin:install -- --all

Add-ons:
  now-playing   Music controls (Windows)
  wallpaper     App background image

Needs Cargo on PATH. After install, open Customize > Extras, press Refresh,
then Turn on.
`);
}

function pluginRoot() {
  if (process.platform === "win32") {
    const local = process.env.LOCALAPPDATA;
    if (!local) {
      throw new Error("Could not find the Windows local app data folder.");
    }
    return path.join(local, "Zest", "plugins");
  }
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support", "Zest", "plugins");
  }
  const xdg = process.env.XDG_DATA_HOME;
  return path.join(xdg || path.join(os.homedir(), ".local", "share"), "Zest", "plugins");
}

function cargoTargetDir() {
  const result = spawnSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
    env: process.env,
    shell: false,
  });
  if (result.error) {
    throw new Error(`Could not start cargo: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || "").trim();
    throw new Error(detail || "Could not ask cargo for the target directory.");
  }
  const meta = JSON.parse(result.stdout);
  if (typeof meta.target_directory !== "string" || !meta.target_directory) {
    throw new Error("Cargo did not report a target directory.");
  }
  return meta.target_directory;
}

function findBuiltBinary(targetDir, exeName) {
  const candidates = [path.join(targetDir, "release", exeName)];
  const triple = process.env.CARGO_BUILD_TARGET?.trim();
  if (triple) {
    candidates.unshift(path.join(targetDir, triple, "release", exeName));
  }

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }

  let found = [];
  try {
    for (const entry of fs.readdirSync(targetDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const candidate = path.join(targetDir, entry.name, "release", exeName);
      if (fs.existsSync(candidate)) found.push(candidate);
    }
  } catch {
    return null;
  }

  if (found.length === 0) return null;
  found.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  return found[0];
}

function runCargo(pkg) {
  const result = spawnSync("cargo", ["build", "-p", pkg, "--release"], {
    cwd: root,
    env: process.env,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) {
    console.error(`Could not start cargo: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function installOne(id) {
  const plugin = PLUGINS[id];
  if (!plugin) {
    console.error(`Unknown add-on: ${id}`);
    usage();
    process.exit(1);
  }
  if (plugin.windowsOnly && process.platform !== "win32") {
    console.error(`${id} only works on Windows (it reads the Windows media session).`);
    process.exit(1);
  }

  console.log(`Building ${plugin.package}…`);
  runCargo(plugin.package);

  const exeName = process.platform === "win32" ? `${plugin.bin}.exe` : plugin.bin;
  const built = findBuiltBinary(cargoTargetDir(), exeName);
  if (!built) {
    console.error(`Build finished, but ${exeName} was not in the Cargo target folder.`);
    process.exit(1);
  }

  const destDir = path.join(pluginRoot(), id);
  fs.mkdirSync(destDir, { recursive: true });

  const destExe = path.join(destDir, exeName);
  fs.copyFileSync(built, destExe);
  if (process.platform !== "win32") {
    fs.chmodSync(destExe, 0o755);
  }

  const srcManifest = path.join(root, plugin.source, "plugin.json");
  const manifest = JSON.parse(fs.readFileSync(srcManifest, "utf8"));
  manifest.executable = exeName;
  fs.writeFileSync(path.join(destDir, "plugin.json"), `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(`Installed ${id} to ${destDir}`);
}

const args = process.argv.slice(2).filter((arg) => arg !== "--");
if (args.includes("-h") || args.includes("--help")) {
  usage();
  process.exit(0);
}

const all = args.includes("--all");
const ids = all ? Object.keys(PLUGINS) : args.filter((arg) => !arg.startsWith("-"));
if (ids.length === 0) {
  usage();
  process.exit(1);
}

for (const extra of args.filter((arg) => arg.startsWith("-") && arg !== "--all")) {
  console.error(`Unknown flag: ${extra}`);
  usage();
  process.exit(1);
}

for (const id of ids) {
  if (all && PLUGINS[id]?.windowsOnly && process.platform !== "win32") {
    console.log(`Skipping ${id} (Windows only).`);
    continue;
  }
  installOne(id);
}

console.log("Open Customize > Extras, press Refresh, then Turn on.");
