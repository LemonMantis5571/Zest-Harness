import type { SessionInfo } from "./types";

/** A durable or not-yet-saved chat that can occupy a split pane. */
export type SplitChatTarget = {
  root: string | null;
  threadId?: string;
  newThread?: boolean;
  providerId?: string;
};

export type SplitAxis = "row" | "column";

/** A binary tree makes arbitrary nested layouts possible without special cases. */
export type LayoutNode =
  | {
      kind: "pane";
      paneId: string;
    }
  | {
      kind: "split";
      id: string;
      axis: SplitAxis;
      ratio: number;
      first: LayoutNode;
      second: LayoutNode;
    };

export type SplitPaneSnapshot = {
  paneId: string;
  session: SessionInfo | null;
  target: SplitChatTarget | null;
  draft: string;
  /** Sidebar title captured from the chat catalogue when available. */
  title?: string;
};

export type SplitGroup = {
  id: string;
  label: string;
  root: LayoutNode;
  focusedPaneId: string;
  /**
   * The pane shown alone while every other pane is drawn as a strip. A
   * render-time override: `root` and its ratios are never touched, so clearing
   * it brings the previous layout back exactly.
   */
  zoomedPaneId?: string;
  /** Panes put away as strips while their turns keep running. */
  minimizedPaneIds?: string[];
};

export type SplitWorkspaceSnapshot = {
  groups: SplitGroup[];
  panes: Record<string, SplitPaneSnapshot>;
  activeGroupId: string;
};

export type SplitSidebarPane = {
  paneId: string;
  threadId: string;
  title: string;
  projectLabel: string;
};

export type SplitSidebarGroup = {
  id: string;
  label: string;
  active: boolean;
  panes: SplitSidebarPane[];
};

let fallbackId = 0;

