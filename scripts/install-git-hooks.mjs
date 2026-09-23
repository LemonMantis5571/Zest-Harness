import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const marker = "# Installed by Zest hooks:install; do not edit this generated wrapper.";
const wrapper = `#!/bin/sh\n${marker}\nset -eu\nrepo_root="$(git rev-parse --show-toplevel)"\nexec sh "$repo_root/.githooks/pre-commit" "$@"\n`;

function main() {
  const configuredHooksPath = spawnSync("git", ["config", "--get", "core.hooksPath"], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  if (configuredHooksPath.error) throw configuredHooksPath.error;
  if (configuredHooksPath.status === 0 && configuredHooksPath.stdout.trim()) {
    throw new Error(
      `core.hooksPath is already set to ${configuredHooksPath.stdout.trim()}. Add the Zest check to that hook path manually; it was not overridden.`,
    );
  }
  if (configuredHooksPath.status !== 1) {
    throw new Error(configuredHooksPath.stderr || "Could not inspect core.hooksPath.");
  }

  const hookPathResult = execFileSync("git", ["rev-parse", "--git-path", "hooks/pre-commit"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim();
  const hookPath = path.isAbsolute(hookPathResult)
    ? hookPathResult
    : path.resolve(repoRoot, hookPathResult);

  if (existsSync(hookPath)) {
    const existing = readFileSync(hookPath, "utf8");
    if (existing === wrapper) {
      console.log("Zest pre-commit hook is already installed.");
      return;
    }
    throw new Error(
      `A pre-commit hook already exists at ${hookPath}. Add the Zest output check to that hook manually; it was not overwritten.`,
    );
  }

  writeFileSync(hookPath, wrapper, { encoding: "utf8", flag: "wx", mode: 0o755 });
  console.log("Installed the Zest pre-commit output-artifact check.");
}

try {
  main();
} catch (error) {
  console.error(error.stderr?.toString("utf8") || error.message);
  process.exitCode = 1;
}
