import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties, type DragEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import {
  ArrowUpIcon,
  ExpandIcon,
  GripVerticalIcon,
  GitForkIcon,
  Maximize2Icon,
  MinusIcon,
  PlusIcon,
  ShrinkIcon,
  SquareIcon,
  XIcon,
} from "lucide-react";

import { Markdown } from "@/components/Markdown";
import { ModelEffortPicker } from "@/components/ModelEffortPicker";
import { PlanningQuestionnaire } from "@/components/PlanningQuestionnaire";
import { AssistantTrace } from "@/components/AssistantTrace";
import { ToolCallRow } from "@/components/ToolCallRow";
import { CommandPalette, type PaletteAction } from "@/components/CommandPalette";
import { readSidebarOpen, writeSidebarOpen } from "@/components/ChatHistorySidebar";
import { SplitPaneStrip } from "@/components/SplitPaneStrip";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { loadDraft } from "@/lib/drafts";
import { formatChord } from "@/lib/keybindings";
import { DEFAULT_EFFORT, type EffortId } from "@/lib/models";
import { effortFromSession } from "@/lib/sessionOptions";
import { splitPaneStatus, type PaneStatus } from "@/lib/splitPaneStatus";
import { needsInput, type ThreadActivityMap } from "@/lib/threadActivity";
import { getUnreadThreads, subscribeUnreadThreads } from "@/lib/threadUnread";
import { useBindings, useKeybindings } from "@/lib/useKeybindings";
import type { ChatUiState } from "@/lib/chatReducer";
import {
  addPaneToLayout,
  appendSplitGroup,
  canMinimizePane,
  canZoomPane,
  clampSplitRatio,
  collapsedPaneIds,
  createSplitId,
  createSplitWorkspaceSnapshot,
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
  targetForSession,
  toggleMinimizePane,
  toggleZoomPane,
  updateSplitRatio,
  wakeMinimizedPanes,
  type LayoutNode,
  type SplitAxis,
  type SplitChatTarget,
  type SplitGroup,
  type SplitPaneSnapshot,
  type SplitSidebarGroup,
  type SplitWorkspaceSnapshot,
} from "@/lib/splitLayout";
import type { ApprovalChoice, ProjectChats, ProviderRow, SessionInfo } from "@/lib/types";

export type { SplitChatTarget, SplitGroup, SplitPaneSnapshot, SplitWorkspaceSnapshot } from "@/lib/splitLayout";

export type SplitPaneOptions = {
  model?: string;
  effort?: EffortId;
  providerId?: string;
  reset?: boolean;
};

export type SplitSidebarRenderModel = {
  open: boolean;
  activeThreadId: string;
  activeProjectPath: string | null;
  activeProviderId: string;
  groups: SplitSidebarGroup[];
  activeGroupId: string;
  onOpenChange: (open: boolean) => void;
  onSearch: () => void;
  onOpenSplitGroup: (groupId: string) => void;
  onFocusSplitPane: (groupId: string, paneId: string) => void;
};

type Props = {
  initial: SessionInfo;
  initialDraft: string;
  states: ReadonlyMap<string, ChatUiState>;
  snapshot?: SplitWorkspaceSnapshot | null;
  startNewGroup?: boolean;
  onOpen: (target: SplitChatTarget, fork?: boolean) => Promise<SessionInfo>;
  onSend: (
    session: SessionInfo,
    target: SplitChatTarget,
    text: string
  ) => Promise<SessionInfo | void>;
  providers: ProviderRow[];
  chatListRevision?: number;
  deletedThreadId?: string | null;
  onUpdateOptions: (
    session: SessionInfo,
    target: SplitChatTarget,
    options: SplitPaneOptions
  ) => Promise<SessionInfo | void>;
  onClose: (session: SessionInfo, draft: string, target: SplitChatTarget) => Promise<void>;
  onStateChange?: (snapshot: SplitWorkspaceSnapshot, choosingPaneId: string | null) => void;
  onResolveApproval: (approvalId: string, decision: ApprovalChoice, threadId: string) => Promise<void>;
  onResolveQuestion: (questionId: string, answer: string, threadId: string) => Promise<void>;
  /** What every chat is doing, so a strip can say "working" or "awaiting approval". */
  threadActivity?: ThreadActivityMap;
  renderSidebar?: (model: SplitSidebarRenderModel) => ReactNode;
};

function readSplitDraft(id: string) {
  try {
    return localStorage.getItem(`zest.splitDraft.${id}`) ?? loadDraft(id);
  } catch {
    return loadDraft(id);
  }
}

function sessionTarget(session: SessionInfo): SplitChatTarget {
  return targetForSession(session);
}

function initialWorkspace(
  snapshot: SplitWorkspaceSnapshot | null | undefined,
  initial: SessionInfo,
  initialDraft: string,
  startNewGroup: boolean
) {
  if (startNewGroup) return appendSplitGroup(snapshot ?? null, initial, initialDraft);
  return snapshot ?? createSplitWorkspaceSnapshot(initial, initialDraft);
}

function paneLabel(path: string[]): string {
  if (path.length === 1) return path[0] === "first" ? "Left chat" : "Right chat";
  const side = path[0] === "first" ? "Left" : "Right";
  const position = path[path.length - 1] === "first" ? "top" : "bottom";
  return `${side} ${position} chat`;
}

function titleForPane(pane: SplitPaneSnapshot, threads: ReadonlyMap<string, string>) {
  if (!pane.session) return "Choose a chat";
  return pane.title?.trim() || threads.get(pane.session.threadId) || pane.session.label || "Untitled chat";
}

function projectForPane(pane: SplitPaneSnapshot) {
  if (!pane.session) return "Open a conversation alongside this one";
  return pane.session.isFreeChat
    ? `No workspace · ${pane.session.label}`
    : `${pane.session.root.split(/[\\/]/).filter(Boolean).at(-1) ?? pane.session.root} · ${pane.session.label}`;
}