export function createSplitId(prefix: string): string {
  const random =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${fallbackId++}`;
  return `${prefix}-${random}`;
}

export function targetForSession(session: SessionInfo): SplitChatTarget {
  return {
    root: session.isFreeChat ? null : session.root,
    threadId: session.threadId,
  };
}

export function initialTargetForSession(session: SessionInfo): SplitChatTarget {
  const hasTranscript =
    session.messages.length > 0 ||
    session.hasOlderMessages ||
    session.hasNewerMessages;
  return hasTranscript
    ? targetForSession(session)
    : {
        root: session.isFreeChat ? null : session.root,
        newThread: true,
        providerId: session.provider,
      };
}

function paneIdFor(prefix: string) {
  return createSplitId(prefix);
}

export function createSplitWorkspaceSnapshot(
  session: SessionInfo,
  draft: string,
  title?: string
): SplitWorkspaceSnapshot {
  const paneId = paneIdFor("pane");
  const emptyPaneId = paneIdFor("pane");
  const groupId = createSplitId("split");
  return {
    groups: [
      {
        id: groupId,
        label: "Split view",
        root: {
          kind: "split",
          id: createSplitId("layout"),
          axis: "row",
          ratio: 50,
          first: { kind: "pane", paneId },
          second: { kind: "pane", paneId: emptyPaneId },
        },
        focusedPaneId: paneId,
      },
    ],
    panes: {
      [paneId]: {
        paneId,
        session,
        target: initialTargetForSession(session),
        draft,
        title,
      },
      [emptyPaneId]: {
        paneId: emptyPaneId,
        session: null,
        target: null,
        draft: "",
      },
    },
    activeGroupId: groupId,
  };
}

/** Add a fresh group while retaining all previous groups. */
export function appendSplitGroup(
  snapshot: SplitWorkspaceSnapshot | null,
  session: SessionInfo,
  draft: string,
  title?: string
): SplitWorkspaceSnapshot {
  const next = createSplitWorkspaceSnapshot(session, draft, title);
  if (!snapshot) return next;
  return {
    groups: [...snapshot.groups, ...next.groups],
    panes: { ...snapshot.panes, ...next.panes },
    activeGroupId: next.activeGroupId,
  };
}

export function clampSplitRatio(value: number): number {
  return Math.max(30, Math.min(70, Math.round(value)));
}

export function layoutPaneIds(node: LayoutNode): string[] {
  if (node.kind === "pane") return [node.paneId];
  return [...layoutPaneIds(node.first), ...layoutPaneIds(node.second)];
}

export function layoutContainsPane(node: LayoutNode, paneId: string): boolean {
  return layoutPaneIds(node).includes(paneId);
}

/**
 * Split the selected leaf. The caller chooses row (left/right) or column
 * (top/bottom), which is what lets a right-hand pane become vertically nested.
 */
export function addPaneToLayout(
  node: LayoutNode,
  anchorPaneId: string,
  newPaneId: string,
  axis: SplitAxis
): LayoutNode {
  if (node.kind === "pane") {
    if (node.paneId !== anchorPaneId) return node;
    return {
      kind: "split",
      id: createSplitId("layout"),
      axis,
      ratio: 50,
      first: node,
      second: { kind: "pane", paneId: newPaneId },
    };
  }

  const first = addPaneToLayout(node.first, anchorPaneId, newPaneId, axis);
  if (first !== node.first) return { ...node, first };
  const second = addPaneToLayout(node.second, anchorPaneId, newPaneId, axis);
  if (second !== node.second) return { ...node, second };
  return node;
}

/** Insert a pane before or after a leaf, preserving the requested axis. */
export function insertPaneAroundLayout(
  node: LayoutNode,
  anchorPaneId: string,
  newPaneId: string,
  axis: SplitAxis,
  before: boolean
): LayoutNode {
  if (node.kind === "pane") {
    if (node.paneId !== anchorPaneId) return node;
    return {
      kind: "split",
      id: createSplitId("layout"),
      axis,
      ratio: 50,
      first: before ? { kind: "pane", paneId: newPaneId } : node,
      second: before ? node : { kind: "pane", paneId: newPaneId },
    };
  }

  const first = insertPaneAroundLayout(node.first, anchorPaneId, newPaneId, axis, before);
  if (first !== node.first) return { ...node, first };
  const second = insertPaneAroundLayout(node.second, anchorPaneId, newPaneId, axis, before);
  if (second !== node.second) return { ...node, second };
  return node;
}

/** Remove a leaf and collapse its now-unnecessary parent split. */
export function removePaneFromLayout(
  node: LayoutNode,
  paneId: string
): LayoutNode | null {
  if (node.kind === "pane") return node.paneId === paneId ? null : node;

  if (layoutContainsPane(node.first, paneId)) {
    const first = removePaneFromLayout(node.first, paneId);
    return first ? { ...node, first } : node.second;
  }
  if (layoutContainsPane(node.second, paneId)) {
    const second = removePaneFromLayout(node.second, paneId);
    return second ? { ...node, second } : node.first;
  }
  return node;
}

/** Move an existing leaf to a drop position in the same layout tree. */
export function movePaneInLayout(
  node: LayoutNode,
  paneId: string,
  anchorPaneId: string,
  axis: SplitAxis,
  before: boolean
): LayoutNode {
  if (paneId === anchorPaneId) return node;
  const without = removePaneFromLayout(node, paneId);
  if (!without) return node;
  return insertPaneAroundLayout(without, anchorPaneId, paneId, axis, before);
}

export function updateSplitRatio(
  node: LayoutNode,
  splitId: string,
  ratio: number
): LayoutNode {
  if (node.kind === "pane") return node;
  if (node.id === splitId) return { ...node, ratio: clampSplitRatio(ratio) };
  return {
    ...node,
    first: updateSplitRatio(node.first, splitId, ratio),
    second: updateSplitRatio(node.second, splitId, ratio),
  };
}

/* -------------------------------------------------------------------------- */
/* Zoom and minimize                                                           */
/* -------------------------------------------------------------------------- */

/*
 * Both controls are overrides on a group, not edits to its tree. A collapsed
 * pane keeps its place in `root` (and so its React identity, draft and running
 * turn) and is only drawn as a strip. That is what lets "restore" bring the
 * old layout back exactly: nothing about the ratios was ever changed.
 */

type PaneLookup = SplitWorkspaceSnapshot["panes"];

export type PaneOverrides = {
  zoomed: string | null;
  minimized: string[];
};

function paneHasChat(panes: PaneLookup, paneId: string): boolean {
  return Boolean(panes[paneId]?.session);
}

/**
 * The group's overrides made consistent with its tree: ids of closed panes are
 * dropped, an empty chooser pane cannot be minimized, zoom needs a second pane
 * to zoom over, and the last visible pane is never folded away.
 */
export function resolvePaneOverrides(group: SplitGroup, panes: PaneLookup): PaneOverrides {
  const leaves = layoutPaneIds(group.root);
  const zoomed =
    group.zoomedPaneId && leaves.length > 1 && leaves.includes(group.zoomedPaneId)
      ? group.zoomedPaneId
      : null;
  const stored = group.minimizedPaneIds ?? [];
  let minimized = leaves.filter(
    (paneId) => stored.includes(paneId) && paneId !== zoomed && paneHasChat(panes, paneId)
  );
  if (minimized.length >= leaves.length) minimized = [];
  return { zoomed, minimized };
}

/** Panes drawn as strips: everything but the zoomed pane, else the minimized ones. */
export function collapsedPaneIds(group: SplitGroup, panes: PaneLookup): ReadonlySet<string> {
  const { zoomed, minimized } = resolvePaneOverrides(group, panes);
  if (zoomed) return new Set(layoutPaneIds(group.root).filter((paneId) => paneId !== zoomed));
  return new Set(minimized);
}

/**
 * Panes whose transcripts are on screen, in layout order. A chat in a strip is
 * not: its completion reads as unread and its requests may notify.
 */
export function visiblePaneIds(group: SplitGroup, panes: PaneLookup): string[] {
  const collapsed = collapsedPaneIds(group, panes);
  return layoutPaneIds(group.root).filter((paneId) => !collapsed.has(paneId));
}

/** Whether a subtree has no expanded pane left, so it draws only strips. */
export function isNodeCollapsed(node: LayoutNode, collapsed: ReadonlySet<string>): boolean {
  return layoutPaneIds(node).every((paneId) => collapsed.has(paneId));
}

/**
 * Which way a strip runs. Beside an expanded pane it is a thin column; above or
 * below one it is a short row, so it spends its thin side where there is room.
 */
export function stripOrientation(parentAxis: SplitAxis): "vertical" | "horizontal" {
  return parentAxis === "row" ? "vertical" : "horizontal";
}

export function canZoomPane(group: SplitGroup, panes: PaneLookup, paneId: string): boolean {
  const leaves = layoutPaneIds(group.root);
  return leaves.length > 1 && leaves.includes(paneId) && paneHasChat(panes, paneId);
}

/** A pane can be put away while another one stays on screen. */
export function canMinimizePane(group: SplitGroup, panes: PaneLookup, paneId: string): boolean {
  if (!paneHasChat(panes, paneId)) return false;
  const visible = visiblePaneIds(group, panes);
  return visible.includes(paneId) && visible.length > 1;
}

/**
 * `focusedPaneId` must not rest on a strip: "Search chats" and the pane
 * shortcuts would act on a pane you cannot see. Same object when it already
 * rests on a visible pane.
 */
export function ensureVisibleFocus(group: SplitGroup, panes: PaneLookup): SplitGroup {
  const visible = visiblePaneIds(group, panes);
  if (visible.length === 0 || visible.includes(group.focusedPaneId)) return group;
  return { ...group, focusedPaneId: visible[0] };
}

function withoutValue(list: string[] | undefined, value: string): string[] {
  return (list ?? []).filter((item) => item !== value);
}

/** Zoom a pane, or give the layout back when it is already zoomed. */
export function toggleZoomPane(group: SplitGroup, panes: PaneLookup, paneId: string): SplitGroup {
  if (resolvePaneOverrides(group, panes).zoomed === paneId) {
    return { ...group, zoomedPaneId: undefined };
  }
  if (!canZoomPane(group, panes, paneId)) return group;
  return {
    ...group,
    zoomedPaneId: paneId,
    // Zooming a pane you had put away is asking to see it; it stays up once zoom ends.
    minimizedPaneIds: withoutValue(group.minimizedPaneIds, paneId),
    focusedPaneId: paneId,
  };
}

/** Put a pane away as a strip, or bring a stripped pane back. */
export function toggleMinimizePane(group: SplitGroup, panes: PaneLookup, paneId: string): SplitGroup {
  if (collapsedPaneIds(group, panes).has(paneId)) return revealPane(group, panes, paneId);
  if (!canMinimizePane(group, panes, paneId)) return group;
  const minimizedPaneIds = [...resolvePaneOverrides(group, panes).minimized, paneId];
  return ensureVisibleFocus({ ...group, minimizedPaneIds }, panes);
}

/**
 * Make a pane visible again: a strip was clicked, or something (the sidebar, an
 * alert's Open) is taking the user to it. Zoom ends because it hid the pane,
 * and the pane leaves the minimized set. Other minimized panes stay put.
 */
export function revealPane(group: SplitGroup, panes: PaneLookup, paneId: string): SplitGroup {
  if (!layoutContainsPane(group.root, paneId)) return group;
  const keepZoom = resolvePaneOverrides(group, panes).zoomed === paneId;
  return {
    ...group,
    zoomedPaneId: keepZoom ? paneId : undefined,
    minimizedPaneIds: withoutValue(group.minimizedPaneIds, paneId),
    focusedPaneId: paneId,
  };
}

/** Clear zoom and every minimized pane. */
export function restoreAllPanes(group: SplitGroup): SplitGroup {
  if (group.zoomedPaneId === undefined && !group.minimizedPaneIds?.length) return group;
  return { ...group, zoomedPaneId: undefined, minimizedPaneIds: [] };
}

/**
 * Bring minimized panes back without moving focus, for a chat that now waits on
 * an approval or an answer. Zoom is left alone: its strips show the same
 * attention state, and the user chose to look at one pane.
 */
export function wakeMinimizedPanes(
  group: SplitGroup,
  panes: PaneLookup,
  shouldWake: (paneId: string) => boolean
): SplitGroup {
  const { zoomed, minimized } = resolvePaneOverrides(group, panes);
  if (zoomed) return group;
  const keep = minimized.filter((paneId) => !shouldWake(paneId));
  if (keep.length === minimized.length) return group;
  return { ...group, minimizedPaneIds: keep };
}

/** Store only what still applies after a pane closed or lost its chat. Same object when nothing changed. */
export function pruneGroupOverrides(group: SplitGroup, panes: PaneLookup): SplitGroup {
  const { zoomed, minimized } = resolvePaneOverrides(group, panes);
  const stored = group.minimizedPaneIds ?? [];
  const unchanged =
    (group.zoomedPaneId ?? null) === zoomed &&
    stored.length === minimized.length &&
    stored.every((paneId, index) => paneId === minimized[index]);
  const pruned = unchanged
    ? group
    : { ...group, zoomedPaneId: zoomed ?? undefined, minimizedPaneIds: minimized };
  return ensureVisibleFocus(pruned, panes);
}

export function splitSidebarGroups(
  snapshot: SplitWorkspaceSnapshot | null,
  activeGroupId?: string | null
): SplitSidebarGroup[] {
  if (!snapshot) return [];
  const active = activeGroupId === undefined ? snapshot.activeGroupId : activeGroupId;
  return snapshot.groups.map((group) => ({
    id: group.id,
    label: group.label,
    active: group.id === active,
    panes: layoutPaneIds(group.root).flatMap((paneId) => {
      const pane = snapshot.panes[paneId];
      if (!pane?.session) return [];
      const projectLabel = pane.session.isFreeChat
        ? "No workspace"
        : pane.session.root.split(/[\\/]/).filter(Boolean).at(-1) ?? pane.session.root;
      return [
        {
          paneId,
          threadId: pane.session.threadId,
          title: pane.title?.trim() || pane.session.label || "Untitled chat",
          projectLabel,
        },
      ];
    }),
  }));
}
