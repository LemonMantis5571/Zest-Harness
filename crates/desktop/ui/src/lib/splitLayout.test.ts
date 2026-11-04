import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  addPaneToLayout,
  canMinimizePane,
  canZoomPane,
  clampSplitRatio,
  collapsedPaneIds,
  isNodeCollapsed,
  layoutPaneIds,
  movePaneInLayout,
  pruneGroupOverrides,
  removePaneFromLayout,
  resolvePaneOverrides,
  restoreAllPanes,
  revealPane,
  splitSidebarGroups,
  stripOrientation,
  toggleMinimizePane,
  toggleZoomPane,
  updateSplitRatio,
  visiblePaneIds,
  wakeMinimizedPanes,
  type LayoutNode,
  type SplitGroup,
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

/** row(a, column(b, c)) with a chat in every pane unless `empty` names one. */
function threePanes(empty: string[] = []) {
  const root: LayoutNode = {
    kind: "split",
    id: "outer",
    axis: "row",
    ratio: 42,
    first: pane("a"),
    second: {
      kind: "split",
      id: "inner",
      axis: "column",
      ratio: 65,
      first: pane("b"),
      second: pane("c"),
    },
  };
  const group: SplitGroup = { id: "g", label: "Split view", root, focusedPaneId: "a" };
  const panes: SplitWorkspaceSnapshot["panes"] = {};
  for (const id of ["a", "b", "c"]) {
    panes[id] = empty.includes(id)
      ? { paneId: id, session: null, target: null, draft: "" }
      : { paneId: id, session: session(`thread-${id}`), target: { root: ".", threadId: `thread-${id}` }, draft: "" };
  }
  return { group, panes };
}

describe("minimizing panes", () => {
  it("draws a minimized pane as a strip and takes it off screen", () => {
    const { group, panes } = threePanes();
    const next = toggleMinimizePane(group, panes, "b");

    assert.deepEqual([...collapsedPaneIds(next, panes)], ["b"]);
    assert.deepEqual(visiblePaneIds(next, panes), ["a", "c"]);
  });

  it("never changes the tree, so restoring returns the same ratios", () => {
    const { group, panes } = threePanes();
    const minimized = toggleMinimizePane(toggleMinimizePane(group, panes, "b"), panes, "c");

    assert.equal(minimized.root, group.root);
    const restored = toggleMinimizePane(minimized, panes, "b");
    assert.equal(restored.root, group.root);
    assert.deepEqual(visiblePaneIds(restored, panes), ["a", "b"]);
  });

  it("refuses to minimize the last visible pane", () => {
    const { group, panes } = threePanes();
    const one = toggleMinimizePane(group, panes, "a");
    const two = toggleMinimizePane(one, panes, "b");

    assert.deepEqual(visiblePaneIds(two, panes), ["c"]);
    assert.equal(canMinimizePane(two, panes, "c"), false);
    assert.equal(toggleMinimizePane(two, panes, "c"), two);
  });

  it("does not offer to minimize a pane that has no chat yet", () => {
    const { group, panes } = threePanes(["c"]);

    assert.equal(canMinimizePane(group, panes, "c"), false);
    assert.equal(toggleMinimizePane(group, panes, "c"), group);
  });

  it("moves focus off a pane that went away", () => {
    const { group, panes } = threePanes();
    const next = toggleMinimizePane(group, panes, "a");

    assert.equal(next.focusedPaneId, "b");
  });

  it("brings a minimized pane back when toggled again", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "b");
    const back = toggleMinimizePane(away, panes, "b");

    assert.deepEqual([...collapsedPaneIds(back, panes)], []);
    assert.equal(back.focusedPaneId, "b");
  });

  it("wakes a minimized pane without moving focus and leaves the others", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(toggleMinimizePane(group, panes, "b"), panes, "c");

    const woken = wakeMinimizedPanes(away, panes, (paneId) => paneId === "b");

    assert.deepEqual([...collapsedPaneIds(woken, panes)], ["c"]);
    assert.equal(woken.focusedPaneId, away.focusedPaneId);
    assert.equal(wakeMinimizedPanes(woken, panes, () => false), woken);
  });
});

describe("zooming panes", () => {
  it("shows one pane and strips for the rest without touching the tree", () => {
    const { group, panes } = threePanes();
    const zoomed = toggleZoomPane(group, panes, "b");

    assert.deepEqual(visiblePaneIds(zoomed, panes), ["b"]);
    assert.deepEqual([...collapsedPaneIds(zoomed, panes)], ["a", "c"]);
    assert.equal(zoomed.root, group.root);
    assert.equal(zoomed.focusedPaneId, "b");
  });

  it("restores the exact layout, including ratios changed beforehand", () => {
    const { group, panes } = threePanes();
    const resized: SplitGroup = { ...group, root: updateSplitRatio(group.root, "outer", 61) };
    const zoomed = toggleZoomPane(resized, panes, "a");
    const restored = toggleZoomPane(zoomed, panes, "a");

    assert.deepEqual(restored.root, resized.root);
    assert.deepEqual(visiblePaneIds(restored, panes), ["a", "b", "c"]);
  });

  it("keeps earlier minimized panes minimized after zoom ends", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "c");
    const zoomed = toggleZoomPane(away, panes, "a");
    assert.deepEqual(visiblePaneIds(zoomed, panes), ["a"]);

    const restored = toggleZoomPane(zoomed, panes, "a");
    assert.deepEqual(visiblePaneIds(restored, panes), ["a", "b"]);
  });

  it("needs a second pane to zoom over", () => {
    const root = pane("only");
    const group: SplitGroup = { id: "g", label: "Split view", root, focusedPaneId: "only" };
    const panes = { only: { paneId: "only", session: session("t"), target: null, draft: "" } };

    assert.equal(canZoomPane(group, panes, "only"), false);
    assert.equal(toggleZoomPane(group, panes, "only"), group);
  });

  it("cannot minimize the zoomed pane, and minimizing a strip brings it back", () => {
    const { group, panes } = threePanes();
    const zoomed = toggleZoomPane(group, panes, "a");

    assert.equal(canMinimizePane(zoomed, panes, "a"), false);
    const revealed = toggleMinimizePane(zoomed, panes, "b");
    assert.deepEqual(visiblePaneIds(revealed, panes), ["a", "b", "c"]);
    assert.equal(revealed.focusedPaneId, "b");
  });

  it("zooming a minimized pane leaves it up once zoom ends", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "b");
    const zoomed = toggleZoomPane(away, panes, "b");
    const restored = toggleZoomPane(zoomed, panes, "b");

    assert.deepEqual(visiblePaneIds(restored, panes), ["a", "b", "c"]);
  });

  it("does not wake a minimized pane while zoomed", () => {
    const { group, panes } = threePanes();
    const zoomed = toggleZoomPane(toggleMinimizePane(group, panes, "c"), panes, "a");

    assert.equal(wakeMinimizedPanes(zoomed, panes, () => true), zoomed);
  });
});

