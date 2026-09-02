import { memo, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  Clock3Icon,
  ChevronRightIcon,
  ChevronsLeftIcon,
  ChevronsRightIcon,
  FolderIcon,
  FolderOpenIcon,
  GitBranchIcon,
  GitForkIcon,
  GitPullRequestIcon,
  MoreHorizontalIcon,
  PinIcon,
  PlusIcon,
  SearchIcon,
  SlidersHorizontalIcon,
  SquarePenIcon,
  Trash2Icon,
} from "lucide-react";

import { AgentQuotaButton } from "@/components/AgentQuotaButton";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ProviderIcon } from "@/components/ProviderIcon";
import { UserAvatar, UserAvatarButton } from "@/components/UserAvatarButton";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { isBooleanRecord, parseJson } from "@/lib/json";
import { formatChord } from "@/lib/keybindings";
import {
  elapsedLabel,
  type ThreadActivity,
  type ThreadActivityMap,
} from "@/lib/threadActivity";
import type { ProjectChats, ProviderRow, ThreadSummary, UserProfile } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  handlePullRequestClick,
  pullRequestAnchorProps,
} from "@/lib/pullRequestLink";

type Props = {
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
  /** The profile screen is showing. */
  profileActive?: boolean;
  providers: ProviderRow[];
  quotaRefreshKey: string | number;
  /** Open the command palette (Search in the sidebar). */
  onSearch: () => void;
  /** Show the transcript when a shell panel is covering the active chat. */
  onRevealTranscript: () => void;
  canNavigateBack: boolean;
  canNavigateForward: boolean;
  onNavigateBack: () => void;
  onNavigateForward: () => void;
  /** Open this chat's pull request in the review pane. */
  onOpenPullRequest?: (project: ProjectChats, thread: ThreadSummary) => void;
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
  return activity.state === "awaiting_approval" ? "Needs approval" : "Working";
}

