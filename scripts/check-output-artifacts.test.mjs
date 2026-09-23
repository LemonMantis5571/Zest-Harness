import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  findOutputArtifactViolations,
  parseCommitRangeEntries,
  OUTPUT_SIZE_LIMIT_BYTES,
} from "./check-output-artifacts.mjs";

test("rejects media files under outputs regardless of extension casing", () => {
  const violations = findOutputArtifactViolations([
    { path: "outputs/demo.MP4", size: 128 },
  ]);

  assert.equal(violations.length, 1);
  assert.match(violations[0].reasons[0], /\.mp4 media files/);
});

test("rejects a media artifact added and later removed within the checked commit range", (t) => {
  const fixtureRoot = mkdtempSync(path.join(os.tmpdir(), "zest-output-policy-"));
  t.after(() => rmSync(fixtureRoot, { recursive: true, force: true }));

  const git = (args) => execFileSync("git", args, { cwd: fixtureRoot });
  git(["init", "--quiet"]);
  git(["config", "user.name", "Output Policy Test"]);
  git(["config", "user.email", "output-policy@example.invalid"]);
  writeFileSync(path.join(fixtureRoot, "README.md"), "base\n");
  git(["add", "README.md"]);
  git(["commit", "--quiet", "-m", "base"]);
  const base = git(["rev-parse", "HEAD"]).toString("utf8").trim();

  mkdirSync(path.join(fixtureRoot, "outputs"));
  writeFileSync(path.join(fixtureRoot, "outputs", "transient.mp4"), "test");
  git(["add", "outputs/transient.mp4"]);
  git(["commit", "--quiet", "-m", "add generated media"]);
  git(["rm", "--quiet", "outputs/transient.mp4"]);
  git(["commit", "--quiet", "-m", "remove generated media"]);
  const head = git(["rev-parse", "HEAD"]).toString("utf8").trim();

  const entries = parseCommitRangeEntries(base, head, git);
  assert.equal(
    findOutputArtifactViolations(entries).some(({ path: filePath }) =>
      filePath.endsWith("transient.mp4"),
    ),
    true,
  );
});

test("rejects output files larger than 1 MiB", () => {
  const violations = findOutputArtifactViolations([
    { path: "outputs/debug-notes/capture.bin", size: OUTPUT_SIZE_LIMIT_BYTES + 1 },
  ]);

  assert.equal(violations.length, 1);
  assert.match(violations[0].reasons[0], /exceeds the 1 MiB limit/);
});

test("allows small text reports and unrelated assets", () => {
  const violations = findOutputArtifactViolations([
    { path: "outputs/latency/RESULTS.md", size: 4096 },
    { path: "crates/desktop/ui/src/assets/demo.mp4", size: 3 * 1024 * 1024 },
  ]);

  assert.deepEqual(violations, []);
});

test("rejects small common image formats under outputs", () => {
  const violations = findOutputArtifactViolations([
    { path: "outputs/screenshot.png", size: 1024 },
    { path: "outputs/thumb.webp", size: 2048 },
  ]);

  assert.equal(violations.length, 2);
});

test("allows only exact paths explicitly approved", () => {
  const allowlist = new Set(["outputs/approved/demo.mp4"]);
  const violations = findOutputArtifactViolations(
    [
      { path: "outputs/approved/demo.mp4", size: 3 * 1024 * 1024 },
      { path: "outputs/approved/other.mp4", size: 32 },
    ],
    allowlist,
  );

  assert.deepEqual(violations.map(({ path: filePath }) => filePath), [
    "outputs/approved/other.mp4",
  ]);
});
