import {
  memo,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ComponentType,
} from "react";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  CheckCircle2Icon,
  CheckIcon,
  Columns2Icon,
  Clock3Icon,
  ChevronRightIcon,
  PanelLeftIcon,
  FolderIcon,
  FolderOpenIcon,
  FolderPlusIcon,
  GitBranchIcon,
  GitForkIcon,
  GitPullRequestIcon,
  LoaderCircleIcon,
  MoreHorizontalIcon,
  PinIcon,
  PlusIcon,
  SearchIcon,
  SlidersHorizontalIcon,
  SquarePenIcon,
  Trash2Icon,
  TriangleAlertIcon,
  XCircleIcon,
} from "lucide-react";

import { AccountMenu } from "@/components/AccountMenu";
import { AgentQuotaButton } from "@/components/AgentQuotaButton";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIcon } from "@/components/ProviderIcon";
import type { SettingsSectionId } from "@/components/SettingsPanel";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import type { PaletteFilter } from "@/lib/commandPaletteSearch";
import {
  CHAT_STATUS_SECTIONS,
  chatProjectLabel,
  groupChatsByStatus,
  type ChatStatusSection,
} from "@/lib/chatStatusGroups";
import { compactChatMark, foreignProvider, type CompactChatMark } from "@/lib/compactChatRow";
import {
  delegationChildStatus,
  delegationChildrenByThread,
  type DelegationChildTone,
  type DelegationChildren,
} from "@/lib/delegationChildren";
import { isBooleanRecord, parseJson } from "@/lib/json";
import { formatChord } from "@/lib/keybindings";
import {
  getSavedChatGrouping,
  getSavedChatViewMode,
  saveChatGrouping,
  subscribeChatViewChange,
  type ChatGrouping,
  type ChatViewMode,
} from "@/lib/chatView";
import {
  activeThreadIds,
  elapsedLabel,
  needsInput,
  type ThreadActivity,
  type ThreadActivityMap,
} from "@/lib/threadActivity";
import { getUnreadThreads, subscribeUnreadThreads } from "@/lib/threadUnread";
import type {
  DelegationJob,
  ProjectChats,
  ProviderRow,
  ThreadSummary,
  UserProfile,
} from "@/lib/types";
import type { SplitSidebarGroup } from "@/lib/splitLayout";
import { cn } from "@/lib/utils";
import {
  handlePullRequestClick,
  pullRequestAnchorProps,
} from "@/lib/pullRequestLink";

type Props = {
  onOpenPullRequests?: () => void;
  pullRequestsActive?: boolean;
  open: boolean;
  activeThreadId: string;
  activeProjectPath: string | null;
  activeProviderId: string;
  sending: boolean;
  threadActivity: ThreadActivityMap;
  onOpenChange: (open: boolean) => void;
  onNewChat: () => void;
  onOpenProjectChat: (options: {
    root: string | null;
    threadId?: string;
    newThread?: boolean;
    providerId?: string;
    copyThread?: boolean;
  }) => Promise<boolean>;
  onForkThread: () => Promise<void>;
  onDeleteThread: (
    id: string,
    projectPath: string | null,
    freeChat: boolean
  ) => Promise<void>;
  onOpenFolder: () => void;
  /** Absent in surfaces that have no app-level navigation (none today). */
  onOpenCustomize?: () => void;
  /** Customize is showing in place of the transcript. */
  customizeActive?: boolean;
  /** Name and avatar for the profile row at the bottom. */
  profile: UserProfile;
  /** Provider shown under the name, e.g. "Deepseek · No workspace". */
  providerLabel?: string;
  /** Open the profile screen. */
  onOpenProfile?: () => void;
  /** Open Settings, optionally at one section. */
  onOpenSettings?: (section?: SettingsSectionId) => void;
  /** The profile screen is showing. */
  profileActive?: boolean;
  providers: ProviderRow[];
  quotaRefreshKey: string | number;
  /** Open the command palette, optionally scoped to chat history. */
  onSearch: (filter?: PaletteFilter) => void;
  /** Show the transcript when a shell panel is covering the active chat. */
  onRevealTranscript: () => void;
  canNavigateBack: boolean;
  canNavigateForward: boolean;
  onNavigateBack: () => void;
  onNavigateForward: () => void;
  /** Open this chat's pull request in the review pane. */
  onOpenPullRequest?: (project: ProjectChats, thread: ThreadSummary) => void;
  /** Saved multi-pane layouts shown separately from normal chat history. */
  splitGroups?: ReadonlyArray<SplitSidebarGroup>;
  activeSplitGroupId?: string | null;
  onOpenSplitGroup?: (groupId: string) => void;
  onFocusSplitPane?: (groupId: string, paneId: string) => void;
  /** Feature-card jobs for the open project, nested under the chat that started each. */
  delegationJobs?: ReadonlyArray<DelegationJob>;
  /** Open the Workbench on its Delegation tab at this job. */
  onOpenDelegationJob?: (jobId: string) => void;
};

function threadTitle(thread: ThreadSummary) {
  const title = thread.title?.trim();
  if (title) return title;
  return "Untitled chat";
}

