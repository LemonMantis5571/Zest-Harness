import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  addPaneToLayout,
  clampSplitRatio,
  layoutPaneIds,
  movePaneInLayout,
  removePaneFromLayout,
  splitSidebarGroups,
  updateSplitRatio,
  type LayoutNode,
  type SplitWorkspaceSnapshot,
} from "./splitLayout.ts";
import type { SessionInfo } from "./types.ts";

function session(threadId: string, label = threadId): SessionInfo {
  return {
    threadId,
    label,
    provider: "fixture",
    root: ".",
    isFreeChat: false,
  } as SessionInfo;
}

function pane(paneId: string): LayoutNode {
  return { kind: "pane", paneId };
}

describe("split layout tree", () => {
  it("nests a third pane inside the selected side", () => {
    const twoPanes: LayoutNode = {
      kind: "split",
      id: "root",
      axis: "row",
      ratio: 50,
      first: pane("one"),
      second: pane("two"),
    };

    const threePanes = addPaneToLayout(twoPanes, "two", "three", "column");

    assert.deepEqual(layoutPaneIds(threePanes), ["one", "two", "three"]);
    assert.equal(threePanes.kind, "split");
    if (threePanes.kind !== "split" || threePanes.second.kind !== "split") return;
    assert.equal(threePanes.second.axis, "column");
    assert.deepEqual(layoutPaneIds(threePanes.second), ["two", "three"]);
  });

  it("removes a leaf and collapses its parent", () => {
    const tree: LayoutNode = {
      kind: "split",
      id: "root",
      axis: "row",
      ratio: 50,
      first: pane("one"),
      second: {
        kind: "split",
        id: "nested",
        axis: "column",
        ratio: 50,
        first: pane("two"),
        second: pane("three"),
      },
    };

    const remaining = removePaneFromLayout(tree, "two");

    assert.deepEqual(layoutPaneIds(remaining!), ["one", "three"]);
    assert.equal(remaining?.kind, "split");
    if (remaining?.kind !== "split") return;
    assert.equal(remaining.second.kind, "pane");
    assert.equal(remaining.second.paneId, "three");
  });

  it("moves a pane around another leaf without duplicating it", () => {
    const tree: LayoutNode = {
      kind: "split",
      id: "root",
      axis: "row",
      ratio: 50,
      first: pane("one"),
      second: pane("two"),
    };

    const moved = movePaneInLayout(tree, "one", "two", "column", false);

    assert.deepEqual(layoutPaneIds(moved), ["two", "one"]);
    assert.equal(moved.kind, "split");
    if (moved.kind !== "split") return;
    assert.equal(moved.axis, "column");
  });

  it("keeps divider ratios within the accessible bounds", () => {
    assert.equal(clampSplitRatio(1), 30);
    assert.equal(clampSplitRatio(51.7), 52);
    assert.equal(clampSplitRatio(99), 70);

    const tree: LayoutNode = {
      kind: "split",
      id: "root",
      axis: "row",
      ratio: 50,
      first: pane("one"),
      second: pane("two"),
    };
    const updated = updateSplitRatio(tree, "root", 12);
    assert.equal(updated.kind, "split");
    if (updated.kind === "split") assert.equal(updated.ratio, 30);
  });

  it("shows only populated panes in the sidebar group", () => {
    const snapshot: SplitWorkspaceSnapshot = {
      activeGroupId: "group",
      groups: [
        {
          id: "group",
          label: "Split view",
          focusedPaneId: "one",
          root: {
            kind: "split",
            id: "root",
            axis: "row",
            ratio: 50,
            first: pane("one"),
            second: pane("empty"),
          },
        },
      ],
      panes: {
        one: { paneId: "one", session: session("thread-one", "First"), target: { root: ".", threadId: "thread-one" }, draft: "" },
        empty: { paneId: "empty", session: null, target: null, draft: "" },
      },
    };

    const groups = splitSidebarGroups(snapshot);

    assert.equal(groups.length, 1);
    assert.equal(groups[0].panes.length, 1);
    assert.equal(groups[0].panes[0].title, "First");
    assert.equal(splitSidebarGroups(snapshot, null)[0].active, false);
  });
});