export function SplitWorkspace({
  initial,
  initialDraft,
  states,
  snapshot,
  startNewGroup = false,
  onOpen,
  onSend,
  providers,
  chatListRevision = 0,
  deletedThreadId = null,
  onUpdateOptions,
  onClose,
  onStateChange,
  onResolveApproval,
  onResolveQuestion,
  threadActivity,
  renderSidebar,
}: Props) {
  const initialWorkspaceRef = useRef<SplitWorkspaceSnapshot | null>(null);
  if (!initialWorkspaceRef.current) {
    initialWorkspaceRef.current = initialWorkspace(snapshot, initial, initialDraft, startNewGroup);
  }
  const initialState = initialWorkspaceRef.current;
  const receivedSnapshotRef = useRef(snapshot);
  const [workspace, setWorkspace] = useState(initialState);
  const [projects, setProjects] = useState<ProjectChats[]>([]);
  const [choosing, setChoosing] = useState<string | null>(() => {
    const group = initialState.groups.find((candidate) => candidate.id === initialState.activeGroupId);
    return group
      ? layoutPaneIds(group.root).find((paneId) => !initialState.panes[paneId]?.session) ?? null
      : null;
  });
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [sidebarOpen, setSidebarOpen] = useState(readSidebarOpen);
  const [draggedPaneId, setDraggedPaneId] = useState<string | null>(null);
  const [dropTargetPaneId, setDropTargetPaneId] = useState<string | null>(null);
  const pointerDragRef = useRef<{
    paneId: string;
    pointerId: number;
    startX: number;
    startY: number;
    started: boolean;
  } | null>(null);

  const activeGroup =
    workspace.groups.find((group) => group.id === workspace.activeGroupId) ??
    workspace.groups[0];
  const activePane = activeGroup ? workspace.panes[activeGroup.focusedPaneId] : undefined;
  const [bindings] = useBindings();
  const unreadThreads = useSyncExternalStore(subscribeUnreadThreads, getUnreadThreads);
  const [paletteOpen, setPaletteOpen] = useState(false);
  /** The pane a palette command acts on, taken before the palette's input takes focus. */
  const paletteTargetRef = useRef<string | null>(null);
  /** Where focus goes once a pane has been put away or brought back. */
  const pendingFocusRef = useRef<{ paneId: string; target: "strip" | "composer" } | null>(null);
  const collapsedIds = useMemo(
    () => (activeGroup ? collapsedPaneIds(activeGroup, workspace.panes) : new Set<string>()),
    [activeGroup, workspace.panes]
  );
  const zoomedPaneId = activeGroup ? resolvePaneOverrides(activeGroup, workspace.panes).zoomed : null;
  const threadTitles = useMemo(() => {
    const map = new Map<string, string>();
    for (const project of projects) {
      for (const thread of project.threads) {
        if (thread.title?.trim()) map.set(thread.id, thread.title.trim());
      }
    }
    return map;
  }, [projects]);

  useEffect(() => {
    if (snapshot !== receivedSnapshotRef.current) {
      receivedSnapshotRef.current = snapshot;
      if (snapshot && snapshot !== workspace) {
        setWorkspace(snapshot);
        setChoosing(null);
        // Do not echo the old workspace while applying external navigation.
        return;
      }
    }
    onStateChange?.(workspace, choosing);
  }, [choosing, onStateChange, snapshot, workspace]);

  // A minimized pane whose chat now waits on an approval or an answer comes
  // back by itself, without taking focus. Zoom is left alone: its strips show
  // the same state and the user chose to look at one pane.
  useEffect(() => {
    setWorkspace((current) => {
      const group =
        current.groups.find((candidate) => candidate.id === current.activeGroupId) ?? current.groups[0];
      if (!group) return current;
      const next = wakeMinimizedPanes(group, current.panes, (paneId) => {
        const threadId = current.panes[paneId]?.session?.threadId;
        return needsInput(threadId ? threadActivity?.[threadId] : undefined);
      });
      if (next === group) return current;
      return {
        ...current,
        groups: current.groups.map((candidate) => (candidate.id === group.id ? next : candidate)),
      };
    });
  }, [threadActivity, workspace]);

  useEffect(() => {
    const pending = pendingFocusRef.current;
    if (!pending) return;
    pendingFocusRef.current = null;
    const section = document.querySelector<HTMLElement>(
      `[data-split-pane-id="${CSS.escape(pending.paneId)}"]`
    );
    const target =
      pending.target === "strip"
        ? section?.querySelector<HTMLElement>("[data-split-strip]")
        : section?.querySelector<HTMLElement>("textarea");
    target?.focus();
  }, [workspace]);

  useEffect(() => {
    if (!deletedThreadId) return;
    try {
      localStorage.removeItem(`zest.splitDraft.${deletedThreadId}`);
    } catch {
      /* Ignore unavailable storage. */
    }
    setWorkspace((current) => {
      let changed = false;
      const panes = { ...current.panes };
      for (const [paneId, pane] of Object.entries(current.panes)) {
        if (pane.session?.threadId !== deletedThreadId) continue;
        changed = true;
        panes[paneId] = {
          ...pane,
          session: null,
          target: null,
          draft: "",
          title: undefined,
        };
      }
      if (!changed) return current;
      // An emptied pane cannot stay minimized: it has no chat for a strip to describe.
      return {
        ...current,
        panes,
        groups: current.groups.map((group) => pruneGroupOverrides(group, panes)),
      };
    });
  }, [deletedThreadId]);

  useEffect(() => {
    let cancelled = false;
    getBackend()
      .listChatProjects()
      .then((next) => {
        if (!cancelled) setProjects(next);
      })
      .catch(() => {
        if (!cancelled) setError("Could not load chats. Close split view and try again.");
      });
    return () => {
      cancelled = true;
    };
  }, [chatListRevision]);

  function setSidebar(next: boolean) {
    setSidebarOpen(next);
    writeSidebarOpen(next);
  }

  function updatePane(paneId: string, update: (pane: SplitPaneSnapshot) => SplitPaneSnapshot) {
    setWorkspace((current) => ({
      ...current,
      panes: {
        ...current.panes,
        [paneId]: update(current.panes[paneId]),
      },
    }));
  }

  function updateGroup(groupId: string, update: (group: SplitGroup) => SplitGroup) {
    setWorkspace((current) => ({
      ...current,
      groups: current.groups.map((group) => (group.id === groupId ? update(group) : group)),
    }));
  }

  function updateActiveGroup(
    update: (group: SplitGroup, panes: SplitWorkspaceSnapshot["panes"]) => SplitGroup
  ) {
    setWorkspace((current) => {
      const group =
        current.groups.find((candidate) => candidate.id === current.activeGroupId) ?? current.groups[0];
      if (!group) return current;
      const next = update(group, current.panes);
      if (next === group) return current;
      return {
        ...current,
        groups: current.groups.map((candidate) => (candidate.id === group.id ? next : candidate)),
      };
    });
  }

  function paneNeedsInput(pane: SplitPaneSnapshot | undefined) {
    const threadId = pane?.session?.threadId;
    return needsInput(threadId ? threadActivity?.[threadId] : undefined);
  }

  function paneStatusFor(pane: SplitPaneSnapshot): PaneStatus {
    const threadId = pane.session?.threadId;
    return splitPaneStatus({
      hasChat: Boolean(pane.session),
      activity: threadId ? threadActivity?.[threadId] : undefined,
      sending: threadId ? states.get(threadId)?.sending : false,
      unread: threadId ? unreadThreads.has(threadId) : false,
    });
  }

  /** The pane a pane shortcut acts on: the one holding keyboard focus, else the group's focused pane. */
  function paneForShortcut(): string | null {
    if (!activeGroup) return null;
    const active = document.activeElement;
    const fromDom =
      active instanceof HTMLElement
        ? active.closest<HTMLElement>("[data-split-pane-id]")?.dataset.splitPaneId
        : undefined;
    const leaves = layoutPaneIds(activeGroup.root);
    if (fromDom && leaves.includes(fromDom)) return fromDom;
    return leaves.includes(activeGroup.focusedPaneId) ? activeGroup.focusedPaneId : null;
  }

  function zoomPane(paneId: string) {
    updateActiveGroup((group, panes) => toggleZoomPane(group, panes, paneId));
  }

  /** Put a pane away, or bring a stripped pane back. A pane waiting on you is not put away. */
  function minimizePane(paneId: string) {
    const stripped = collapsedIds.has(paneId);
    if (!stripped && paneNeedsInput(workspace.panes[paneId])) return;
    pendingFocusRef.current = { paneId, target: stripped ? "composer" : "strip" };
    updateActiveGroup((group, panes) => toggleMinimizePane(group, panes, paneId));
  }

  /** A strip was clicked: the pane comes back and takes focus. */
  function restoreStrip(paneId: string) {
    pendingFocusRef.current = { paneId, target: "composer" };
    updateActiveGroup((group, panes) => revealPane(group, panes, paneId));
  }

  function restoreAllPaneStrips() {
    updateActiveGroup((group) => restoreAllPanes(group));
  }

  function openPalette() {
    paletteTargetRef.current = paneForShortcut();
    setPaletteOpen(true);
  }

  useKeybindings({
    "view.palette": openPalette,
    "split.zoom": () => {
      const paneId = paneForShortcut();
      if (paneId) zoomPane(paneId);
    },
    "split.minimize": () => {
      const paneId = paneForShortcut();
      if (paneId) minimizePane(paneId);
    },
    "split.restore": restoreAllPaneStrips,
  });

  const shortcutFor = (id: "split.zoom" | "split.minimize" | "split.restore") => {
    const chord = bindings[id];
    return chord ? formatChord(chord) : undefined;
  };

  const paletteActions: PaletteAction[] = [
    {
      id: "zoom-pane",
      label: "Zoom pane",
      description: "Show the focused split pane full size, or bring the others back",
      shortcut: shortcutFor("split.zoom"),
      run: () => {
        const paneId = paletteTargetRef.current ?? paneForShortcut();
        if (paneId) zoomPane(paneId);
      },
    },
    {
      id: "minimize-pane",
      label: "Minimize pane",
      description: "Put the focused split pane away as a strip while its turn keeps running",
      shortcut: shortcutFor("split.minimize"),
      run: () => {
        const paneId = paletteTargetRef.current ?? paneForShortcut();
        if (paneId) minimizePane(paneId);
      },
    },
    {
      id: "restore-panes",
      label: "Restore all panes",
      description: "End zoom and bring back every minimized pane",
      shortcut: shortcutFor("split.restore"),
      run: restoreAllPaneStrips,
    },
  ];

  function updatePaneOptions(paneId: string, options: SplitPaneOptions) {
    const pane = workspace.panes[paneId];
    if (!pane?.session || !pane.target) return;
    void run(async () => {
      const next = await onUpdateOptions(pane.session!, pane.target!, options);
      if (!next) return;
      updatePane(paneId, (current) => ({
        ...current,
        session: next,
        target: sessionTarget(next),
      }));
    });
  }

  function updateDraft(paneId: string, value: string) {
    updatePane(paneId, (pane) => ({ ...pane, draft: value }));
    const threadId = workspace.panes[paneId]?.session?.threadId;
    if (!threadId) return;
    try {
      localStorage.setItem(`zest.splitDraft.${threadId}`, value);
    } catch {
      /* Draft remains in the workspace snapshot. */
    }
  }

  function focusPane(groupId: string, paneId: string) {
    setWorkspace((current) => ({
      ...current,
      activeGroupId: groupId,
      // Going to a pane means seeing it, so it leaves a strip.
      groups: current.groups.map((group) =>
        group.id === groupId ? revealPane(group, current.panes, paneId) : group
      ),
    }));
    setChoosing(workspace.panes[paneId]?.session ? null : paneId);
  }

  function activateGroup(groupId: string) {
    const group = workspace.groups.find((candidate) => candidate.id === groupId);
    if (!group) return;
    const focused = workspace.panes[group.focusedPaneId];
    const fallback = focused?.session
      ? group.focusedPaneId
      : layoutPaneIds(group.root).find((paneId) => workspace.panes[paneId]?.session) ?? group.focusedPaneId;
    setWorkspace((current) => ({
      ...current,
      activeGroupId: groupId,
      groups: current.groups.map((candidate) =>
        candidate.id === groupId ? { ...candidate, focusedPaneId: fallback } : candidate
      ),
    }));
    setChoosing(workspace.panes[fallback]?.session ? null : fallback);
  }

  async function run(action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }

  function addEmptyPane(groupId: string, anchorPaneId: string, axis: SplitAxis): string | null {
    const paneId = createSplitId("pane");
    const group = workspace.groups.find((candidate) => candidate.id === groupId);
    if (!group || !workspace.panes[anchorPaneId]) return null;
    const root = addPaneToLayout(group.root, anchorPaneId, paneId, axis);
    if (root === group.root) return null;
    setWorkspace((current) => {
      const currentGroup = current.groups.find((candidate) => candidate.id === groupId);
      if (!currentGroup || !current.panes[anchorPaneId]) return current;
      const currentRoot = addPaneToLayout(currentGroup.root, anchorPaneId, paneId, axis);
      if (currentRoot === currentGroup.root) return current;
      return {
        ...current,
        panes: {
          ...current.panes,
          [paneId]: { paneId, session: null, target: null, draft: "" },
        },
        groups: current.groups.map((candidate) =>
          candidate.id === groupId
            // A new pane would be born hidden behind zoom or a minimized
            // sibling, so adding one brings every pane back.
            ? restoreAllPanes({ ...candidate, root: currentRoot, focusedPaneId: paneId })
            : candidate
        ),
      };
    });
    setChoosing(paneId);
    return paneId;
  }

  function emptyPaneInGroup(group: SplitGroup) {
    return layoutPaneIds(group.root).find((paneId) => !workspace.panes[paneId]?.session);
  }

  function addPane(groupId: string, anchorPaneId: string, axis: SplitAxis = "column") {
    const group = workspace.groups.find((candidate) => candidate.id === groupId);
    if (!group) return null;
    const empty = emptyPaneInGroup(group);
    if (empty) {
      // The empty pane may be a strip; its chooser has to be on screen.
      updateGroup(groupId, restoreAllPanes);
      setChoosing(empty);
      return empty;
    }
    return addEmptyPane(groupId, anchorPaneId, axis);
  }

  async function choose(paneId: string, target: SplitChatTarget, fork = false) {
    await run(async () => {
      const info = await onOpen(target, fork);
      const paneTarget = target.newThread && !fork
        ? { root: target.root, newThread: true, providerId: target.providerId }
        : sessionTarget(info);
      const selectedTitle = target.threadId ? threadTitles.get(target.threadId) : undefined;
      updatePane(paneId, (pane) => ({
        ...pane,
        session: info,
        target: paneTarget,
        draft: pane.draft || (info.threadId ? readSplitDraft(info.threadId) : ""),
        title: selectedTitle ?? pane.title,
      }));
      const groupId = workspace.activeGroupId;
      if (groupId) updateGroup(groupId, (group) => ({ ...group, focusedPaneId: paneId }));
      setChoosing(null);
      setQuery("");
    });
  }

  function closePane(groupId: string, paneId: string) {
    const group = workspace.groups.find((candidate) => candidate.id === groupId);
    const pane = workspace.panes[paneId];
    if (!group || !pane) return;
    const leaves = layoutPaneIds(group.root);
    if (leaves.length <= 1) {
      if (pane.session && pane.target) {
        void run(() => onClose(pane.session!, pane.draft, pane.target!));
      }
      return;
    }
    const root = removePaneFromLayout(group.root, paneId);
    if (!root) return;
    const remaining = layoutPaneIds(root);
    const hasPopulatedPane = remaining.some((candidate) => Boolean(workspace.panes[candidate]?.session));
    if (!hasPopulatedPane) {
      const otherGroups = workspace.groups.filter((candidate) => candidate.id !== groupId);
      if (otherGroups.length === 0) {
        if (pane.session && pane.target) {
          void run(() => onClose(pane.session!, pane.draft, pane.target!));
        } else {
          // A pane can be invalidated by deleting its chat. If that was the
          // last usable chat in the only split group, close the split shell
          // against the app's current session instead of leaving an empty,
          // uncloseable layout behind.
          void run(() => onClose(initial, initialDraft, sessionTarget(initial)));
        }
        return;
      }
      const removedPaneIds = new Set(layoutPaneIds(group.root));
      setWorkspace((current) => {
        const panes = Object.fromEntries(
          Object.entries(current.panes).filter(([id]) => !removedPaneIds.has(id))
        );
        return {
          ...current,
          panes,
          groups: current.groups.filter((candidate) => candidate.id !== groupId),
          activeGroupId: otherGroups[0].id,
        };
      });
      setChoosing(null);
      return;
    }
    const nextFocus = remaining.find((candidate) => workspace.panes[candidate]?.session) ?? remaining[0];
    setWorkspace((current) => {
      const panes = { ...current.panes };
      delete panes[paneId];
      return {
        ...current,
        panes,
        groups: current.groups.map((candidate) =>
          candidate.id === groupId
            // A closed pane leaves zoom and the minimized set; if it was the
            // last one on screen, the strips come back with it.
            ? pruneGroupOverrides({ ...candidate, root, focusedPaneId: nextFocus }, panes)
            : candidate
        ),
      };
    });
    setChoosing(workspace.panes[nextFocus]?.session ? null : nextFocus);
  }

  function continueSingle(pane: SplitPaneSnapshot) {
    if (!pane.session || !pane.target) return;
    void run(() => onClose(pane.session!, pane.draft, pane.target!));
  }

  function dropEdge(rect: DOMRect, clientX: number, clientY: number) {
    const distances = [
      { distance: clientX - rect.left, axis: "row" as const, before: true },
      { distance: rect.right - clientX, axis: "row" as const, before: false },
      { distance: clientY - rect.top, axis: "column" as const, before: true },
      { distance: rect.bottom - clientY, axis: "column" as const, before: false },
    ];
    return distances.reduce((closest, candidate) =>
      candidate.distance < closest.distance ? candidate : closest
    );
  }

  function movePaneAt(
    sourcePaneId: string,
    anchorPaneId: string,
    clientX: number,
    clientY: number,
    rect: DOMRect
  ) {
    if (sourcePaneId === anchorPaneId) return;
    const edge = dropEdge(rect, clientX, clientY);
    setWorkspace((current) => ({
      ...current,
      groups: current.groups.map((group) => {
        if (group.id !== current.activeGroupId || !layoutPaneIds(group.root).includes(sourcePaneId)) return group;
        return {
          ...group,
          root: movePaneInLayout(group.root, sourcePaneId, anchorPaneId, edge.axis, edge.before),
          focusedPaneId: sourcePaneId,
        };
      }),
    }));
  }

  function dropPane(anchorPaneId: string, event: DragEvent<HTMLElement>) {
    event.preventDefault();
    const sourcePaneId = event.dataTransfer.getData("text/plain") || draggedPaneId;
    if (sourcePaneId) {
      movePaneAt(
        sourcePaneId,
        anchorPaneId,
        event.clientX,
        event.clientY,
        event.currentTarget.getBoundingClientRect()
      );
    }
    setDraggedPaneId(null);
    setDropTargetPaneId(null);
  }

  function paneElementAt(clientX: number, clientY: number) {
    const element = document.elementFromPoint(clientX, clientY);
    return element instanceof HTMLElement
      ? element.closest<HTMLElement>("[data-split-pane-id]")
      : null;
  }

  function beginPointerDrag(
    paneId: string,
    event: ReactPointerEvent<HTMLButtonElement>
  ) {
    if (busy || event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    pointerDragRef.current = {
      paneId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      started: false,
    };
  }

  function updatePointerDrag(event: ReactPointerEvent<HTMLButtonElement>) {
    const drag = pointerDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const distance = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
    if (!drag.started && distance < 6) return;
    if (!drag.started) {
      drag.started = true;
      setDraggedPaneId(drag.paneId);
    }
    event.preventDefault();
    setDropTargetPaneId(paneElementAt(event.clientX, event.clientY)?.dataset.splitPaneId ?? null);
  }

  function finishPointerDrag(event: ReactPointerEvent<HTMLButtonElement>) {
    const drag = pointerDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (drag.started) {
      const target = paneElementAt(event.clientX, event.clientY);
      const anchorPaneId = target?.dataset.splitPaneId;
      if (anchorPaneId) {
        movePaneAt(
          drag.paneId,
          anchorPaneId,
          event.clientX,
          event.clientY,
          target.getBoundingClientRect()
        );
      }
    }
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    pointerDragRef.current = null;
    setDraggedPaneId(null);
    setDropTargetPaneId(null);
  }

  const sidebar = renderSidebar
    ? renderSidebar({
        open: sidebarOpen,
        activeThreadId: activePane?.session?.threadId ?? initial.threadId,
        activeProjectPath: activePane?.session?.isFreeChat
          ? null
          : activePane?.session?.root ?? null,
        activeProviderId: activePane?.session?.provider ?? initial.provider,
        groups: splitSidebarGroups(workspace, workspace.activeGroupId),
        activeGroupId: workspace.activeGroupId,
        onOpenChange: setSidebar,
        onSearch: () => {
          setChoosing(activeGroup.focusedPaneId);
          setQuery("");
        },
        onOpenSplitGroup: activateGroup,
        onFocusSplitPane: focusPane,
      })
    : null;

  if (!activeGroup) return null;

  /**
   * Draw the tree. Zoom and minimize only change how a pane or subtree is
   * drawn (`collapsedIds`): every pane keeps its place in the tree, so it stays
   * mounted and its draft and running turn carry on behind its strip.
   * `strip` is the direction a strip runs if this node turns out to be collapsed.
   */
  const renderNode = (
    node: LayoutNode,
    path: string[],
    strip: "vertical" | "horizontal"
  ): ReactNode => {
    if (node.kind === "pane") {
      const pane = workspace.panes[node.paneId];
      if (!pane) return null;
      const collapsed = collapsedIds.has(node.paneId);
      const title = titleForPane(pane, threadTitles);
      const needsAnswer = paneNeedsInput(pane);
      const zoomed = zoomedPaneId === node.paneId;
      return (
        <SplitPaneView
          key={node.paneId}
          pane={pane}
          label={paneLabel(path)}
          title={title}
          projectLabel={projectForPane(pane)}
          busy={busy}
          states={states}
          collapsed={collapsed}
          strip={strip}
          status={paneStatusFor(pane)}
          zoomed={zoomed}
          canZoom={canZoomPane(activeGroup, workspace.panes, node.paneId)}
          minimizeBlockedReason={
            canMinimizePane(activeGroup, workspace.panes, node.paneId)
              ? needsAnswer
                ? "Answer the pending request before minimizing this pane"
                : undefined
              : "Keep at least one pane visible"
          }
          zoomShortcut={shortcutFor("split.zoom")}
          minimizeShortcut={shortcutFor("split.minimize")}
          onZoom={() => zoomPane(node.paneId)}
          onMinimize={() => minimizePane(node.paneId)}
          onRestore={() => restoreStrip(node.paneId)}
          onChoose={() => {
            setChoosing(node.paneId);
            setQuery("");
          }}
          onAddPane={() => {
            if (pane.session) addPane(activeGroup.id, node.paneId);
          }}
          onFork={() => {
            if (!pane.session) return;
            const empty = emptyPaneInGroup(activeGroup);
            if (empty && empty !== node.paneId) {
              updateGroup(activeGroup.id, restoreAllPanes);
              void choose(empty, sessionTarget(pane.session), true);
            } else {
              const created = addEmptyPane(activeGroup.id, node.paneId, "column");
              if (created) void choose(created, sessionTarget(pane.session), true);
            }
          }}
          onContinue={() => continueSingle(pane)}
          onClose={() => closePane(activeGroup.id, node.paneId)}
          choosing={choosing === node.paneId || !pane.session}
          query={query}
          setQuery={setQuery}
          projects={projects}
          currentThreadIds={new Set(layoutPaneIds(activeGroup.root).flatMap((paneId) => {
            const session = workspace.panes[paneId]?.session;
            return session ? [session.threadId] : [];
          }))}
          onCancelChoose={() => setChoosing(null)}
          onChooseTarget={(target) => void choose(node.paneId, target)}
          onNewChat={() => {
            void choose(node.paneId, {
              root: initial.isFreeChat ? null : initial.root,
              newThread: true,
              providerId: initial.provider,
            });
          }}
          draft={pane.draft}
          onDraft={(value) => updateDraft(node.paneId, value)}
          onSend={() => {
            if (!pane.session || !pane.target) return Promise.resolve();
            return run(async () => {
              const text = workspace.panes[node.paneId]?.draft.trim();
              if (!text) return;
              const next = await onSend(pane.session!, pane.target!, text);
              const nextSession = next ?? pane.session!;
              updatePane(node.paneId, (current) => ({
                ...current,
                session: nextSession,
                target: sessionTarget(nextSession),
                draft: "",
              }));
            });
          }}
          onStop={() => {
            const threadId = pane.session?.threadId;
            if (threadId) void run(() => getBackend().cancelTurn(threadId));
          }}
          onError={setError}
          onResolveApproval={onResolveApproval}
          onResolveQuestion={onResolveQuestion}
          providers={providers}
          onUpdateOptions={(options) => updatePaneOptions(node.paneId, options)}
          onResizeFocus={() => focusPane(activeGroup.id, node.paneId)}
          dragging={draggedPaneId === node.paneId}
          canDrop={draggedPaneId !== null && draggedPaneId !== node.paneId && !collapsed}
          onDragStart={() => setDraggedPaneId(node.paneId)}
          onDragEnd={() => setDraggedPaneId(null)}
          onDrop={(event) => dropPane(node.paneId, event)}
          onPointerDown={(event) => beginPointerDrag(node.paneId, event)}
          onPointerMove={updatePointerDrag}
          onPointerUp={finishPointerDrag}
          onPointerCancel={finishPointerDrag}
          dropTarget={dropTargetPaneId === node.paneId}
        />
      );
    }

    const ratio = clampSplitRatio(node.ratio);
    const firstCollapsed = isNodeCollapsed(node.first, collapsedIds);
    const secondCollapsed = isNodeCollapsed(node.second, collapsedIds);
    const sharesSpace = !firstCollapsed && !secondCollapsed;
    // A collapsed subtree is a run of strips, whatever its own axis was. The
    // ratio is only used while both sides are expanded, so it is never edited.
    const childStrip = firstCollapsed && secondCollapsed ? strip : stripOrientation(node.axis);
    const direction = firstCollapsed && secondCollapsed
      ? strip === "vertical" ? "flex-row" : "flex-col"
      : node.axis === "row" ? "flex-row" : "flex-col";
    const separatorClass = node.axis === "row"
      ? "h-full w-2 cursor-col-resize"
      : "h-2 w-full cursor-row-resize";
    const firstStyle: CSSProperties = {
      flex: sharesSpace ? `0 0 calc(${ratio}% - 4px)` : firstCollapsed ? "0 0 auto" : "1 1 0%",
    };
    const secondStyle: CSSProperties = { flex: secondCollapsed ? "0 0 auto" : "1 1 0%" };

    return (
      <div key={node.id} className={`zest-split-node flex min-h-0 min-w-0 flex-1 ${direction}`}>
        <div
          className={`flex min-h-0 min-w-0 flex-col ${firstCollapsed ? "zest-split-collapsed" : ""}`}
          style={firstStyle}
        >
          {renderNode(node.first, [...path, "first"], childStrip)}
        </div>
        {sharesSpace ? (
          <SplitDivider
            axis={node.axis}
            ratio={ratio}
            className={separatorClass}
            onChange={(next) => {
              setWorkspace((current) => ({
                ...current,
                groups: current.groups.map((group) =>
                  group.id === activeGroup.id
                    ? { ...group, root: updateSplitRatio(group.root, node.id, next) }
                    : group
                ),
              }));
            }}
          />
        ) : null}
        <div
          className={`flex min-h-0 min-w-0 flex-col ${secondCollapsed ? "zest-split-collapsed" : ""}`}
          style={secondStyle}
        >
          {renderNode(node.second, [...path, "second"], childStrip)}
        </div>
      </div>
    );
  };

  return (
    <section aria-label="Split workspace" className="relative flex min-h-0 min-w-0 flex-1 bg-[var(--canvas-solid)]">
      {sidebar}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {error ? (
          <p role="alert" className="shrink-0 border-b border-border px-4 py-2 text-sm text-destructive">
            {error}
          </p>
        ) : null}
        <div className="zest-split-panes flex min-h-0 flex-1">
          {renderNode(activeGroup.root, [], "vertical")}
        </div>
      </div>
      {paletteOpen ? (
        <CommandPalette
          open
          scope="actions"
          actions={paletteActions}
          onClose={() => setPaletteOpen(false)}
          onCommand={() => setPaletteOpen(false)}
        />
      ) : null}
    </section>
  );
}

function SplitDivider({
  axis,
  ratio,
  className,
  onChange,
}: {
  axis: SplitAxis;
  ratio: number;
  className: string;
  onChange: (ratio: number) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const orientation = axis === "row" ? "vertical" : "horizontal";

  return (
    <div
      ref={container}
      role="separator"
      aria-label="Resize split panes"
      aria-orientation={orientation}
      aria-valuemin={30}
      aria-valuemax={70}
      aria-valuenow={ratio}
      tabIndex={0}
      className={`zest-split-divider shrink-0 touch-none bg-border/40 hover:bg-primary/40 focus-visible:bg-primary/40 focus-visible:outline-none ${className}`}
      onDoubleClick={() => onChange(50)}
      onKeyDown={(event) => {
        if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const decrease = axis === "row"
          ? event.key === "ArrowLeft"
          : event.key === "ArrowUp";
        onChange(event.key === "Home" ? 30 : event.key === "End" ? 70 : ratio + (decrease ? -2 : 2));
      }}
      onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
      onPointerMove={(event) => {
        if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
        const rect = container.current?.parentElement?.getBoundingClientRect();
        if (!rect) return;
        const value = axis === "row"
          ? ((event.clientX - rect.left) / rect.width) * 100
          : ((event.clientY - rect.top) / rect.height) * 100;
        onChange(value);
      }}
      onPointerUp={(event) => {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId);
        }
      }}
    />
  );
}

