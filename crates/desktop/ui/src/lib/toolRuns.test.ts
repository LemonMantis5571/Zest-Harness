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
  it("keeps short runs expanded while anything is still live", () => {
    // Rows folding away as each call lands makes a working turn hard to follow.
    const working = [...many(3), tool({ id: "spin", status: "running" })];
    assert.equal(collapseThresholdFor(working), COLLAPSE_THRESHOLD);
    assert.equal(groupToolRuns(working, collapseThresholdFor(working)).length, 4);
  });

  it("folds a finished message's tools once nothing is live", () => {
    // The reported case: four reads that stayed as four rows after the turn
    // ended, because the live-turn threshold is five.
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
    // A settled-looking message with an unanswered prompt in it must not fold
    // the prompt out of sight.
    const pending = [
      ...many(3),
      tool({ id: "ask", name: "bash", status: "awaiting_approval" }),
    ];
    assert.equal(collapseThresholdFor(pending), COLLAPSE_THRESHOLD);
  });
});

describe("groupToolRuns", () => {
  it("leaves a short run expanded", () => {
    const runs = groupToolRuns(many(COLLAPSE_THRESHOLD - 1));
    assert.equal(runs.length, COLLAPSE_THRESHOLD - 1);
    assert.ok(runs.every((r) => r.kind === "single"));
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
    const runs = groupToolRuns(tools);
    assert.equal(runs.length, 2);
    assert.equal(runs[0].kind, "group");
    assert.equal(runs[1].kind, "single");
    if (runs[1].kind === "single") assert.equal(runs[1].tool.id, "live");
    if (runs[0].kind === "group") {
      assert.ok(runs[0].tools.some((item) => item.id === "spin"));
    }
  });

  it("keeps later finished calls inside the first group", () => {
    const tools = [
      ...many(6).map((t, i) => ({ ...t, id: `a${i}` })),
      tool({ id: "live", status: "running" }),
      ...many(6).map((t, i) => ({ ...t, id: `b${i}` })),
    ];
    const runs = groupToolRuns(tools);
    assert.deepEqual(
      runs.map((r) => r.kind),
      ["group"]
    );
    if (runs[0].kind === "group") {
      assert.equal(runs[0].tools.length, 13);
      assert.equal(runs[0].summary.label, "Ran 12 lookups");
    }
  });

  it("does not open a new card stack under a collapsed group", () => {
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
