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