function SplitPaneView({
  pane,
  label,
  title,
  projectLabel,
  busy,
  states,
  collapsed,
  strip,
  status,
  zoomed,
  canZoom,
  minimizeBlockedReason,
  zoomShortcut,
  minimizeShortcut,
  onZoom,
  onMinimize,
  onRestore,
  onChoose,
  onAddPane,
  onFork,
  onContinue,
  onClose,
  choosing,
  query,
  setQuery,
  projects,
  currentThreadIds,
  onCancelChoose,
  onChooseTarget,
  onNewChat,
  draft,
  onDraft,
  onSend,
  onStop,
  onError,
  onResolveApproval,
  onResolveQuestion,
  providers,
  onUpdateOptions,
  onResizeFocus,
  dragging,
  canDrop,
  onDragStart,
  onDragEnd,
  onDrop,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
  dropTarget,
}: {
  pane: SplitPaneSnapshot;
  label: string;
  title: string;
  projectLabel: string;
  busy: boolean;
  states: ReadonlyMap<string, ChatUiState>;
  /** Drawn as a strip. The content below stays mounted, only hidden. */
  collapsed: boolean;
  strip: "vertical" | "horizontal";
  status: PaneStatus;
  zoomed: boolean;
  canZoom: boolean;
  /** Why this pane cannot be minimized now, or undefined when it can. */
  minimizeBlockedReason?: string;
  zoomShortcut?: string;
  minimizeShortcut?: string;
  onZoom: () => void;
  onMinimize: () => void;
  onRestore: () => void;
  onChoose: () => void;
  onAddPane: () => void;
  onFork: () => void;
  onContinue: () => void;
  onClose: () => void;
  choosing: boolean;
  query: string;
  setQuery: (value: string) => void;
  projects: ProjectChats[];
  currentThreadIds: Set<string>;
  onCancelChoose: () => void;
  onChooseTarget: (target: SplitChatTarget) => void;
  onNewChat: () => void;
  draft: string;
  onDraft: (value: string) => void;
  onSend: () => Promise<void>;
  onStop: () => void;
  onError: (message: string) => void;
  onResolveApproval: Props["onResolveApproval"];
  onResolveQuestion: Props["onResolveQuestion"];
  providers: ProviderRow[];
  onUpdateOptions: (options: SplitPaneOptions) => void;
  onResizeFocus: () => void;
  dragging: boolean;
  canDrop: boolean;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDrop: (event: DragEvent<HTMLElement>) => void;
  onPointerDown: (event: ReactPointerEvent<HTMLButtonElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLButtonElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLButtonElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<HTMLButtonElement>) => void;
  dropTarget: boolean;
}) {
  const bodyId = `split-pane-body-${pane.paneId}`;
  return (
    <section
      aria-label={label}
      data-split-pane-id={pane.paneId}
      data-collapsed={collapsed ? strip : undefined}
      className={`zest-split-pane flex min-h-0 min-w-0 flex-col ${dragging ? "opacity-60" : ""} ${dropTarget ? "ring-2 ring-inset ring-primary" : canDrop ? "ring-1 ring-inset ring-primary/40" : ""}`}
      onDragOver={(event) => {
        if (!canDrop) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
      }}
      onDrop={onDrop}
    >
      {collapsed ? (
        <SplitPaneStrip
          paneId={pane.paneId}
          title={title}
          status={status}
          orientation={strip}
          controlsId={bodyId}
          onRestore={onRestore}
        />
      ) : null}
      {/* Hidden, not unmounted, behind a strip: the transcript scroll, composer and
          pickers keep their state, and the turn keeps streaming into them. */}
      <div id={bodyId} className={collapsed ? "hidden" : "contents"}>
      <header className="flex min-w-0 shrink-0 items-center gap-1 border-b border-border/60 px-1.5 py-1">
        <button
          type="button"
          draggable={!busy}
          aria-label={`Drag ${title}`}
          aria-grabbed={dragging}
          title="Drag to rearrange pane"
          className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onFocus={onResizeFocus}
          onDragStart={(event) => {
            event.dataTransfer.effectAllowed = "move";
            event.dataTransfer.setData("text/plain", pane.paneId);
            onDragStart();
          }}
          onDragEnd={onDragEnd}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerCancel}
        >
          <GripVerticalIcon className="size-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={onChoose}
          className="min-w-0 flex-1 rounded-md px-1.5 py-1 text-left hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <span className="block truncate text-xs font-medium">{title}</span>
          <span className="block truncate text-[10px] text-muted-foreground">{projectLabel}</span>
        </button>
        {pane.session ? (
          <>
            <Button variant="ghost" size="icon-xs" title="Add pane" aria-label="Add pane" disabled={busy} onClick={onAddPane}>
              <PlusIcon aria-hidden="true" />
            </Button>
            <Button variant="ghost" size="icon-xs" title="Fork into another pane" aria-label="Fork into other pane" disabled={busy || states.get(pane.session.threadId)?.sending} onClick={onFork}>
              <GitForkIcon aria-hidden="true" />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              title={
                canZoom
                  ? `${zoomed ? "Restore pane layout" : "Zoom pane"}${zoomShortcut ? ` (${zoomShortcut})` : ""}`
                  : "Open a second pane to zoom"
              }
              aria-label="Zoom pane"
              aria-pressed={zoomed}
              disabled={!canZoom}
              onClick={onZoom}
            >
              {zoomed ? <ShrinkIcon aria-hidden="true" /> : <ExpandIcon aria-hidden="true" />}
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              title={minimizeBlockedReason ?? `Minimize pane${minimizeShortcut ? ` (${minimizeShortcut})` : ""}`}
              aria-label="Minimize pane"
              aria-expanded
              aria-controls={bodyId}
              disabled={Boolean(minimizeBlockedReason)}
              onClick={onMinimize}
            >
              <MinusIcon aria-hidden="true" />
            </Button>
            <Button variant="ghost" size="icon-xs" title="Continue in single view" aria-label="Continue in single view" disabled={busy} onClick={onContinue}>
              <Maximize2Icon aria-hidden="true" />
            </Button>
          </>
        ) : null}
        <Button variant="ghost" size="icon-xs" title="Close pane" aria-label={`Close ${title}`} disabled={busy} onClick={onClose}>
          <XIcon aria-hidden="true" />
        </Button>
      </header>
      {choosing ? (
        <PaneChooser
          pane={pane}
          busy={busy}
          query={query}
          setQuery={setQuery}
          projects={projects}
          currentThreadIds={currentThreadIds}
          onCancel={onCancelChoose}
          onChoose={onChooseTarget}
          onNewChat={onNewChat}
        />
      ) : pane.session ? (
        <SplitConversation
          session={pane.session}
          state={states.get(pane.session.threadId)}
          draft={draft}
          busy={busy}
          onDraft={onDraft}
          onSend={onSend}
          onStop={onStop}
          onError={onError}
          onResolveApproval={onResolveApproval}
          onResolveQuestion={onResolveQuestion}
          providers={providers}
          onUpdateOptions={onUpdateOptions}
          active={!collapsed}
        />
      ) : null}
      </div>
    </section>
  );
}

