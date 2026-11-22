import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const steps = [
  [
    "Output artifact policy tests",
    process.execPath,
    ["--test", "./scripts/check-output-artifacts.test.mjs"],
  ],
  ["Output artifact policy", process.execPath, ["./scripts/check-output-artifacts.mjs"]],
  ["Feature map tests", process.execPath, ["--test", "./scripts/feature-map.test.mjs"]],
  ["Feature map", process.execPath, ["./scripts/feature-map.mjs", "check"]],
  ["Rust formatting", "cargo", ["fmt", "--all", "--", "--check"]],
  [
    "Rust clippy",
    "cargo",
    ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
  ],
  ["Rust library tests", "cargo", ["test", "--workspace", "--lib"]],
  // The desktop app is its own Cargo workspace (GPUI is pinned to a Zed revision).
  [
    "Desktop tests",
    "cargo",
    ["test", "--locked", "--release", "--manifest-path", "crates/desktop/Cargo.toml"],
  ],
  ["Git whitespace", "git", ["diff", "--check"]],
];

for (const [name, command, args] of steps) {
  console.log(`\n==> ${name}`);
  const result = spawnSync(command, args, {
    cwd: root,
    env: process.env,
    stdio: "inherit",
    shell: false,
  });

  if (result.error) {
    console.error(`${name} failed to start: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`${name} failed with exit ${result.status ?? "unknown"}`);
    process.exit(result.status ?? 1);
  }
}

console.log("\ndev-verify passed");