function activityDescription(activity: ThreadActivity, now: number) {
  const parts = [activityStateLabel(activity)];
  if (activity.tool) parts.push(`running ${activity.tool.replaceAll("_", " ")}`);
  const elapsed = elapsedLabel(activity.startedAt, now);
  if (elapsed) parts.push(`for ${elapsed}`);
  return parts.join(", ");
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
  const [now, setNow] = useState(() => Date.now());
  const [projectMenuPath, setProjectMenuPath] = useState<string | null>(null);
  const [workspaceAction, setWorkspaceAction] = useState<WorkspaceAction | null>(null);
  const [projectBusy, setProjectBusy] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);

  const hasActiveActivity = Object.values(threadActivity).some(
    (activity) => activity.state !== "idle"
  );

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
      <li key={key} className="group/thread relative">
        {isEditing ? (
          <div
            role="group"
            aria-label={`Renaming ${title}`}
            className="flex w-full cursor-text items-center gap-2 rounded-md bg-[var(--sidebar-accent)] py-1 pr-24 pl-2 text-left outline-none"
          >
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
              ignoreExpectedFailure(error, "open chat from history")
            );
          }}
          onDoubleClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            beginRename(project, thread, key);
          }}
          title={`Double-click to rename “${title}”`}
          aria-label={[title, activityText, gitText].filter(Boolean).join(". ")}
          className={cn(
            "flex w-full cursor-pointer items-center gap-2 rounded-md py-1 pr-24 pl-2 text-left outline-none transition-colors",
            "hover:bg-[var(--sidebar-accent)] hover:text-[var(--sidebar-accent-foreground)]",
            "focus-visible:ring-2 focus-visible:ring-ring/50",
            active
              ? "bg-[var(--sidebar-accent)] text-[var(--sidebar-accent-foreground)]"
              : ""
          )}
        >
          <span className="min-w-0 flex-1 truncate text-[13px]">{title}</span>
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
          {owner ? (
            <span
              title={`This chat belongs to ${owner}. Zest will keep the original provider or let you open a copy.`}
              className="flex shrink-0 items-center text-muted-foreground"
            >
              {/* The name still reaches a screen reader through the title
                  above, so the glyph itself stays decorative. */}
              <ProviderIcon
                providerId={owner}
                label={owner}
                className="size-4 opacity-80"
              />
            </span>
          ) : null}
          {activity && activity.state !== "idle" ? (
            <span
              className="flex shrink-0 items-center gap-1 text-[10px] tabular-nums text-muted-foreground"
              aria-hidden="true"
            >
              {activity.state === "awaiting_approval" ? (
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
        )}
        {/* One flex row rather than three fixed offsets. Fork only exists on
            the active chat, and pinning it to its own `right-7` slot meant
            every other row rendered that slot empty — a hole between the pin
            and the bin that read as a missing button. Packing them lets the
            row close up when fork is absent. */}
        <div className="absolute top-1 right-0.5 flex items-center gap-0.5">
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            title={thread.pinned ? "Unpin chat" : "Pin chat"}
            aria-label={thread.pinned ? "Unpin chat" : "Pin chat"}
            aria-pressed={thread.pinned}
            disabled={deleting || pinning === thread.id || (sending && active)}
            className={cn(
              "text-muted-foreground transition-opacity",
              "hover:bg-muted hover:text-foreground",
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
          {active ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              title="Fork conversation"
              aria-label="Fork conversation"
              disabled={sending || deleting}
              className={cn(
                "text-muted-foreground transition-opacity",
                "hover:bg-muted hover:text-foreground",
                "opacity-100 focus-visible:opacity-100"
              )}
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
            disabled={deleting}
            className={cn(
              "text-muted-foreground transition-opacity",
              "hover:bg-destructive/15 hover:text-destructive",
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
      </li>
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
              aria-expanded={open}
              onClick={() => onOpenChange(false)}
            >
              <ChevronsLeftIcon />
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
            <ChevronsRightIcon aria-hidden="true" />
          </Button>
        )}
      </div>

      {!open ? (
        <div className="flex flex-col items-center gap-1 px-1 py-2">
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
            onClick={onSearch}
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
              onClick={onSearch}
              className={navItemClass()}
            >
              <SearchIcon className="size-4 shrink-0 text-muted-foreground" />
              <span>Search</span>
            </button>
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

          {loading && projects.length === 0 ? (
            <p className="px-2 py-1 text-xs text-muted-foreground">Loading…</p>
          ) : error ? (
            <p className="px-2 py-1 text-xs text-destructive">{error}</p>
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
                              "hover:bg-[var(--sidebar-accent)] focus-visible:ring-2 focus-visible:ring-ring/50",
                              project.active && "bg-[var(--sidebar-accent)]"
                            )}
                          >
                            <ChevronRightIcon
                              className={cn(
                                "size-3 shrink-0 text-muted-foreground transition-transform",
                                expandedHere && "rotate-90"
                              )}
                            />
                            {expandedHere ? (
                              <FolderOpenIcon className="size-3.5 shrink-0 text-muted-foreground" />
                            ) : (
                              <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
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
                          <div className="absolute right-0 top-9 z-30 w-[238px] rounded-lg border border-border/80 bg-popover p-1.5 text-popover-foreground shadow-xl">
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
        {open ? (
          <div className="flex items-center gap-0.5">
            <button
              type="button"
              onClick={onOpenProfile}
              aria-current={profileActive ? "page" : undefined}
              title="Your profile"
              className={cn(
                "flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md px-1.5 py-1.5 text-left outline-none transition-colors",
                "hover:bg-[var(--sidebar-accent)] focus-visible:ring-2 focus-visible:ring-ring/50",
                profileActive && "bg-[var(--sidebar-accent)]"
              )}
            >
              <UserAvatar
                avatarDataUrl={profile.avatarDataUrl}
                displayName={profile.displayName}
                className="shrink-0"
              />
              <span className="min-w-0 flex-1 leading-tight">
                <span className="block truncate text-[13px] font-medium text-foreground">
                  {profile.displayName.trim() || "Zest"}
                </span>
                {providerLabel ? (
                  <span className="block truncate text-[11px] text-muted-foreground">
                    {providerLabel}
                  </span>
                ) : null}
              </span>
            </button>
            <AgentQuotaButton
              providers={providers}
              refreshKey={quotaRefreshKey}
              placement="above"
            />
          </div>
        ) : (
          <div className="flex flex-col items-center gap-1">
            <UserAvatarButton
              avatarDataUrl={profile.avatarDataUrl}
              displayName={profile.displayName}
              title="Your profile"
              onClick={() => onOpenProfile?.()}
            />
            <AgentQuotaButton
              providers={providers}
              refreshKey={quotaRefreshKey}
              placement="above"
            />
          </div>
        )}
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