function PaneChooser({
  pane,
  busy,
  query,
  setQuery,
  projects,
  currentThreadIds,
  onCancel,
  onChoose,
  onNewChat,
}: {
  pane: SplitPaneSnapshot;
  busy: boolean;
  query: string;
  setQuery: (value: string) => void;
  projects: ProjectChats[];
  currentThreadIds: Set<string>;
  onCancel: () => void;
  onChoose: (target: SplitChatTarget) => void;
  onNewChat: () => void;
}) {
  const needle = query.trim().toLowerCase();
  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-4">
      <div className="mb-3 flex gap-2">
        <input aria-label="Find a chat" placeholder="Find a chat or project" value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-0 flex-1 rounded-md border border-border bg-background px-3 py-2 text-sm" />
        {pane.session ? <Button variant="ghost" size="sm" onClick={onCancel}>Cancel</Button> : null}
      </div>
      <Button variant="outline" size="sm" disabled={busy} onClick={onNewChat}>New chat in this project</Button>
      <div className="mt-4 flex flex-col gap-1">
        {projects.flatMap((project) => project.threads
          .filter((thread) => !currentThreadIds.has(thread.id))
          .filter((thread) => `${project.name} ${thread.title ?? ""}`.toLowerCase().includes(needle))
          .map((thread) => (
            <button key={`${project.path}:${thread.id}`} type="button" disabled={busy} className="rounded-lg px-3 py-2 text-left hover:bg-secondary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => onChoose({ root: project.path, threadId: thread.id })}>
              <span className="block truncate text-sm">{thread.title || "Untitled chat"}</span>
              <span className="block truncate text-xs text-muted-foreground">{project.name}</span>
            </button>
          ))) }
      </div>
    </div>
  );
}