describe("revealing and restoring panes", () => {
  it("revealing a pane ends zoom and unminimizes it, leaving other strips", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "c");
    const zoomed = toggleZoomPane(away, panes, "a");

    const revealed = revealPane(zoomed, panes, "b");

    assert.deepEqual(visiblePaneIds(revealed, panes), ["a", "b"]);
    assert.equal(revealed.focusedPaneId, "b");
  });

  it("revealing the zoomed pane keeps zoom", () => {
    const { group, panes } = threePanes();
    const zoomed = toggleZoomPane(group, panes, "a");

    assert.equal(resolvePaneOverrides(revealPane(zoomed, panes, "a"), panes).zoomed, "a");
  });

  it("restore all clears both zoom and minimize", () => {
    const { group, panes } = threePanes();
    const both = toggleZoomPane(toggleMinimizePane(group, panes, "c"), panes, "a");
    const restored = restoreAllPanes(both);

    assert.deepEqual(visiblePaneIds(restored, panes), ["a", "b", "c"]);
    assert.equal(restoreAllPanes(restored), restored);
    assert.equal(restored.root, group.root);
  });
});

describe("overrides after the tree changes", () => {
  it("drops a closed pane from the minimized set", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "c");
    const root = removePaneFromLayout(away.root, "c");
    assert.ok(root);

    const pruned = pruneGroupOverrides({ ...away, root }, panes);

    assert.deepEqual(pruned.minimizedPaneIds, []);
    assert.deepEqual(visiblePaneIds(pruned, panes), ["a", "b"]);
  });

  it("ends zoom when the zoomed pane closes", () => {
    const { group, panes } = threePanes();
    const zoomed = toggleZoomPane(group, panes, "a");
    const root = removePaneFromLayout(zoomed.root, "a");
    assert.ok(root);

    const pruned = pruneGroupOverrides({ ...zoomed, root, focusedPaneId: "b" }, panes);

    assert.equal(pruned.zoomedPaneId, undefined);
    assert.deepEqual(visiblePaneIds(pruned, panes), ["b", "c"]);
  });

  it("restores everything when the only visible pane closes", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(toggleMinimizePane(group, panes, "b"), panes, "c");
    const root = removePaneFromLayout(away.root, "a");
    assert.ok(root);

    const pruned = pruneGroupOverrides({ ...away, root }, panes);

    assert.deepEqual(visiblePaneIds(pruned, panes), ["b", "c"]);
    assert.deepEqual(pruned.minimizedPaneIds, []);
  });

  it("ignores a minimized pane whose chat was deleted", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "b");
    const emptied = { ...panes, b: { paneId: "b", session: null, target: null, draft: "" } };

    assert.deepEqual(visiblePaneIds(away, emptied), ["a", "b", "c"]);
    assert.equal(pruneGroupOverrides(away, panes), away);
  });

  it("moves focus off a minimized pane when a pane closes", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "b");
    const root = removePaneFromLayout(away.root, "c");
    assert.ok(root);

    // The caller picks the next focus without knowing which panes are strips.
    const pruned = pruneGroupOverrides({ ...away, root, focusedPaneId: "b" }, panes);

    assert.equal(pruned.focusedPaneId, "a");
    assert.equal(pruneGroupOverrides(pruned, panes), pruned);
  });

  it("survives a pane move because overrides follow pane ids", () => {
    const { group, panes } = threePanes();
    const away = toggleMinimizePane(group, panes, "b");
    const moved = { ...away, root: movePaneInLayout(away.root, "b", "a", "row", true) };

    assert.deepEqual([...collapsedPaneIds(moved, panes)], ["b"]);
  });
});

describe("drawing strips", () => {
  it("finds subtrees that hold only strips", () => {
    const { group, panes } = threePanes();
    const collapsed = collapsedPaneIds(toggleZoomPane(group, panes, "a"), panes);
    const root = group.root;
    assert.equal(root.kind, "split");
    if (root.kind !== "split") return;

    assert.equal(isNodeCollapsed(root, collapsed), false);
    assert.equal(isNodeCollapsed(root.first, collapsed), false);
    assert.equal(isNodeCollapsed(root.second, collapsed), true);
  });

  it("runs a strip along the thin side of the room it leaves", () => {
    assert.equal(stripOrientation("row"), "vertical");
    assert.equal(stripOrientation("column"), "horizontal");
  });
});
