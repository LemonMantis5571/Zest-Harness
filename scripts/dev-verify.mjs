import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const npm = process.platform === "win32" ? process.execPath : "npm";
const npmPrefix =
  process.platform === "win32"
    ? [
        process.env.npm_execpath ??
          path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
      ]
    : [];

const steps = [
  [
    "Output artifact policy tests",
    process.execPath,
    ["--test", "./scripts/check-output-artifacts.test.mjs"],
  ],
  ["Output artifact policy", process.execPath, ["./scripts/check-output-artifacts.mjs"]],
  ["Feature map tests", process.execPath, ["--test", "./scripts/feature-map.test.mjs"]],
  ["Feature map", process.execPath, ["./scripts/feature-map.mjs", "check"]],
  ["UI control CLI tests", process.execPath, ["--test", "./scripts/zest-control.test.mjs"]],
  ["UI tests", npm, [...npmPrefix, "run", "ui:test"]],
  ["UI lint", npm, [...npmPrefix, "run", "ui:lint"]],
  ["UI lint plugin tests", npm, [...npmPrefix, "run", "ui:lint:plugins"]],
  ["UI build", npm, [...npmPrefix, "run", "ui:build"]],
  // Needs Playwright's Chromium once per machine: npx playwright install chromium
  ["UI end-to-end tests", npm, [...npmPrefix, "run", "ui:e2e"]],
  // Drives the live UI through scripts/zest-control.mjs, so the agent-facing
  // control CLI is exercised on every run and cannot rot unnoticed.
  ["UI control smoke", process.execPath, ["./scripts/zest-control.mjs", "check"]],
  ["Rust formatting", "cargo", ["fmt", "--all", "--", "--check"]],
  [
    "Rust clippy",
    "cargo",
    ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
  ],
  ["Rust library tests", "cargo", ["test", "--workspace", "--lib"]],
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
