import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  countDiffLines,
  summarizeTools,
} from "./toolRuns.ts";
import type { ToolPart } from "./types.ts";

function tool(over: Partial<ToolPart> & { id: string }): ToolPart {
  return { name: "read_file", status: "done", ...over };
}

function many(n: number, over: Partial<ToolPart> = {}): ToolPart[] {
  return Array.from({ length: n }, (_, i) =>
    tool({ id: `t${i}`, ...over })
  );
}

describe("countDiffLines", () => {
  it("ignores the file headers", () => {
    const diff = [
      "--- a/src/App.tsx",
      "+++ b/src/App.tsx",
      "@@ -1,3 +1,4 @@",
      " context",
      "-old",
      "+new",
      "+extra",
    ].join("\n");
    // Without the header skip this would report 3 added / 2 removed.
    assert.deepEqual(countDiffLines(diff), { added: 2, removed: 1 });
  });

  it("handles a missing diff", () => {
    assert.deepEqual(countDiffLines(undefined), { added: 0, removed: 0 });
  });
});

describe("summarizeTools", () => {
  it("counts commands, distinct files, and line deltas", () => {
    const summary = summarizeTools([
      tool({ id: "1", name: "bash", path: "npm install" }),
      tool({ id: "2", name: "bash", path: "npm run build" }),
      tool({
        id: "3",
        name: "write_file",
        path: "src/App.tsx",
        diff: "--- a/src/App.tsx\n+++ b/src/App.tsx\n+a\n+b\n-c",
      }),
      tool({
        id: "4",
        name: "edit_file",
        path: "src/main.tsx",
        diff: "--- a/src/main.tsx\n+++ b/src/main.tsx\n+d",
      }),
    ]);
    assert.equal(summary.commands, 2);
    assert.equal(summary.filesEdited, 2);
    assert.equal(summary.added, 3);
    assert.equal(summary.removed, 1);
    assert.equal(summary.label, "Ran 2 commands, edited 2 files");
    assert.equal(summary.inspections, 0);
  });

  it("counts repeated edits to one file once", () => {
    const summary = summarizeTools([
      tool({ id: "1", name: "edit_file", path: "a.ts", diff: "+x" }),
      tool({ id: "2", name: "edit_file", path: "a.ts", diff: "+y" }),
    ]);
    assert.equal(summary.filesEdited, 1, "same path is one file changed");
    assert.equal(summary.added, 2, "but both edits still count lines");
    assert.equal(summary.label, "Ran edited 1 file");
  });

  it("names lookups even when a command is in the same run", () => {
    const summary = summarizeTools([
      tool({ id: "1", name: "bash" }),
      tool({ id: "2", name: "read_file" }),
      tool({ id: "3", name: "grep" }),
    ]);
    assert.equal(summary.label, "Ran 1 command, 2 lookups");
  });

  it("falls back to lookups when nothing was changed", () => {
    const summary = summarizeTools(many(3, { name: "read_file" }));
    assert.equal(summary.inspections, 3);
    assert.equal(summary.label, "Ran 3 lookups");
  });

  it("singularises", () => {
    const summary = summarizeTools([tool({ id: "1", name: "bash" })]);
    assert.equal(summary.label, "Ran 1 command");
  });

  it("tracks errors so a collapsed run cannot hide a failure", () => {
    const summary = summarizeTools([
      tool({ id: "1", name: "bash" }),
      tool({ id: "2", name: "bash", status: "error" }),
    ]);
    assert.equal(summary.errors, 1);
  });
});
