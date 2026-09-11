import { useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import {
  ArrowUpIcon,
  GripVerticalIcon,
  GitForkIcon,
  Maximize2Icon,
  PlusIcon,
  SquareIcon,
  XIcon,
} from "lucide-react";

import { Markdown } from "@/components/Markdown";
import { ModelEffortPicker } from "@/components/ModelEffortPicker";
import { PlanningQuestionnaire } from "@/components/PlanningQuestionnaire";
import { ToolCallRow } from "@/components/ToolCallRow";
import { readSidebarOpen, writeSidebarOpen } from "@/components/ChatHistorySidebar";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { loadDraft } from "@/lib/drafts";
import { DEFAULT_EFFORT, type EffortId } from "@/lib/models";
import { effortFromSession } from "@/lib/sessionOptions";
import type { ChatUiState } from "@/lib/chatReducer";
import {
  addPaneToLayout,
  appendSplitGroup,
  clampSplitRatio,
  createSplitId,
  createSplitWorkspaceSnapshot,
  layoutPaneIds,
  movePaneInLayout,
  removePaneFromLayout,
  splitSidebarGroups,
  targetForSession,
  updateSplitRatio,
  type LayoutNode,
  type SplitAxis,
  type SplitChatTarget,
  type SplitGroup,
  type SplitPaneSnapshot,
  type SplitSidebarGroup,
  type SplitWorkspaceSnapshot,
} from "@/lib/splitLayout";
import type { ProjectChats, ProviderRow, SessionInfo } from "@/lib/types";

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
  onStateChange?: (snapshot: SplitWorkspaceSnapshot) => void;
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
  renderSidebar,
}: Props) {
  const initialWorkspaceRef = useRef<SplitWorkspaceSnapshot | null>(null);
  if (!initialWorkspaceRef.current) {
    initialWorkspaceRef.current = initialWorkspace(snapshot, initial, initialDraft, startNewGroup);
  }
  const initialState = initialWorkspaceRef.current;
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
    onStateChange?.(workspace);
  }, [onStateChange, workspace]);

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
      return changed ? { ...current, panes } : current;
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
      groups: current.groups.map((group) =>
        group.id === groupId ? { ...group, focusedPaneId: paneId } : group
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
            ? { ...candidate, root: currentRoot, focusedPaneId: paneId }
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
            ? { ...candidate, root, focusedPaneId: nextFocus }
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

  const renderNode = (node: LayoutNode, path: string[]): ReactNode => {
    if (node.kind === "pane") {
      const pane = workspace.panes[node.paneId];
      if (!pane) return null;
      return (
        <SplitPaneView
          key={node.paneId}
          pane={pane}
          label={paneLabel(path)}
          title={titleForPane(pane, threadTitles)}
          projectLabel={projectForPane(pane)}
          busy={busy}
          states={states}
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
          providers={providers}
          onUpdateOptions={(options) => updatePaneOptions(node.paneId, options)}
          onResizeFocus={() => focusPane(activeGroup.id, node.paneId)}
          dragging={draggedPaneId === node.paneId}
          canDrop={draggedPaneId !== null && draggedPaneId !== node.paneId}
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
    const axisClass = node.axis === "row" ? "flex-row" : "flex-col";
    const separatorClass = node.axis === "row"
      ? "h-full w-2 cursor-col-resize"
      : "h-2 w-full cursor-row-resize";
    const firstStyle: CSSProperties = { flex: `0 0 calc(${ratio}% - 4px)` };

    return (
      <div key={node.id} className={`zest-split-node flex min-h-0 min-w-0 flex-1 ${axisClass}`}>
        <div className="flex min-h-0 min-w-0 flex-col" style={firstStyle}>
          {renderNode(node.first, [...path, "first"])}
        </div>
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
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {renderNode(node.second, [...path, "second"])}
        </div>
      </div>
    );
  };

  return (
    <section aria-label="Split workspace" className="flex min-h-0 min-w-0 flex-1 bg-[var(--canvas-solid)]">
      {sidebar}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {error ? (
          <p role="alert" className="shrink-0 border-b border-border px-4 py-2 text-sm text-destructive">
            {error}
          </p>
        ) : null}
        <div className="zest-split-panes flex min-h-0 flex-1">
          {renderNode(activeGroup.root, [])}
        </div>
      </div>
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
  return (
    <section
      aria-label={label}
      data-split-pane-id={pane.paneId}
      className={`zest-split-pane flex min-h-0 min-w-0 flex-col ${dragging ? "opacity-60" : ""} ${dropTarget ? "ring-2 ring-inset ring-primary" : canDrop ? "ring-1 ring-inset ring-primary/40" : ""}`}
      onDragOver={(event) => {
        if (!canDrop) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
      }}
      onDrop={onDrop}
    >
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
          providers={providers}
          onUpdateOptions={onUpdateOptions}
        />
      ) : null}
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
}: {
  session: SessionInfo;
  state?: ChatUiState;
  draft: string;
  busy: boolean;
  providers: ProviderRow[];
  onUpdateOptions: (options: SplitPaneOptions) => void;
  onDraft: (value: string) => void;
  onSend: () => Promise<void>;
  onStop: () => void;
  onError: (value: string) => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const messages = state?.messages ?? session.messages;
  const sending = state?.sending ?? false;

  useEffect(() => {
    if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
  }, [messages]);

  return (
    <>
      <div ref={viewport} className="min-h-0 flex-1 overflow-y-auto px-5 py-4" onScroll={() => {
        const node = viewport.current;
        if (node) follow.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
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
                {message.thinking ? <details className="mb-2 text-xs text-muted-foreground"><summary>Thinking</summary><p className="whitespace-pre-wrap py-2">{message.thinking}</p></details> : null}
                <Markdown streaming={message.streaming}>{message.text}</Markdown>
                {message.tools.map((tool) => <ToolCallRow key={tool.id} tool={tool} asCard={tool.status === "awaiting_approval"} onResolveApproval={async (id, choice) => {
                  try {
                    await getBackend().resolveApproval(id, choice, session.threadId);
                  } catch (error) {
                    onError(String(error));
                    throw error;
                  }
                }} />)}
                {message.question?.questionId ? <PlanningQuestionnaire question={message.question} onSubmit={async (answer) => {
                  try {
                    await getBackend().resolveQuestion(message.question!.questionId!, answer, session.threadId);
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
