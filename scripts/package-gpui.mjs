// Build the native desktop and package the same executable into each OS format.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = path.join(repo, "crates/desktop/Cargo.toml");
const packaging = path.join(repo, "packaging/gpui");
const art = path.join(repo, "crates/desktop");
const { values } = parseArgs({ options: { out: { type: "string" }, binary: { type: "string" } } });
const windows = process.platform === "win32";
if ((!windows && process.platform !== "linux") || process.arch !== "x64") {
  throw new Error("Zest desktop installers currently target x86_64 Windows and Linux.");
}

function run(command, args, capture = false) {
  const result = spawnSync(command, args, { cwd: repo, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status ?? result.signal}`);
  return result.stdout;
}

function windowsTool(name, directories) {
  const candidates = [...process.env.PATH.split(path.delimiter), ...directories.filter(Boolean)];
  const dir = candidates.find(dir => existsSync(path.join(dir, name)));
  if (!dir) throw new Error(`Install ${name === "makensis.exe" ? "NSIS 3" : "WiX Toolset 3"} and place ${name} on PATH.`);
  return path.join(dir, name);
}

const workspace = readFileSync(path.join(repo, "Cargo.toml"), "utf8");
const version = workspace.match(/^\[workspace\.package\]\s*\r?\n(?:[^\[]*?)^version\s*=\s*"([\d.]+(?:-[A-Za-z0-9.-]+)?)"/m)?.[1];
if (!version) throw new Error("Cannot read the release version from [workspace.package].");
const output = path.resolve(values.out ?? path.join(repo, "target/gpui/bundle", version, windows ? "windows" : "linux"));
let binary;
if (values.binary) {
  binary = path.resolve(values.binary);
} else {
  const { target_directory: target } = JSON.parse(run("cargo", ["metadata", "--manifest-path", manifest, "--no-deps", "--format-version", "1"], true));
  run("cargo", ["build", "--manifest-path", manifest, "--locked", "--release", "--bin", "zest-desktop"]);
  binary = path.join(target, "release", windows ? "zest-desktop.exe" : "zest-desktop");
}
if (!existsSync(binary)) throw new Error(`Missing release executable: ${binary}`);

mkdirSync(output, { recursive: true });
const stage = mkdtempSync(path.join(output, ".pack-"));
const scratch = mkdtempSync(path.join(os.tmpdir(), "zest-installer-"));
try {
  if (windows) {
    // Machines that built the former Tauri app keep WiX and NSIS in its tool cache.
    const cache = path.join(process.env.LOCALAPPDATA, "tauri");
    const wix = [process.env.WIX && path.join(process.env.WIX, "bin"), path.join(cache, "WixTools314")];
    const nsis = [process.env["ProgramFiles(x86)"] && path.join(process.env["ProgramFiles(x86)"], "NSIS"), path.join(cache, "NSIS")];
    const numericVersion = version.split("-")[0];
    const license = path.join(repo, "LICENSE");
    const licenseRtf = path.join(scratch, "License.rtf");
    const text = readFileSync(license, "utf8").replace(/[\\{}]/g, "\\$&").replace(/\r?\n/g, "\\par\n");
    writeFileSync(licenseRtf, "{\\rtf1\\ansi " + text + "}");
    const object = path.join(scratch, "installer.wixobj");
    const msi = path.join(stage, `Zest_${version}_x64_en-US.msi`);
    run(windowsTool("candle.exe", wix), ["-nologo", "-arch", "x64", `-dBinary=${binary}`, `-dNumericVersion=${numericVersion}`,
      `-dArt=${art}`, `-dLicenseRtf=${licenseRtf}`, "-out", object, path.join(packaging, "windows/installer.wxs")]);
    run(windowsTool("light.exe", wix), ["-nologo", "-ext", "WixUIExtension", "-spdb", "-out", msi, object]);
    run(windowsTool("makensis.exe", nsis), ["/V2", `/DBinary=${binary}`, `/DArt=${art}`, `/DLicense=${license}`,
      `/DNumericVersion=${numericVersion}`, `/DVersion=${version}`, `/DOutput=${path.join(stage, `Zest_${version}_x64-setup.exe`)}`,
      path.join(packaging, "windows/installer.nsi")]);
  } else {
    run("bash", [path.join(packaging, "linux/package.sh"), repo, binary, version, stage, scratch]);
  }
  const files = readdirSync(stage);
  for (const file of files) renameSync(path.join(stage, file), path.join(output, file));
  console.log(files.map(file => path.join(output, file)).join("\n"));
} finally {
  // Both directories are unique, created by this invocation.
  rmSync(stage, { recursive: true, force: true });
  rmSync(scratch, { recursive: true, force: true });
}