function SplitConversation({
  session,
  state,
  draft,
  busy,
  providers,
  onUpdateOptions,
  onDraft,
  onSend,
  onStop,
  onError,
  onResolveApproval,
  onResolveQuestion,
  active = true,
}: {
  session: SessionInfo;
  /** False while the pane is a strip: its transcript is hidden but still mounted. */
  active?: boolean;
  state?: ChatUiState;
  draft: string;
  busy: boolean;
  providers: ProviderRow[];
  onUpdateOptions: (options: SplitPaneOptions) => void;
  onDraft: (value: string) => void;
  onSend: () => Promise<void>;
  onStop: () => void;
  onError: (value: string) => void;
  onResolveApproval: Props["onResolveApproval"];
  onResolveQuestion: Props["onResolveQuestion"];
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const messages = state?.messages ?? session.messages;
  const sending = state?.sending ?? false;

  const savedScroll = useRef(0);

  useEffect(() => {
    if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
  }, [messages]);

  // A hidden element loses its scroll offset, so put the reader back where they
  // were (or at the end, if they were following) when the pane returns.
  useEffect(() => {
    const node = viewport.current;
    if (active && node) node.scrollTop = follow.current ? node.scrollHeight : savedScroll.current;
  }, [active]);

  return (
    <>
      <div ref={viewport} className="min-h-0 flex-1 overflow-y-auto px-5 py-4" onScroll={() => {
        const node = viewport.current;
        // Hiding the element reports a scroll to 0; that is not the reader moving.
        if (!node || !active) return;
        savedScroll.current = node.scrollTop;
        follow.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
      }}>
        {session.hasOlderMessages ? <p className="mb-4 text-xs text-muted-foreground">Showing recent messages. Continue in single view to load earlier history.</p> : null}
        {!messages.length ? <p className="py-12 text-center text-sm text-muted-foreground">Start a conversation.</p> : null}
        {messages.map((message) => (
          <article key={message.id} className="mb-6 min-w-0 break-words">
            <p className="mb-2 text-xs font-medium text-muted-foreground">{message.role === "user" ? "You" : session.label}</p>
            {message.role === "user" ? (
              <div className="whitespace-pre-wrap rounded-xl bg-secondary p-3 text-sm">
                {message.text}
                {message.attachments?.map((item) => <span key={item.name} className="block text-xs text-muted-foreground">{item.name}</span>)}
              </div>
            ) : (
              <>
                <AssistantTrace message={message} providerLabel={session.label} className="mb-3" />
                <Markdown streaming={message.streaming}>{message.text}</Markdown>
                {/* A pane has no shared approval bar above its composer, so the card stays inline. */}
                {message.tools.filter((tool) => tool.status === "awaiting_approval").map((tool) => <ToolCallRow key={tool.id} tool={tool} asCard onResolveApproval={async (id, choice) => {
                  try {
                    await onResolveApproval(id, choice, session.threadId);
                  } catch (error) {
                    onError(String(error));
                    throw error;
                  }
                }} />)}
                {message.question?.questionId ? <PlanningQuestionnaire question={message.question} onSubmit={async (answer) => {
                  try {
                    await onResolveQuestion(message.question!.questionId!, answer, session.threadId);
                  } catch (error) {
                    onError(String(error));
                    throw error;
                  }
                }} /> : null}
                {message.error ? <p role="alert" className="text-sm text-destructive">{message.error}</p> : null}
              </>
            )}
          </article>
        ))}
      </div>
      <form className="m-3 shrink-0 rounded-xl border border-border bg-card p-3 focus-within:ring-1 focus-within:ring-ring/40" onSubmit={(event) => {
        event.preventDefault();
        if (!busy && !sending) void onSend();
      }}>
        <textarea aria-label="Message" placeholder="Ask anything…" value={draft} onChange={(event) => onDraft(event.target.value)} rows={3} className="w-full resize-none bg-transparent text-sm outline-none" onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (!busy && !sending) void onSend();
          }
        }} />
        <div className="flex items-center justify-between gap-2">
          {sending ? (
            <span role="status" className="truncate text-xs text-muted-foreground">Working…</span>
          ) : (
            <ModelEffortPicker
              model={session.model}
              effort={effortFromSession(session.effort, DEFAULT_EFFORT)}
              models={session.models}
              defaultModel={session.defaultModel}
              currentProviderId={session.provider}
              currentProviderLabel={session.label}
              providers={providers}
              disabled={busy}
              pending={busy}
              onModelChange={(model) => onUpdateOptions({ model })}
              onEffortChange={(effort) => onUpdateOptions({ effort })}
              onSwitchProvider={(providerId, model) => onUpdateOptions({ providerId, model })}
              onReset={() => onUpdateOptions({ reset: true })}
            />
          )}
          {sending ? <Button type="button" size="icon-sm" aria-label="Stop response" disabled={busy} onClick={onStop}><SquareIcon aria-hidden="true" /></Button> : <Button type="submit" size="icon-sm" aria-label="Send message" disabled={busy || !draft.trim()}><ArrowUpIcon aria-hidden="true" /></Button>}
        </div>
      </form>
    </>
  );
}
