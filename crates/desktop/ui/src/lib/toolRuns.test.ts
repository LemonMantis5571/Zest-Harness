import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  COLLAPSE_THRESHOLD,
  collapseThresholdFor,
  countDiffLines,
  groupToolRuns,
  SETTLED_COLLAPSE_THRESHOLD,
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

describe("collapseThresholdFor", () => {
  it("folds a live turn the same way as a finished one", () => {
    const working = [...many(8), tool({ id: "spin", status: "running" })];
    assert.equal(collapseThresholdFor(working), COLLAPSE_THRESHOLD);
    const runs = groupToolRuns(working, collapseThresholdFor(working));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "group");
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, 9);
    }
  });

  it("folds a finished message's tools once nothing is live", () => {
    const finished = many(4);
    assert.equal(collapseThresholdFor(finished), SETTLED_COLLAPSE_THRESHOLD);

    const runs = groupToolRuns(finished, collapseThresholdFor(finished));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "group");
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, 4);
      assert.equal(runs[0].summary.label, "Ran 4 lookups");
    }
  });

  it("still shows a lone finished call as itself", () => {
    // "Ran 1 lookup" is longer than the row it replaces and hides the path.
    const runs = groupToolRuns(many(1), collapseThresholdFor(many(1)));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "single");
  });

  it("waits for an approval that is still pending", () => {
    const pending = [
      ...many(3),
      tool({ id: "ask", name: "bash", status: "awaiting_approval" }),
    ];
    const runs = groupToolRuns(pending, collapseThresholdFor(pending));
    assert.equal(runs.length, 2);
    assert.equal(runs[0].kind, "group");
    assert.equal(runs[1].kind, "single");
    if (runs[1].kind === "single") assert.equal(runs[1].tool.id, "ask");
  });
});

describe("groupToolRuns", () => {
  it("leaves a single call as a row", () => {
    const runs = groupToolRuns(many(1));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "single");
  });

  it("collapses once the run reaches the threshold", () => {
    const runs = groupToolRuns(many(COLLAPSE_THRESHOLD));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "group");
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, COLLAPSE_THRESHOLD);
    }
  });

  it("never folds an awaiting-approval row into the summary", () => {
    const tools = [
      ...many(6),
      tool({ id: "live", name: "bash", status: "awaiting_approval" }),
      tool({ id: "spin", name: "bash", status: "running" }),
    ];
    const runs = groupToolRuns(tools, collapseThresholdFor(tools));
    assert.equal(runs[0]?.kind, "group");
    assert.equal(runs[1]?.kind, "single");
    if (runs[1].kind === "single") assert.equal(runs[1].tool.id, "live");
    assert.equal(runs[2]?.kind, "single");
    if (runs[2].kind === "single") assert.equal(runs[2].tool.id, "spin");
  });

  it("keeps later calls inside the first group", () => {
    const tools = [
      ...many(6).map((t, i) => ({ ...t, id: `a${i}` })),
      tool({ id: "live", status: "running" }),
      ...many(6).map((t, i) => ({ ...t, id: `b${i}` })),
    ];
    const runs = groupToolRuns(tools, collapseThresholdFor(tools));
    assert.deepEqual(
      runs.map((r) => r.kind),
      ["group"]
    );
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, 13);
      assert.equal(runs[0].summary.label, "Ran 12 lookups");
    }
  });

  it("does not open a new card stack under a folded group", () => {
    const tools = [
      ...many(6),
      tool({ id: "a", name: "mcp__Haiku__context_shell", status: "done" }),
      tool({ id: "b", name: "mcp__Haiku__context_shell", status: "done" }),
      tool({ id: "c", name: "mcp__Haiku__context_shell", status: "done" }),
      tool({ id: "spin", name: "mcp__Haiku__context_shell", status: "running" }),
    ];
    const runs = groupToolRuns(tools, collapseThresholdFor(tools));
    assert.equal(runs.length, 1);
    assert.equal(runs[0].kind, "group");
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, 10);
      assert.equal(runs[0].summary.inspections, 9);
    }
  });

  it("starts a new run after an approval instead of jumping the queue", () => {
    const tools = [
      ...many(3),
      tool({ id: "ask", name: "bash", status: "awaiting_approval" }),
      tool({ id: "after-1", status: "done" }),
      tool({ id: "after-2", status: "done" }),
    ];
    const runs = groupToolRuns(tools, collapseThresholdFor(tools));
    assert.deepEqual(
      runs.map((r) => r.kind),
      ["group", "single", "group"]
    );
    if (runs[2].kind === "group") {
      assert.deepEqual(
        runs[2].tools.map((item) => item.id),
        ["after-1", "after-2"]
      );
    }
  });

  it("preserves order and loses nothing", () => {
    const tools = [...many(7), tool({ id: "live", status: "running" }), ...many(2)];
    const flat = groupToolRuns(tools).flatMap((r) =>
      r.kind === "group" ? r.tools : [r.tool]
    );
    assert.deepEqual(
      flat.map((t) => t.id),
      tools.map((t) => t.id)
    );
  });

  it("handles an empty list", () => {
    assert.deepEqual(groupToolRuns([]), []);
  });
});
