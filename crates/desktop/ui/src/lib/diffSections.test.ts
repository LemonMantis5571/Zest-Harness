import assert from "node:assert/strict";
import test from "node:test";

import { splitDiffSections } from "./diffSections.ts";

test("splits a git diff into file sections and counts changes", () => {
  const sections = splitDiffSections(
    [
      "diff --git a/ARCHITECTURE.md b/ARCHITECTURE.md",
      "--- a/ARCHITECTURE.md",
      "+++ b/ARCHITECTURE.md",
      "@@ -1 +1,2 @@",
      " old",
      "+new",
      "diff --git a/Cargo.lock b/Cargo.lock",
      "--- a/Cargo.lock",
      "+++ b/Cargo.lock",
      "@@ -4 +4 @@",
      "-old",
      "+new",
    ].join("\n")
  );

  assert.deepEqual(
    sections.map(({ path, added, removed }) => ({ path, added, removed })),
    [
      { path: "ARCHITECTURE.md", added: 1, removed: 0 },
      { path: "Cargo.lock", added: 1, removed: 1 },
    ]
  );
});

test("keeps a tool diff as one section when it has no file headers", () => {
  const [section] = splitDiffSections("-old\n+new", "src/app.ts");

  assert.equal(section.path, "src/app.ts");
  assert.equal(section.added, 1);
  assert.equal(section.removed, 1);
});

test("recognizes multiple plain unified sections", () => {
  const sections = splitDiffSections(
    [
      "--- a/one.ts",
      "+++ b/one.ts",
      "@@ -1 +1 @@",
      "+one",
      "--- a/two.ts",
      "+++ b/two.ts",
      "@@ -1 +1 @@",
      "+two",
    ].join("\n")
  );

  assert.deepEqual(
    sections.map((section) => section.path),
    ["one.ts", "two.ts"]
  );
});
