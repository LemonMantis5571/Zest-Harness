import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const tag = process.argv[2] ?? process.env.GITHUB_REF_NAME;
if (!tag) throw new Error("Pass a release tag, for example v0.1.0-beta.1");

const expected = tag.replace(/^v/, "");
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(expected)) {
  throw new Error(`Release tag is not valid semver: ${tag}`);
}

const cargo = await readFile(resolve("Cargo.toml"), "utf8");
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const tauri = JSON.parse(await readFile(resolve("crates/desktop/tauri.conf.json"), "utf8"));

if (cargoVersion !== expected) {
  throw new Error(`Cargo version ${cargoVersion ?? "missing"} does not match ${expected}`);
}
if (tauri.version !== expected) {
  throw new Error(`Tauri version ${tauri.version ?? "missing"} does not match ${expected}`);
}

console.log(`Release version OK: ${tag}`);