/** Compact relative age (6m, 8h, 2d). */
function formatAge(epochSecs: number) {
  if (!epochSecs) return "";
  const now = Math.floor(Date.now() / 1000);
  const delta = Math.max(0, now - epochSecs);
  if (delta < 60) return `${delta}s`;
  if (delta < 3600) return `${Math.floor(delta / 60)}m`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h`;
  if (delta < 86400 * 14) return `${Math.floor(delta / 86400)}d`;
  try {
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
    }).format(new Date(epochSecs * 1000));
  } catch {
    return "";
  }
}

const STORAGE_KEY = "zest.sidebarOpen";
const EXPANDED_KEY = "zest.sidebarProjectsExpanded";
const MAX_CHAT_TITLE_CHARS = 200;

export function readSidebarOpen(): boolean {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return true;
    return raw === "1" || raw === "true";
  } catch {
    return true;
  }
}

export function writeSidebarOpen(open: boolean) {
  try {
    localStorage.setItem(STORAGE_KEY, open ? "1" : "0");
  } catch {
    /* ignore */
  }
}

function readExpandedMap(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(EXPANDED_KEY);
    if (!raw) return {};
    const parsed = parseJson(raw);
    return isBooleanRecord(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function writeExpandedMap(map: Record<string, boolean>) {
  try {
    localStorage.setItem(EXPANDED_KEY, JSON.stringify(map));
  } catch {
    /* ignore */
  }
}

function navItemClass(active = false) {
  return cn(
    "flex h-7 w-full cursor-pointer items-center gap-2 rounded-md px-2 text-left text-[13px] outline-none transition-colors",
    "hover:bg-[var(--sidebar-accent)] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50",
    active && "bg-[var(--sidebar-accent)] text-foreground"
  );
}

function activityStateLabel(activity: ThreadActivity) {
  if (activity.state === "awaiting_approval") return "Needs approval";
  if (activity.state === "awaiting_input") return "Needs your answer";
  return "Working";
}

function activityDescription(activity: ThreadActivity, now: number) {
  const parts = [activityStateLabel(activity)];
  if (activity.tool) parts.push(`running ${activity.tool.replaceAll("_", " ")}`);
  const elapsed = elapsedLabel(activity.startedAt, now);
  if (elapsed) parts.push(`for ${elapsed}`);
  return parts.join(", ");
}

/** The leading mark of a compact row; `compactChatMark` decides which. */
function CompactChatMarkIcon({ mark }: { mark: CompactChatMark }) {
  return (
    <span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden="true">
      {mark === "needs_input" ? (
        <span className="size-2 rounded-full bg-amber-400" />
      ) : mark === "working" ? (
        <LoaderCircleIcon className="size-3.5 animate-spin text-[var(--sidebar-primary)] motion-reduce:animate-none" />
      ) : mark === "pull_request" ? (
        <GitPullRequestIcon className="size-3.5 text-[var(--sidebar-primary)]" />
      ) : mark === "branch" ? (
        <GitBranchIcon className="size-3.5 text-muted-foreground" />
      ) : (
        <span className="size-2 rounded-full border border-muted-foreground/50" />
      )}
    </span>
  );
}

/**
 * A running chat's title shimmers, like the working indicator in the
 * transcript. With reduced motion the app-wide rule stops the sweep, but a
 * gradient frozen mid-sweep leaves the title half dimmed, so it falls back to
 * plain text. The overrides need `!` because `.shimmer-text` is unlayered.
 */
const WORKING_TITLE_CLASS = "shimmer-text motion-reduce:bg-none! motion-reduce:text-inherit!";

const STATUS_SECTION_META: Record<
  ChatStatusSection,
  { label: string; Icon: ComponentType<{ className?: string; "aria-hidden"?: boolean }>; iconClass: string }
> = {
  needs_input: { label: "Needs input", Icon: TriangleAlertIcon, iconClass: "text-amber-400" },
  working: { label: "Working", Icon: LoaderCircleIcon, iconClass: "text-[var(--sidebar-primary)]" },
  recent: { label: "Recent", Icon: Clock3Icon, iconClass: "text-muted-foreground" },
};

const NO_DELEGATION_JOBS: ReadonlyArray<DelegationJob> = [];

function DelegationChildIcon({ tone }: { tone: DelegationChildTone }) {
  const base = "size-3 shrink-0";
  switch (tone) {
    case "attention":
      return <TriangleAlertIcon className={cn(base, "text-amber-400")} aria-hidden="true" />;
    case "running":
      return (
        <LoaderCircleIcon
          className={cn(base, "animate-spin text-[var(--sidebar-primary)] motion-reduce:animate-none")}
          aria-hidden="true"
        />
      );
    case "ready":
      return <CheckCircle2Icon className={cn(base, "text-[var(--sidebar-primary)]")} aria-hidden="true" />;
    case "waiting":
      return <Clock3Icon className={cn(base, "text-muted-foreground")} aria-hidden="true" />;
    case "done":
      return <CheckIcon className={cn(base, "text-muted-foreground")} aria-hidden="true" />;
    case "cancelled":
      return <XCircleIcon className={cn(base, "text-muted-foreground")} aria-hidden="true" />;
  }
}

type EditingThread = {
  key: string;
  id: string;
  projectPath: string | null;
  value: string;
};

type WorkspaceAction = {
  project: ProjectChats;
};

export const ChatHistorySidebar = memo(function ChatHistorySidebar({
  onOpenPullRequests,
  pullRequestsActive = false,
  open,
  activeThreadId,
  activeProjectPath,
  activeProviderId,
  sending,
  threadActivity,
  onOpenChange,
  onNewChat,
  onOpenProjectChat,
  onForkThread,
  onDeleteThread,
  onOpenFolder,
  onOpenCustomize,
  customizeActive = false,
  profile,
  providerLabel,
  onOpenProfile,
  onOpenSettings,
  profileActive = false,
  providers,
  quotaRefreshKey,
  onSearch,
  onRevealTranscript,
  canNavigateBack,
  canNavigateForward,
  onNavigateBack,
  onNavigateForward,
  onOpenPullRequest,
  splitGroups = [],
  activeSplitGroupId = null,
  onOpenSplitGroup,
  onFocusSplitPane,
  delegationJobs = NO_DELEGATION_JOBS,
  onOpenDelegationJob,
}: Props) {
  const [projects, setProjects] = useState<ProjectChats[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [expanded, setExpanded] = useState<Record<string, boolean>>(readExpandedMap);
  const [pendingDelete, setPendingDelete] = useState<{
    thread: ThreadSummary;
    projectPath: string | null;
  } | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [pinning, setPinning] = useState<string | null>(null);
  const [editingThread, setEditingThread] = useState<EditingThread | null>(null);
  const [renameBusy, setRenameBusy] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const renameSavingRef = useRef(false);
  const renameCancelledRef = useRef(false);
  const projectMenuRef = useRef<HTMLDivElement>(null);
  const [now, setNow] = useState(() => Date.now());
  const [projectMenuPath, setProjectMenuPath] = useState<string | null>(null);
  const [workspaceAction, setWorkspaceAction] = useState<WorkspaceAction | null>(null);
  const [projectBusy, setProjectBusy] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [chatViewMode, setChatViewMode] = useState<ChatViewMode>(() => getSavedChatViewMode());
  const unreadThreads = useSyncExternalStore(subscribeUnreadThreads, getUnreadThreads);
  const [grouping, setGrouping] = useState<ChatGrouping>(() => getSavedChatGrouping());

  const hasActiveActivity = Object.values(threadActivity).some(
    (activity) => activity.state !== "idle"
  );

  useEffect(() => subscribeChatViewChange(() => setChatViewMode(getSavedChatViewMode())), []);

  useEffect(() => {
    if (!hasActiveActivity) return;
    setNow(Date.now());
    const interval = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, [hasActiveActivity]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    getBackend()
      .listChatProjects()
      .then((list) => {
        if (!cancelled) setProjects(list);
      })
      .catch(() => {
        if (!cancelled) setError("Could not load chat history. Try again.");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, activeThreadId, activeProjectPath, activeProviderId, tick]);

  useEffect(() => {
    if (!open) return;
    const onFocus = () => setTick((value) => value + 1);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [open]);

  useEffect(() => {
    if (!projectMenuPath) return;

    const onPointerDown = (event: PointerEvent) => {
      const menu = projectMenuRef.current;
      if (menu && event.target instanceof Node && menu.contains(event.target)) {
        return;
      }
      setProjectMenuPath(null);
    };

    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [projectMenuPath]);

  const wasSending = useRef(false);
  useEffect(() => {
    if (sending) {
      wasSending.current = true;
      return;
    }
    if (open && wasSending.current) {
      wasSending.current = false;
      setTick((n) => n + 1);
    }
  }, [open, sending]);

  // A chat that ends a turn in the background has a new `updatedAt` (and maybe
  // a new title) the list has not seen, so reload when any busy chat settles.
  const busyThreadKey = useMemo(
    () => activeThreadIds(threadActivity).sort().join(" "),
    [threadActivity]
  );
  const previousBusyThreadKey = useRef(busyThreadKey);
  useEffect(() => {
    const before = previousBusyThreadKey.current;
    previousBusyThreadKey.current = busyThreadKey;
    if (!open || !before) return;
    const stillBusy = new Set(busyThreadKey.split(" "));
    if (before.split(" ").some((id) => !stillBusy.has(id))) {
      setTick((n) => n + 1);
    }
  }, [busyThreadKey, open]);

  const editingKey = editingThread?.key;
  useEffect(() => {
    if (!editingKey) return;
    const frame = window.requestAnimationFrame(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [editingKey]);

  const freeChatProject = useMemo(
    () => projects.find((project) => project.path === null) ?? null,
    [projects]
  );
  const recentThreads = useMemo(
    () =>
      freeChatProject
        ? freeChatProject.threads
            .map((thread) => ({ project: freeChatProject, thread }))
            .sort((a, b) => b.thread.updatedAt - a.thread.updatedAt)
        : [],
    [freeChatProject]
  );

  const statusGroups = useMemo(
    () => groupChatsByStatus(projects, threadActivity),
    [projects, threadActivity]
  );

  const delegationChildren = useMemo(
    () => delegationChildrenByThread(delegationJobs),
    [delegationJobs]
  );

  function chooseGrouping(next: ChatGrouping) {
    setGrouping(saveChatGrouping(next));
  }

  const visibleProjects = useMemo(
    () =>
      projects.filter(
        (project): project is ProjectChats & { path: string } =>
          project.path !== null
      ),
    [projects]
  );

  function isExpanded(project: ProjectChats) {
    if (project.path === null) return false;
    if (project.path in expanded) return expanded[project.path];
    // Default: open active project and any project that already has chats.
    return project.active || project.threads.length > 0;
  }

  function toggleExpanded(path: string) {
    setExpanded((prev) => {
      const project = projects.find((p) => p.path === path);
      const currently =
        path in prev
          ? prev[path]
          : Boolean(project && (project.active || project.threads.length > 0));
      const next = { ...prev, [path]: !currently };
      writeExpandedMap(next);
      return next;
    });
  }

  async function confirmWorkspaceAction() {
    if (!workspaceAction || projectBusy) return;
    const action = workspaceAction;
    if (action.project.path === null) return;
    setProjectBusy(true);
    setProjectError(null);
    try {
      await getBackend().forgetWorkspace(action.project.path);
      setWorkspaceAction(null);
      setProjectMenuPath(null);
      setTick((n) => n + 1);
    } catch {
      setProjectError("Could not remove the project from Zest. Switch projects and try again.");
    } finally {
      setProjectBusy(false);
    }
  }

  async function confirmDelete() {
    if (!pendingDelete) return;
    setDeleting(true);
    try {
      await onDeleteThread(
        pendingDelete.thread.id,
        pendingDelete.projectPath,
        pendingDelete.projectPath === null
      );
      setPendingDelete(null);
      setTick((n) => n + 1);
    } catch {
      /* parent toasts */
    } finally {
      setDeleting(false);
    }
  }

  async function togglePinned(projectPath: string | null, thread: ThreadSummary) {
    if (pinning === thread.id) return;
    setPinning(thread.id);
    try {
      await getBackend().setThreadPinned(
        thread.id,
        projectPath,
        !thread.pinned,
        projectPath === null
      );
      setError(null);
      setTick((n) => n + 1);
    } catch {
      setError("Could not update the pinned chat. Try again.");
    } finally {
      setPinning(null);
    }
  }

  function beginRename(project: ProjectChats, thread: ThreadSummary, key: string) {
    const rowIsLiveTurn = sending && thread.id === activeThreadId;
    if (rowIsLiveTurn || deleting || renameBusy || pinning === thread.id) return;
    renameCancelledRef.current = false;
    setRenameError(null);
    setEditingThread({
      key,
      id: thread.id,
      projectPath: project.path,
      value: thread.title?.trim() ?? "",
    });
  }

  function cancelRename() {
    renameCancelledRef.current = true;
    setEditingThread(null);
    setRenameError(null);
  }

  async function commitRename() {
    const edit = editingThread;
    if (!edit || renameSavingRef.current) return;
    const title = edit.value.trim();
    if (!title) {
      cancelRename();
      return;
    }

    renameCancelledRef.current = true;
    renameSavingRef.current = true;
    setRenameBusy(true);
    setRenameError(null);
    try {
      await getBackend().renameThread(
        edit.id,
        edit.projectPath,
        title,
        edit.projectPath === null
      );
      setEditingThread(null);
      setTick((n) => n + 1);
    } catch {
      renameCancelledRef.current = false;
      setRenameError("Could not rename chat. Try again.");
      window.requestAnimationFrame(() => {
        renameInputRef.current?.focus();
        renameInputRef.current?.select();
      });
    } finally {
      renameSavingRef.current = false;
      setRenameBusy(false);
    }
  }

  function handleRenameBlur() {
    window.setTimeout(() => {
      if (!renameCancelledRef.current) void commitRename();
    }, 0);
  }

  async function openThread(project: ProjectChats, thread: ThreadSummary) {
    // Route every thread open through the project-aware backend. This preserves
    // provider ownership and lets legacy or unavailable chats show recovery
    // actions instead of silently switching providers.
    await onOpenProjectChat({
      root: project.path,
      threadId: thread.id,
    });
  }

  function renderThreadItem(
    project: ProjectChats,
    thread: ThreadSummary,
    key: string
  ) {
    const active = thread.id === activeThreadId;
    const title = threadTitle(thread);
    const age = formatAge(thread.updatedAt);
    const activity = threadActivity[thread.id];
    const activityText =
      activity && activity.state !== "idle"
        ? activityDescription(activity, now)
        : undefined;
    const git = thread.gitContext;
    const branchChanged = Boolean(
      git?.branch && git.baseBranch && git.branch !== git.baseBranch
    );
    const pullRequest = git?.pullRequest;
    const gitText = pullRequest
      ? `Pull request #${pullRequest.number}: +${pullRequest.additions} −${pullRequest.deletions} · ${pullRequest.changedFiles} files`
      : branchChanged
        ? `Branch ${git?.branch} differs from ${git?.baseBranch}`
        : undefined;
    // Shown on every row now that it is a mark rather than a word. The name
    // used to be hidden unless a project mixed providers, because `anthropic`
    // spelled out next to every chat was noise — a glyph is not, and knowing
    // who owns a chat before you open it is worth a few pixels.
    const owner = thread.providerId;
    const isEditing = editingThread?.key === key;

    return (
      <li key={key} className="group/thread">
        <div
          className={cn(
            "relative flex items-center gap-0.5 rounded-md",
            isEditing
              ? "bg-[var(--sidebar-accent)]"
              : active
                ? "bg-[color-mix(in_srgb,var(--sidebar-primary)_18%,transparent)] text-[var(--sidebar-accent-foreground)] before:absolute before:inset-y-1 before:left-0 before:w-0.5 before:rounded-full before:bg-[var(--sidebar-primary)]"
                : "hover:bg-[var(--sidebar-accent)] hover:text-[var(--sidebar-accent-foreground)]"
          )}
        >
          {isEditing ? (
            <div
              role="group"
              aria-label={`Renaming ${title}`}
              className="flex min-w-0 flex-1 items-center gap-1.5 py-1 pr-1 pl-2.5"
            >
              {owner ? (
                <ProviderIcon
                  providerId={owner}
                  label={owner}
                  className="size-4 shrink-0 opacity-80"
                />
              ) : null}
              <input
                ref={renameInputRef}
                value={editingThread?.value ?? ""}
                maxLength={MAX_CHAT_TITLE_CHARS}
                aria-label={`Rename chat ${title}`}
                placeholder="Untitled chat"
                disabled={renameBusy}
                className="min-w-0 flex-1 rounded-sm border border-primary/60 bg-background/60 px-1.5 py-0.5 text-[13px] text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-60"
                onChange={(event) =>
                  setEditingThread((current) =>
                    current ? { ...current, value: event.target.value } : current
                  )
                }
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void commitRename();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    cancelRename();
                  }
                }}
                onBlur={handleRenameBlur}
              />
            </div>
          ) : (
          <div className="flex min-w-0 flex-1 items-center">
          <button
            type="button"
            onClick={() => {
              if (active) {
                onRevealTranscript();
                return;
              }
              void openThread(project, thread).catch((error) =>
                ignoreExpectedFailure(error, "open chat from history")
              );
            }}
            onDoubleClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              beginRename(project, thread, key);
            }}
            title={`Double-click to rename “${title}”`}
            aria-current={active ? "page" : undefined}
            aria-label={[title, activityText, gitText].filter(Boolean).join(". ")}
            className="flex min-w-0 cursor-pointer items-center gap-1.5 overflow-hidden py-1 pr-1 pl-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          >
            {owner ? (
              <span
                title={`This chat belongs to ${owner}. Zest will keep the original provider or let you open a copy.`}
                className="flex shrink-0 items-center text-muted-foreground"
              >
                <ProviderIcon
                  providerId={owner}
                  label={owner}
                  className="size-4 opacity-80"
                />
              </span>
            ) : null}
            <span className="min-w-0 truncate text-[13px]">{title}</span>
            {branchChanged ? (
              <span
                title={`This chat is on ${git?.branch}; its original branch was ${git?.baseBranch}.`}
                aria-label={gitText}
                className="flex shrink-0 items-center text-muted-foreground"
              >
                <GitBranchIcon className="size-3.5 opacity-80" />
              </span>
            ) : null}
            {pullRequest ? (
              <a
                {...pullRequestAnchorProps(pullRequest.url)}
                title={`Pull request #${pullRequest.number}: ${pullRequest.title} · +${pullRequest.additions} −${pullRequest.deletions} · ${pullRequest.changedFiles} files`}
                aria-label={`Pull request #${pullRequest.number}`}
                className="flex shrink-0 items-center text-muted-foreground hover:text-foreground"
                onClick={(event) =>
                  handlePullRequestClick(event, () =>
                    onOpenPullRequest?.(project, thread)
                  )
                }
              >
                <GitPullRequestIcon className="size-3.5 opacity-80" />
              </a>
            ) : null}
            {activity && activity.state !== "idle" ? (
              <span
                className="flex shrink-0 items-center gap-1 text-[10px] tabular-nums text-muted-foreground"
                aria-hidden="true"
              >
                {needsInput(activity) ? (
                  <span className="size-1.5 rounded-full bg-amber-400" />
                ) : (
                  <span className="flex items-center gap-0.5">
                    <span className="size-1 rounded-full bg-primary animate-bounce [animation-delay:-0.32s]" />
                    <span className="size-1 rounded-full bg-primary animate-bounce [animation-delay:-0.16s]" />
                    <span className="size-1 rounded-full bg-primary animate-bounce" />
                  </span>
                )}
                {elapsedLabel(activity.startedAt, now) ?? age}
              </span>
            ) : age ? (
              <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                {age}
              </span>
            ) : null}
          </button>
          {active ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="Fork conversation"
              aria-label="Fork conversation"
              disabled={sending || deleting}
              className="shrink-0 text-muted-foreground hover:bg-muted hover:text-foreground"
              onClick={(event) => {
                event.stopPropagation();
                void onForkThread();
              }}
            >
              <GitForkIcon aria-hidden="true" />
            </Button>
          ) : null}
          </div>
          )}
          <div className="flex shrink-0 items-center gap-0.5 pr-0.5">
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title={thread.pinned ? "Unpin chat" : "Pin chat"}
              aria-label={thread.pinned ? "Unpin chat" : "Pin chat"}
              aria-pressed={thread.pinned}
              disabled={deleting || pinning === thread.id || (sending && active)}
              className={cn(
                "text-muted-foreground hover:bg-muted hover:text-foreground",
                thread.pinned
                  ? "fill-current text-primary opacity-100"
                  : "opacity-0 group-hover/thread:opacity-100 focus-visible:opacity-100"
              )}
              onClick={(event) => {
                event.stopPropagation();
                void togglePinned(project.path, thread);
              }}
            >
              <PinIcon aria-hidden="true" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title={`Delete “${title}”`}
              disabled={deleting}
              className={cn(
                "text-muted-foreground hover:bg-destructive/15 hover:text-destructive",
                "focus-visible:opacity-100",
                active ? "opacity-100" : "opacity-0 group-hover/thread:opacity-100"
              )}
              onClick={(event) => {
                event.stopPropagation();
                setPendingDelete({
                  thread,
                  projectPath: project.path,
                });
              }}
            >
              <Trash2Icon />
            </Button>
          </div>
        </div>
      </li>
    );
  }

  function renderCompactThreadItem(
    project: ProjectChats,
    thread: ThreadSummary,
    key: string,
    { projectLabel }: { projectLabel?: string } = {}
  ) {
    const active = thread.id === activeThreadId;
    const title = threadTitle(thread);
    const activity = threadActivity[thread.id];
    const busy = activity && activity.state !== "idle" ? activity : undefined;
    const git = thread.gitContext;
    const pullRequest = git?.pullRequest;
    const branchChanged = Boolean(
      git?.branch && git.baseBranch && git.branch !== git.baseBranch
    );
    const mark = compactChatMark({
      activity,
      hasPullRequest: Boolean(pullRequest),
      branchChanged,
    });
    const otherProvider = foreignProvider(thread.providerId, activeProviderId);
    const isEditing = editingThread?.key === key;
    // Bold until opened, like unread mail. A running chat has nothing new yet.
    const unread = unreadThreads.has(thread.id) && !active && !busy;
    const working = busy?.state === "working";
    const markText =
      mark === "pull_request" && pullRequest
        ? `Pull request #${pullRequest.number}`
        : mark === "branch"
          ? `On ${git?.branch}, started on ${git?.baseBranch}`
          : busy
            ? activityDescription(busy, now)
            : undefined;
    const ariaLabel = [
      title,
      markText,
      unread ? "Finished since you last looked" : undefined,
      projectLabel ? `in ${projectLabel}` : undefined,
      otherProvider ? `on ${otherProvider}` : undefined,
      thread.pinned ? "pinned" : undefined,
    ]
      .filter(Boolean)
      .join(". ");

    const children = delegationChildren.get(thread.id);

    return (
      <li key={key}>
        <div
          className={cn(
            "group/thread relative flex h-7 items-center rounded-md",
            isEditing || active
              ? "bg-[var(--sidebar-accent)] text-[var(--sidebar-accent-foreground)]"
              : "hover:bg-[var(--sidebar-accent)] hover:text-[var(--sidebar-accent-foreground)]"
          )}
        >
          {isEditing ? (
            <div
              role="group"
              aria-label={`Renaming ${title}`}
              className="flex min-w-0 flex-1 items-center gap-2 pl-2 pr-1"
            >
              <CompactChatMarkIcon mark={mark} />
              <input
                ref={renameInputRef}
                value={editingThread?.value ?? ""}
                maxLength={MAX_CHAT_TITLE_CHARS}
                aria-label={`Rename chat ${title}`}
                placeholder="Untitled chat"
                disabled={renameBusy}
                className="min-w-0 flex-1 rounded-sm border border-primary/60 bg-background/60 px-1.5 py-0.5 text-[13px] text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-60"
                onChange={(event) =>
                  setEditingThread((current) =>
                    current ? { ...current, value: event.target.value } : current
                  )
                }
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void commitRename();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    cancelRename();
                  }
                }}
                onBlur={handleRenameBlur}
              />
            </div>
          ) : (
            <button
              type="button"
              onClick={() => {
                if (active) {
                  onRevealTranscript();
                  return;
                }
                void openThread(project, thread).catch((error) =>
                  ignoreExpectedFailure(error, "open compact chat history")
                );
              }}
              onDoubleClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                beginRename(project, thread, key);
              }}
              title={markText ? `${title}\n${markText}` : title}
              aria-current={active ? "page" : undefined}
              aria-label={ariaLabel}
              className="flex h-full min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md pl-2 pr-1.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
            >
              <CompactChatMarkIcon mark={mark} />
              <span
                className={cn(
                  "min-w-0 flex-1 truncate text-[13px]",
                  working && WORKING_TITLE_CLASS,
                  unread && "font-semibold text-foreground"
                )}
              >
                {title}
              </span>
              {projectLabel ? (
                <span className="max-w-[40%] shrink-0 truncate text-[11px] text-muted-foreground">
                  {projectLabel}
                </span>
              ) : null}
              {/* Exceptions only. The usual case (your provider, not pinned,
                  not running) adds nothing to the row. */}
              {otherProvider ? (
                <span
                  title={`On ${otherProvider}. Opening it keeps that provider or offers a copy.`}
                  className="flex shrink-0 items-center text-muted-foreground"
                >
                  <ProviderIcon
                    providerId={otherProvider}
                    label={otherProvider}
                    className="size-3.5 opacity-70"
                  />
                </span>
              ) : null}
              {thread.pinned ? (
                <PinIcon
                  className="size-3 shrink-0 fill-current text-muted-foreground"
                  aria-hidden="true"
                />
              ) : null}
              {busy ? (
                <span
                  className="shrink-0 text-[10px] tabular-nums text-muted-foreground"
                  aria-hidden="true"
                >
                  {elapsedLabel(busy.startedAt, now)}
                </span>
              ) : null}
            </button>
          )}
          {isEditing ? null : (
            <div className="pointer-events-none absolute inset-y-0 right-0.5 z-10 flex items-center gap-0.5 rounded-md bg-[var(--sidebar-accent)] pl-1 opacity-0 transition-opacity group-hover/thread:pointer-events-auto group-hover/thread:opacity-100 group-focus-within/thread:pointer-events-auto group-focus-within/thread:opacity-100">
              {pullRequest ? (
                <a
                  {...pullRequestAnchorProps(pullRequest.url)}
                  title={`Pull request #${pullRequest.number}: ${pullRequest.title} · +${pullRequest.additions} −${pullRequest.deletions} · ${pullRequest.changedFiles} files`}
                  aria-label={`Open pull request #${pullRequest.number}`}
                  className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                  onClick={(event) =>
                    handlePullRequestClick(event, () => onOpenPullRequest?.(project, thread))
                  }
                >
                  <GitPullRequestIcon className="size-3.5" aria-hidden="true" />
                </a>
              ) : null}
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title={thread.pinned ? "Unpin chat" : "Pin chat"}
                aria-label={thread.pinned ? "Unpin chat" : "Pin chat"}
                aria-pressed={thread.pinned}
                disabled={deleting || pinning === thread.id || (sending && active)}
                className={cn(
                  "text-muted-foreground hover:bg-muted hover:text-foreground",
                  thread.pinned && "fill-current text-primary"
                )}
                onClick={(event) => {
                  event.stopPropagation();
                  void togglePinned(project.path, thread);
                }}
              >
                <PinIcon aria-hidden="true" />
              </Button>
              {active ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  title="Fork conversation"
                  aria-label="Fork conversation"
                  disabled={sending || deleting}
                  className="text-muted-foreground hover:bg-muted hover:text-foreground"
                  onClick={(event) => {
                    event.stopPropagation();
                    void onForkThread();
                  }}
                >
                  <GitForkIcon aria-hidden="true" />
                </Button>
              ) : null}
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                title={`Delete “${title}”`}
                aria-label={`Delete “${title}”`}
                disabled={deleting}
                className="text-muted-foreground hover:bg-destructive/15 hover:text-destructive"
                onClick={(event) => {
                  event.stopPropagation();
                  setPendingDelete({
                    thread,
                    projectPath: project.path,
                  });
                }}
              >
                <Trash2Icon aria-hidden="true" />
              </Button>
            </div>
          )}
        </div>
        {children ? renderDelegationChildren(title, children) : null}
      </li>
    );
  }

  /**
   * Jobs a chat dispatched, hung off its row by a tree line that starts under
   * the row's mark. Each opens the Workbench at that job.
   */
  function renderDelegationChildren(parentTitle: string, children: DelegationChildren) {
    const more = children.hidden[0];
    const rows = more ? [...children.shown, more] : children.shown;
    return (
      <ul aria-label={`Jobs started from ${parentTitle}`} className="m-0 flex list-none flex-col p-0">
        {rows.map((job, index) => {
          const last = index === rows.length - 1;
          const isMore = more !== undefined && last;
          const status = delegationChildStatus(job.status);
          return (
            <li key={isMore ? `delegation-more:${job.jobId}` : `delegation:${job.jobId}`} className="relative pl-[26px]">
              <span
                aria-hidden="true"
                className="pointer-events-none absolute left-[15px] top-0 h-1/2 w-2 rounded-bl-[5px] border-b border-l border-border"
              />
              {last ? null : (
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute bottom-0 left-[15px] top-1/2 border-l border-border"
                />
              )}
              {isMore ? (
                <button
                  type="button"
                  onClick={() => onOpenDelegationJob?.(job.jobId)}
                  aria-label={`${children.hidden.length} more ${children.hidden.length === 1 ? "job" : "jobs"} from ${parentTitle}, in the Workbench`}
                  className="flex h-6 w-full min-w-0 cursor-pointer items-center rounded-md pl-1 pr-2 text-left text-[11px] text-muted-foreground outline-none transition-colors hover:bg-[var(--sidebar-accent)] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
                >
                  {children.hidden.length} more
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => onOpenDelegationJob?.(job.jobId)}
                  title={`${job.title} · ${status.label}. Opens in the Workbench.`}
                  aria-label={`${job.title}. Delegated job, ${status.label}`}
                  className="flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-md pl-1 pr-2 text-left text-[12px] text-muted-foreground outline-none transition-colors hover:bg-[var(--sidebar-accent)] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
                >
                  <DelegationChildIcon tone={status.tone} />
                  <span
                    className={cn(
                      "min-w-0 flex-1 truncate",
                      status.tone === "running" && WORKING_TITLE_CLASS
                    )}
                  >
                    {job.title}
                  </span>
                  <span className="shrink-0 text-[10px]">{status.label}</span>
                </button>
              )}
            </li>
          );
        })}
      </ul>
    );
  }

  return (
    <aside
      className={cn(
        "relative flex h-full shrink-0 flex-col border-r border-border/60 bg-[var(--sidebar)] text-[var(--sidebar-foreground)] transition-[width] duration-200 ease-out",
        open ? "w-[260px]" : "w-11"
      )}
    >
      <div
        className={cn(
          "flex h-10 shrink-0 items-center border-b border-border/60",
          open ? "justify-end gap-1 px-2" : "justify-center px-1"
        )}
      >
        {open ? (
          <div className="flex w-full items-center justify-between gap-1">
            <div className="flex items-center gap-0.5">
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title="Back"
                aria-label="Back"
                disabled={!canNavigateBack}
                onClick={onNavigateBack}
              >
                <ArrowLeftIcon aria-hidden="true" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title="Forward"
                aria-label="Forward"
                disabled={!canNavigateForward}
                onClick={onNavigateForward}
              >
                <ArrowRightIcon aria-hidden="true" />
              </Button>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title="Collapse sidebar"
              aria-label="Collapse sidebar"
              aria-expanded={open}
              onClick={() => onOpenChange(false)}
            >
              <PanelLeftIcon aria-hidden="true" />
            </Button>
          </div>
        ) : (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Expand sidebar"
            aria-label="Expand sidebar"
            aria-expanded={open}
            onClick={() => onOpenChange(true)}
          >
            <PanelLeftIcon aria-hidden="true" />
          </Button>
        )}
      </div>

      {!open ? (
        <div className="flex flex-col items-center gap-1 px-1 py-2">
          {onOpenPullRequests ? (
            <Button type="button" variant="ghost" size="icon-sm" title="Pull requests" aria-label="Pull requests" aria-current={pullRequestsActive ? "page" : undefined} onClick={onOpenPullRequests}>
              <GitPullRequestIcon aria-hidden="true" />
            </Button>
          ) : null}
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="New chat (Ctrl+N)"
            onClick={onNewChat}
          >
            <PlusIcon />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title={`Search (${formatChord("Mod+K")})`}
            aria-label="Search"
            onClick={() => onSearch()}
          >
            <SearchIcon />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            title="Open project folder"
            onClick={onOpenFolder}
          >
            <FolderOpenIcon />
          </Button>
        </div>
      ) : null}

      {open ? (
        <div className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1.5">
          <nav aria-label="Primary" className="flex flex-col gap-0.5">
            <button
              type="button"
              onClick={onNewChat}
              className={cn(
                navItemClass(),
                "disabled:pointer-events-none disabled:opacity-50"
              )}
            >
              <SquarePenIcon className="size-4 shrink-0 text-muted-foreground" />
              <span>New chat</span>
            </button>
            <button
              type="button"
              title={`Search (${formatChord("Mod+K")})`}
              onClick={() => onSearch()}
              className={navItemClass()}
            >
              <SearchIcon className="size-4 shrink-0 text-muted-foreground" />
              <span>Search</span>
            </button>
            {onOpenPullRequests ? (
              <button type="button" onClick={onOpenPullRequests}
                aria-current={pullRequestsActive ? "page" : undefined}
                className={navItemClass(pullRequestsActive)}>
                <GitPullRequestIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <span>Pull requests</span>
              </button>
            ) : null}
            {onOpenCustomize ? (
              <button
                type="button"
                onClick={onOpenCustomize}
                aria-current={customizeActive ? "page" : undefined}
                className={navItemClass(customizeActive)}
              >
                <SlidersHorizontalIcon className="size-4 shrink-0 text-muted-foreground" />
                <span>Customize</span>
              </button>
            ) : null}
          </nav>

          <div className="my-2 border-t border-border/40" />

          {splitGroups.length > 0 ? (
            <section aria-labelledby="split-groups-heading" className="mb-3 flex flex-col gap-1">
              <div className="flex items-center gap-1.5 px-2 pb-0.5 text-[10px] font-medium uppercase tracking-[0.1em] text-muted-foreground">
                <Columns2Icon className="size-3.5" aria-hidden="true" />
                <h2 id="split-groups-heading" className="m-0">Split views</h2>
              </div>
              <ul className="m-0 flex list-none flex-col gap-1 p-0">
                {splitGroups.map((group) => {
                  const active = group.active || activeSplitGroupId === group.id;
                  return (
                    <li key={`split-group:${group.id}`} className="min-w-0">
                      <button
                        type="button"
                        aria-current={active ? "page" : undefined}
                        aria-label={`${group.label} with ${group.panes.length} chats`}
                        onClick={() => onOpenSplitGroup?.(group.id)}
                        className={cn(
                          "flex h-7 w-full min-w-0 items-center gap-1.5 rounded-md border-l-2 px-1.5 text-left text-[12px] outline-none transition-colors",
                          active
                            ? "border-[var(--sidebar-primary)] bg-[var(--sidebar-accent)] text-foreground"
                            : "border-transparent text-muted-foreground hover:bg-[var(--sidebar-accent)] hover:text-foreground",
                          "focus-visible:ring-2 focus-visible:ring-ring/50"
                        )}
                      >
                        <Columns2Icon className={cn("size-3.5 shrink-0", active && "text-[var(--sidebar-primary)]")} aria-hidden="true" />
                        <span className="min-w-0 flex-1 truncate">{group.label}</span>
                        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">{group.panes.length}</span>
                      </button>
                      {group.panes.length > 0 ? (
                        <ul className="m-0 mt-0.5 flex list-none flex-col gap-px p-0 pl-4">
                          {group.panes.map((pane) => (
                            <li key={`split-pane:${group.id}:${pane.paneId}`}>
                              <button
                                type="button"
                                aria-label={`Open ${pane.title} in ${group.label}`}
                                onClick={() => onFocusSplitPane?.(group.id, pane.paneId)}
                                className={cn(
                                  "flex min-w-0 w-full flex-col rounded-md px-2 py-1 text-left outline-none transition-colors hover:bg-[var(--sidebar-accent)] focus-visible:ring-2 focus-visible:ring-ring/50",
                                  active && pane.threadId === activeThreadId && "bg-[var(--sidebar-accent)] text-foreground"
                                )}
                              >
                                <span className="truncate text-[12px]">{pane.title}</span>
                                <span className="truncate text-[10px] text-muted-foreground">{pane.projectLabel}</span>
                              </button>
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            </section>
          ) : null}

          {loading && projects.length === 0 ? (
            <p className="px-2 py-1 text-xs text-muted-foreground">Loading…</p>
          ) : error ? (
            <p className="px-2 py-1 text-xs text-destructive">{error}</p>
          ) : chatViewMode === "compact" ? (
            // One heading tier: the project name is the heading, or the
            // status when grouping by status. There is no "Chats" label above
            // it, and a project with no chats is just its heading and its +
            // button.
            <div className="flex flex-col gap-3">
              <div
                role="group"
                aria-label="Group chats"
                className="mx-1 grid grid-cols-2 gap-0.5 rounded-md bg-[color-mix(in_srgb,var(--sidebar-accent)_70%,transparent)] p-0.5"
              >
                {(["project", "status"] as const).map((value) => (
                  <button
                    key={value}
                    type="button"
                    aria-pressed={grouping === value}
                    onClick={() => chooseGrouping(value)}
                    className={cn(
                      "h-6 cursor-pointer rounded px-2 text-[11px] font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50",
                      grouping === value
                        ? "bg-[var(--sidebar)] text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground"
                    )}
                  >
                    {value === "project" ? "By project" : "By status"}
                  </button>
                ))}
              </div>
              {projectError ? (
                <p className="m-0 px-2 text-[11px] text-destructive">{projectError}</p>
              ) : null}
              {renameError ? (
                <p className="m-0 px-2 text-[11px] text-destructive">{renameError}</p>
              ) : null}

              {grouping === "status" ? (
                CHAT_STATUS_SECTIONS.map((section) => {
                  const rows = statusGroups[section];
                  if (rows.length === 0) return null;
                  const { label, Icon, iconClass } = STATUS_SECTION_META[section];
                  return (
                    <section
                      key={`compact-status:${section}`}
                      aria-labelledby={`compact-status-heading:${section}`}
                      className="flex flex-col"
                    >
                      <div className="flex h-7 items-center gap-1.5 pl-2 pr-2">
                        <Icon className={cn("size-3.5 shrink-0", iconClass)} aria-hidden />
                        <h2
                          id={`compact-status-heading:${section}`}
                          className="m-0 min-w-0 flex-1 truncate text-[12px] font-medium text-muted-foreground"
                        >
                          {label}
                        </h2>
                        <span
                          className="shrink-0 text-[10px] tabular-nums text-muted-foreground/70"
                          aria-hidden="true"
                        >
                          {rows.length}
                        </span>
                      </div>
                      <ul className="m-0 flex list-none flex-col gap-px p-0">
                        {rows.map(({ project, thread }) =>
                          renderCompactThreadItem(project, thread, `compact:status:${thread.id}`, {
                            projectLabel: chatProjectLabel(project),
                          })
                        )}
                      </ul>
                    </section>
                  );
                })
              ) : (
                <>
                  {recentThreads.length > 0 ? (
                    <section aria-labelledby="compact-free-heading" className="flex flex-col">
                      <div className="flex h-7 items-center gap-0.5 pl-2 pr-0.5">
                        <h2
                          id="compact-free-heading"
                          className="m-0 min-w-0 flex-1 truncate text-[12px] font-medium text-muted-foreground"
                        >
                          No project
                        </h2>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          title="New chat without a project"
                          aria-label="New chat without a project"
                          className="shrink-0 text-muted-foreground"
                          onClick={onNewChat}
                        >
                          <PlusIcon aria-hidden="true" />
                        </Button>
                      </div>
                      <ul className="m-0 flex list-none flex-col gap-px p-0">
                        {recentThreads.map(({ project, thread }) =>
                          renderCompactThreadItem(project, thread, `compact:free:${thread.id}`)
                        )}
                      </ul>
                    </section>
                  ) : null}

                  {visibleProjects.map((project) => (
                    <section
                      key={`compact-project:${project.path}`}
                      aria-labelledby={`compact-project-heading:${project.path}`}
                      className="flex flex-col"
                    >
                      <div className="group/project relative flex h-7 items-center gap-0.5 pl-2 pr-0.5">
                        <h2 className="m-0 min-w-0 flex-1 text-[12px] font-medium">
                          <button
                            type="button"
                            id={`compact-project-heading:${project.path}`}
                            title={project.path}
                            aria-current={project.active ? "true" : undefined}
                            onClick={() => {
                              if (!project.active) {
                                void onOpenProjectChat({ root: project.path });
                              }
                            }}
                            className={cn(
                              "block max-w-full cursor-pointer truncate rounded-sm text-left outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50",
                              project.active ? "text-foreground/85" : "text-muted-foreground"
                            )}
                          >
                            {project.name}
                          </button>
                        </h2>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          title={`Project options for ${project.name}`}
                          aria-label={`Project options for ${project.name}`}
                          className="shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/project:opacity-100 focus-visible:opacity-100"
                          onClick={() => {
                            setProjectError(null);
                            setProjectMenuPath((path) =>
                              path === project.path ? null : project.path
                            );
                          }}
                        >
                          <MoreHorizontalIcon />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          title={`New chat in ${project.name}`}
                          aria-label={`New chat in ${project.name}`}
                          className="shrink-0 text-muted-foreground"
                          onClick={() => {
                            void onOpenProjectChat({ root: project.path, newThread: true });
                          }}
                        >
                          <PlusIcon aria-hidden="true" />
                        </Button>
                        {projectMenuPath === project.path ? (
                          <div
                            ref={projectMenuRef}
                            className="absolute right-0 top-8 z-30 w-[238px] rounded-lg border border-border/80 bg-popover p-1.5 text-popover-foreground shadow-xl"
                          >
                            <div className="flex items-center gap-2 px-2 pb-1.5">
                              <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                              <div className="min-w-0">
                                <div className="truncate text-xs font-medium text-foreground">
                                  {project.name}
                                </div>
                                <div className="truncate text-[10px] text-muted-foreground">
                                  Project actions
                                </div>
                              </div>
                            </div>
                            <button
                              type="button"
                              disabled={projectBusy || project.active}
                              title={
                                project.active
                                  ? "Switch projects before removing the active workspace"
                                  : "Keep the folder and chats on disk"
                              }
                              className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-xs transition-colors hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-45"
                              onClick={() => {
                                setProjectMenuPath(null);
                                setWorkspaceAction({ project });
                              }}
                            >
                              <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                              <span className="min-w-0 flex-1 truncate">Remove from Zest</span>
                            </button>
                            {project.active ? (
                              <p className="px-2 pt-1.5 text-[10px] leading-4 text-muted-foreground">
                                Switch projects before managing this folder.
                              </p>
                            ) : null}
                          </div>
                        ) : null}
                      </div>
                      {project.threads.length > 0 ? (
                        // Backend order: pinned first, then most recently updated.
                        <ul className="m-0 flex list-none flex-col gap-px p-0">
                          {project.threads.map((thread) =>
                            renderCompactThreadItem(
                              project,
                              thread,
                              `compact:project:${project.path}:${thread.id}`
                            )
                          )}
                        </ul>
                      ) : null}
                    </section>
                  ))}
                </>
              )}

              <button
                type="button"
                onClick={onOpenFolder}
                className="flex h-7 items-center gap-2 rounded-md px-2 text-left text-[12px] font-medium text-muted-foreground outline-none transition-colors hover:bg-[var(--sidebar-accent)] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
              >
                <FolderPlusIcon className="size-3.5 shrink-0" aria-hidden="true" />
                Open a project folder
              </button>
            </div>
          ) : (
            <>
              <section aria-labelledby="projects-heading" className="flex flex-col gap-2">
                <div className="flex items-center justify-between px-1">
                  <div className="flex min-w-0 items-center gap-1.5">
                    <FolderIcon className="size-3.5 text-muted-foreground" />
                    <h2
                      id="projects-heading"
                      className="m-0 text-[10px] font-medium uppercase tracking-[0.1em] text-muted-foreground"
                    >
                      Projects
                    </h2>
                    <span className="text-[10px] tabular-nums text-muted-foreground/60">
                      {visibleProjects.length}
                    </span>
                  </div>
                  <div className="flex shrink-0 items-center gap-0.5">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      title={`Search chats (${formatChord("Mod+K")})`}
                      aria-label="Search chats"
                      onClick={() => onSearch("chats")}
                    >
                      <SearchIcon aria-hidden="true" />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      title="Open project folder"
                      aria-label="Open project folder"
                      onClick={onOpenFolder}
                    >
                      <PlusIcon aria-hidden="true" />
                    </Button>
                  </div>
                </div>
                {projectError ? (
                  <p className="px-2 pb-1 text-[11px] text-destructive">{projectError}</p>
                ) : null}
                {renameError ? (
                  <p className="px-2 pb-1 text-[11px] text-destructive">{renameError}</p>
                ) : null}
                {visibleProjects.length === 0 ? (
                  <p className="px-2 py-1 text-xs text-muted-foreground">
                    Open a project folder to get started.
                  </p>
                ) : (
                  <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
                    {visibleProjects.map((project) => {
                      const expandedHere = isExpanded(project);
                      const folderClass = cn(
                        "size-3.5 shrink-0",
                        project.active
                          ? "text-[var(--sidebar-primary)]"
                          : "text-muted-foreground"
                      );
                      return (
                        <li key={project.path} className="relative min-w-0">
                        <div className="group/project flex items-center gap-0.5">
                          <button
                            type="button"
                            title={project.path}
                            onClick={() => {
                              toggleExpanded(project.path);
                              if (!project.active) {
                                void onOpenProjectChat({ root: project.path });
                              }
                            }}
                            className={cn(
                              "flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 rounded-md px-1.5 py-1 text-left outline-none transition-colors",
                              "hover:bg-[var(--sidebar-accent)] focus-visible:ring-2 focus-visible:ring-ring/50"
                            )}
                          >
                            <ChevronRightIcon
                              className={cn(
                                "size-3 shrink-0 text-muted-foreground transition-transform",
                                expandedHere && "rotate-90"
                              )}
                            />
                            {expandedHere ? (
                              <FolderOpenIcon className={folderClass} />
                            ) : (
                              <FolderIcon className={folderClass} />
                            )}
                            <span className="truncate text-[13px] font-medium">
                              {project.name}
                            </span>
                          </button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-xs"
                            title={`Project options for ${project.name}`}
                            aria-label={`Project options for ${project.name}`}
                            className="shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/project:opacity-100 focus-visible:opacity-100"
                            onClick={() => {
                              setProjectError(null);
                              setProjectMenuPath((path) =>
                                path === project.path ? null : project.path
                              );
                            }}
                          >
                            <MoreHorizontalIcon />
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-xs"
                            title={`New chat in ${project.name}`}
                            className="shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/project:opacity-100 focus-visible:opacity-100"
                            onClick={() => {
                              void onOpenProjectChat({
                                root: project.path,
                                newThread: true,
                              });
                            }}
                          >
                            <PlusIcon />
                          </Button>
                        </div>

                        {projectMenuPath === project.path ? (
                          <div
                            ref={projectMenuRef}
                            className="absolute right-0 top-9 z-30 w-[238px] rounded-lg border border-border/80 bg-popover p-1.5 text-popover-foreground shadow-xl"
                          >
                            <div className="flex items-center gap-2 px-2 pb-1.5">
                              <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                              <div className="min-w-0">
                                <div className="truncate text-xs font-medium text-foreground">
                                  {project.name}
                                </div>
                                <div className="truncate text-[10px] text-muted-foreground">
                                  Project actions
                                </div>
                              </div>
                            </div>
                            <div className="flex flex-col gap-0.5">
                              <button
                                type="button"
                                disabled={projectBusy || project.active}
                                title={
                                  project.active
                                    ? "Switch projects before removing the active workspace"
                                    : "Keep the folder and chats on disk"
                                }
                                className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-xs transition-colors hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-45"
                                onClick={() => {
                                  setProjectMenuPath(null);
                                  setWorkspaceAction({ project });
                                }}
                              >
                                <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                                <span className="min-w-0 flex-1 truncate">Remove from Zest</span>
                              </button>
                            </div>
                            {project.active ? (
                              <p className="px-2 pt-1.5 text-[10px] leading-4 text-muted-foreground">
                                Switch projects before managing this folder.
                              </p>
                            ) : null}
                          </div>
                        ) : null}

                        {expandedHere ? (
                          <ul className="m-0 mt-0.5 mb-0.5 flex list-none flex-col gap-0.5 p-0 pl-3">
                            {project.threads.length === 0 ? (
                              <li className="px-2 py-1 text-[11px] text-muted-foreground/80">
                                No chats yet
                              </li>
                            ) : (
                              project.threads.map((thread) =>
                                renderThreadItem(
                                  project,
                                  thread,
                                  `project:${project.path}:${thread.id}`
                                )
                              )
                            )}
                          </ul>
                        ) : null}
                        </li>
                      );
                    })}
                  </ul>
                )}
              </section>

              {recentThreads.length > 0 ? (
                <section aria-labelledby="recent-chats-heading" className="mt-3">
                  <div
                    id="recent-chats-heading"
                    className="flex items-center gap-1.5 px-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground"
                  >
                    <Clock3Icon className="size-3.5" />
                    Recent
                  </div>
                  <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
                    {recentThreads.map(({ project, thread }) =>
                      renderThreadItem(
                        project,
                        thread,
                        `recent:${project.path}:${thread.id}`
                      )
                    )}
                  </ul>
                </section>
              ) : null}
            </>
          )}

        </div>
      ) : null}

      {/*
       * Who you are sits at the bottom, under the chats. It used to be the
       * first thing in the chat header, where it competed with the project and
       * branch the header is actually for.
       */}
      <div className="relative z-20 mt-auto shrink-0 border-t border-border/60 p-1.5">
        <div className={open ? "flex items-center gap-0.5" : "flex flex-col items-center gap-1"}>
          <AccountMenu
            profile={profile}
            providerLabel={providerLabel}
            compact={!open}
            profileActive={profileActive}
            onOpenProfile={onOpenProfile}
            onOpenSettings={onOpenSettings}
          />
          <AgentQuotaButton
            providers={providers}
            refreshKey={quotaRefreshKey}
            placement="above"
          />
        </div>
      </div>

      <ConfirmDialog
        open={workspaceAction != null}
        title="Remove project from Zest?"
        description={
          workspaceAction
            ? `${workspaceAction.project.name} will disappear from Zest's Projects list. Its folder and chats will stay on disk.`
            : ""
        }
        confirmLabel="Remove"
        cancelLabel="Cancel"
        busy={projectBusy}
        onCancel={() => {
          if (!projectBusy) setWorkspaceAction(null);
        }}
        onConfirm={() => {
          void confirmWorkspaceAction();
        }}
      />

      <ConfirmDialog
        open={pendingDelete != null}
        title="Delete chat?"
        description={
          pendingDelete
            ? `“${threadTitle(pendingDelete.thread)}” will be permanently removed.`
            : ""
        }
        confirmLabel="Delete"
        cancelLabel="Cancel"
        destructive
        busy={deleting}
        onCancel={() => {
          if (!deleting) setPendingDelete(null);
        }}
        onConfirm={() => {
          void confirmDelete();
        }}
      />
    </aside>
  );
});
