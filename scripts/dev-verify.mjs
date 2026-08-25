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
  ["UI tests", npm, [...npmPrefix, "run", "ui:test"]],
  ["UI lint", npm, [...npmPrefix, "run", "ui:lint"]],
  ["UI lint plugin tests", npm, [...npmPrefix, "run", "ui:lint:plugins"]],
  ["UI build", npm, [...npmPrefix, "run", "ui:build"]],
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
