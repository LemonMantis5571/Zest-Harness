import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  CheckCircle2Icon,
  FileIcon,
  FileTextIcon,
  FolderOpenIcon,
  GitBranchIcon,
  ImageIcon,
  CommandIcon,
  LoaderCircleIcon,
  PanelRightOpenIcon,
  PencilIcon,
  SettingsIcon,
  TriangleAlertIcon,
  XCircleIcon,
  XIcon,
} from "lucide-react";

import {
  ChatHistorySidebar,
  readSidebarOpen,
  writeSidebarOpen,
} from "@/components/ChatHistorySidebar";
import { CommandOutputCard } from "@/components/CommandOutputCard";
import { CheckpointRail } from "@/components/CheckpointRail";
import { CommandPalette, type PaletteAction } from "@/components/CommandPalette";
import { AgentQuotaButton } from "@/components/AgentQuotaButton";
import { Composer } from "@/components/Composer";
import { DiffViewer, type DiffViewerTarget } from "@/components/DiffViewer";
import { MarkdownActions } from "@/components/MarkdownActions";
import { NeedsInputCard } from "@/components/NeedsInputCard";
import { PlanningQuestionnaire } from "@/components/PlanningQuestionnaire";
import { looksLikeDocument } from "@/lib/documentShape";
import { buildablePlanId } from "@/lib/planActions";
import { planningQuestionFor } from "@/lib/planningQuestion";
import { Markdown } from "@/components/Markdown";
import { NowPlayingButton } from "@/components/NowPlayingButton";
import { ProviderSwitchSheet } from "@/components/ProviderSwitchSheet";
import { SettingsPanel } from "@/components/SettingsPanel";
import { ToolCallRow } from "@/components/ToolCallRow";
import { ToolRunGroup } from "@/components/ToolRunGroup";
import { ThinkingTrace } from "@/components/ThinkingTrace";
import { UserAvatarButton } from "@/components/UserAvatarButton";
import { WorkbenchPanel } from "@/components/WorkbenchPanel";
import {
  Attachment,
  AttachmentContent,
  AttachmentGroup,
  AttachmentMedia,
  AttachmentTitle,
} from "@/components/ui/attachment";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Message, MessageContent } from "@/components/ui/message";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { ZestPulse } from "@/components/ZestPulse";
import { LinkifyText } from "@/lib/linkify";
import { sessionSupportsModelPicker, type EffortId } from "@/lib/models";
import { collapseThresholdFor, groupToolRuns } from "@/lib/toolRuns";
import type { ThreadActivityMap } from "@/lib/threadActivity";
import type { QueuedTurn } from "@/lib/threadQueue";
import { useKeybindings } from "@/lib/useKeybindings";
import type {
  ApprovalChoice,
  ApprovalMode,
  ChatMessage,
  DelegationJob,
  GitContext,
  PreparedAttachment,
  ProviderActivityPart,
  ProviderRow,
  SessionInfo,
  SessionWarning,
  UserProfile,
  WorkspaceChange,
  WorkspaceReview,
} from "@/lib/types";
import { cn } from "@/lib/utils";

function shortRoot(root: string): string {
  const cleaned = root.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
  const normalized = cleaned.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  if (parts.length <= 2) return cleaned;
  return parts.slice(-2).join("/");
}
type Props = {
  session: SessionInfo;
  messages: ChatMessage[];
  draft: string;
  attachments: PreparedAttachment[];
  branch: string | null;
  gitContext: GitContext | null;
  profile: UserProfile;
  sending: boolean;
  queuedMessages: ReadonlyArray<QueuedTurn>;
  onUpdateQueuedMessage: (turnId: string, text: string) => void;
  onRemoveQueuedMessage: (turnId: string) => void;
  threadActivity: ThreadActivityMap;
  model: string;
  effort: EffortId;
  onDraftChange: (value: string) => void;
  onSend: (text?: string) => void;
  onEditMessage: (messageId: string, text: string) => Promise<void>;
  onStop?: () => void;
  onNewChat: () => void;
  onForkThread: () => Promise<void>;
  onRewindThread: (checkpointId: string) => Promise<void>;
  workspaceReview: WorkspaceReview | null;
  workspaceChange: WorkspaceChange | null;
  onRefreshWorkspaceChanges: () => Promise<WorkspaceChange>;
  onVerifyWorkspace: () => Promise<void>;
  compacting?: boolean;
  onDeleteThread: (
    id: string,
    projectPath: string | null,
    freeChat: boolean
  ) => Promise<void>;
  onOpenProjectChat: (options: {
    root: string | null;
    threadId?: string;
    newThread?: boolean;
    providerId?: string;
    copyThread?: boolean;
  }) => Promise<boolean>;
  providers: ProviderRow[];
  onSwitchProvider: (providerId: string) => Promise<void>;
  onReloadSession?: () => Promise<void>;
  /** Re-run sign-in for a provider whose credentials the gateway rejected. */
  onReconnectProvider?: (providerId: string) => void;
  onRefreshProviders: () => Promise<void>;
  onReconnect: () => void;
  /** A background verification that failed after this chat opened. */
  sessionWarning?: SessionWarning | null;
  onDismissWarning?: () => void;
  onModelChange: (model: string) => void;
  onEffortChange: (effort: EffortId) => void;
  approvalMode: ApprovalMode;
  onApprovalModeChange: (mode: ApprovalMode) => void;
  /** Leave Plan mode and build the newest plan. */
  onBuildPlan?: () => void;
  /** Show the profile screen (avatar click). */
  onOpenProfile?: () => void;
  /** Show the usage screen. */
  onOpenUsage?: () => void;
  /**
   * Bumped to request the User section of Settings — the profile screen sends
   * edits here rather than duplicating the form.
   */
  settingsRequest?: number;
  onResolveApproval: (
    approvalId: string,
    decision: ApprovalChoice
  ) => Promise<void>;
  onResolveQuestion: (questionId: string, answer: string) => Promise<void>;
  onAttachFiles: () => void;
  onOpenFolder: () => void;
  onRemoveAttachment: (id: string) => void;
  onPasteImages: (files: File[]) => void;
  onProfileChange: (profile: UserProfile) => void;
  optionsDisabled?: boolean;
  delegationJobs: DelegationJob[];
  onApproveDelegation: (jobId: string) => Promise<void>;
  onCancelDelegation: (jobId: string) => Promise<void>;
  onRetryDelegation: (jobId: string) => Promise<void>;
  onApplyDelegation: (jobId: string) => Promise<void>;
};

function focusComposer() {
  const el = document.getElementById(
    "zest-composer-input"
  ) as HTMLTextAreaElement | null;
  if (!el) return;
  el.focus();
  const len = el.value.length;
  el.setSelectionRange(len, len);
}

type ChatMessageRowProps = {
  message: ChatMessage;
  isLast: boolean;
  sending: boolean;
  approvalMode: ApprovalMode;
  planToBuild: string | null;
  onBuildPlan?: () => void;
  onResolveApproval: (
    approvalId: string,
    decision: ApprovalChoice
  ) => Promise<void>;
  onOpenDiff: (path: string, diff: string) => void;
  onReconnectProvider?: (providerId: string) => void;
  onSend: (text?: string) => void;
  editing: boolean;
  editingText: string;
  editingBusy: boolean;
  onStartEdit: (messageId: string, text: string) => void;
  onChangeEdit: (text: string) => void;
  onCancelEdit: () => void;
  onSubmitEdit: () => void;
  onResolveQuestion: (questionId: string, answer: string) => Promise<void>;
  pinQuestion?: boolean;
};

type MessageEditFormProps = {
  value: string;
  busy: boolean;
  onChange: (value: string) => void;
  onCancel: () => void;
  onSubmit: () => void;
};

function MessageEditForm({
  value,
  busy,
  onChange,
  onCancel,
  onSubmit,
}: MessageEditFormProps) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (busy) return;
    const textarea = ref.current;
    if (!textarea) return;
    textarea.focus();
    const end = textarea.value.length;
    textarea.setSelectionRange(end, end);
  }, [busy]);

  return (
    <div className="w-full max-w-[42rem] overflow-hidden rounded-xl border border-border/80 bg-[var(--chat-header)]">
      <textarea
        ref={ref}
        value={value}
        rows={3}
        aria-label="Edit message"
        aria-busy={busy}
        disabled={busy}
        className="block max-h-[220px] min-h-[84px] w-full resize-y bg-transparent px-3.5 pt-3 text-sm leading-relaxed text-foreground outline-none placeholder:text-muted-foreground"
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            if (!busy) onCancel();
            return;
          }
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (!busy && value.trim()) onSubmit();
          }
        }}
      />
      <div className="flex items-center justify-end gap-1.5 px-2.5 pb-2.5">
        <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={busy || !value.trim()}
          onClick={onSubmit}
        >
          Send
        </Button>
      </div>
    </div>
  );
}

function ProviderActivityTrace({
  activities,
}: {
  activities: ProviderActivityPart[];
}) {
  if (activities.length === 0) return null;
  return (
    <div className="flex min-w-0 flex-col gap-1 text-xs text-muted-foreground" aria-label="Provider activity">
      <div className="text-[10px] font-medium uppercase tracking-[0.14em] text-muted-foreground/60">
        Claude Code
      </div>
      {activities.map((activity) => {
        const icon =
          activity.status === "done" ? (
            <CheckCircle2Icon className="size-3.5 shrink-0 text-primary/90" aria-hidden />
          ) : activity.status === "error" ? (
            <XCircleIcon className="size-3.5 shrink-0 text-destructive/90" aria-hidden />
          ) : (
            <LoaderCircleIcon className="size-3.5 shrink-0 animate-spin text-muted-foreground" aria-hidden />
          );
        return (
          <div key={activity.id} className="flex min-w-0 items-center gap-1.5">
            {icon}
            <span className="min-w-0 truncate text-foreground/75">{activity.title}</span>
            <span className="shrink-0 text-[10px] text-muted-foreground/60">
              {activity.status}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/**
 * Keep settled messages out of the streaming render path. Appending a delta
 * still updates the active row, but unchanged rows now retain their existing
 * React subtree instead of rebuilding every tool card and Markdown document.
 */
const ChatMessageRow = memo(function ChatMessageRow({
  message: msg,
  isLast,
  sending,
  approvalMode,
  planToBuild,
  onBuildPlan,
  onResolveApproval,
  onOpenDiff,
  onReconnectProvider,
  onSend,
  editing,
  editingText,
  editingBusy,
  onStartEdit,
  onChangeEdit,
  onCancelEdit,
  onSubmitEdit,
  onResolveQuestion,
  pinQuestion = false,
}: ChatMessageRowProps) {
  if (msg.role === "user") {
    if (editing) {
      return (
        <MessageScrollerItem id={`message-${msg.id}`} messageId={msg.id} scrollAnchor={isLast}>
          <Message align="end" className="justify-end">
            <MessageContent className="items-end gap-1.5">
              <MessageEditForm
                value={editingText}
                busy={editingBusy}
                onChange={onChangeEdit}
                onCancel={onCancelEdit}
                onSubmit={onSubmitEdit}
              />
            </MessageContent>
          </Message>
        </MessageScrollerItem>
      );
    }

    return (
      <MessageScrollerItem id={`message-${msg.id}`} messageId={msg.id} scrollAnchor={isLast}>
        <Message align="end" className="justify-end">
          <MessageContent className="items-end gap-1.5">
            <div className="group/user flex w-full flex-col items-end gap-1.5">
              {msg.attachments && msg.attachments.length > 0 ? (
                <AttachmentGroup className="justify-end">
                  {msg.attachments.map((att) => (
                    <Attachment key={`${msg.id}-${att.name}`} size="sm">
                      <AttachmentMedia variant="icon">
                        {att.kind === "pdf" ? (
                          <FileTextIcon />
                        ) : att.kind === "image" ? (
                          <ImageIcon />
                        ) : (
                          <FileIcon />
                        )}
                      </AttachmentMedia>
                      <AttachmentContent>
                        <AttachmentTitle>{att.name}</AttachmentTitle>
                      </AttachmentContent>
                    </Attachment>
                  ))}
                </AttachmentGroup>
              ) : null}
              {msg.text.trim() ? (
                <Bubble variant="secondary" align="end" className="max-w-[85%]">
                  <BubbleContent className="whitespace-pre-wrap bg-[var(--user-bubble)] text-[13.5px] leading-relaxed text-foreground">
                    <LinkifyText text={msg.text} />
                  </BubbleContent>
                </Bubble>
              ) : null}
              {!sending && !(msg.attachments && msg.attachments.length > 0) ? (
                <div className="flex items-center gap-0.5 text-muted-foreground opacity-0 transition-opacity group-hover/user:opacity-100 focus-within:opacity-100">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    title="Edit message"
                    aria-label="Edit message"
                    onClick={() => onStartEdit(msg.id, msg.text)}
                  >
                    <PencilIcon />
                  </Button>
                </div>
              ) : null}
            </div>
          </MessageContent>
        </Message>
      </MessageScrollerItem>
    );
  }

  const structuredQuestion = isLast ? msg.question : undefined;
  const planningQuestion = pinQuestion
    ? null
    : structuredQuestion ?? (isLast ? planningQuestionFor(msg) : null);
  const submitQuestion = structuredQuestion?.questionId
    ? (answer: string) =>
        onResolveQuestion(structuredQuestion.questionId as string, answer)
    : onSend;

  return (
    <MessageScrollerItem id={`message-${msg.id}`} messageId={msg.id} scrollAnchor={isLast}>
      <Message align="start">
        <MessageContent className="w-full max-w-full gap-2.5">
          <div className="text-[11px] font-medium tracking-wide text-muted-foreground/80">
            Zest
          </div>

          {msg.tools.length > 0 ? (
            <div className="flex w-full max-w-full flex-col gap-0.5">
              {groupToolRuns(msg.tools, collapseThresholdFor(msg.tools)).map((run) =>
                run.kind === "group" ? (
                  <ToolRunGroup
                    key={`group-${run.tools[0].id}`}
                    tools={run.tools}
                    summary={run.summary}
                    onResolveApproval={onResolveApproval}
                    onOpenDiff={onOpenDiff}
                  />
                ) : (
                  <ToolCallRow
                    key={run.tool.id}
                    tool={run.tool}
                    onResolveApproval={onResolveApproval}
                    onOpenDiff={onOpenDiff}
                  />
                )
              )}
            </div>
          ) : null}

          {msg.providerActivity ? (
            <ProviderActivityTrace activities={msg.providerActivity} />
          ) : null}

          {msg.thinking ? (
            <ThinkingTrace thinking={msg.thinking} streaming={msg.streaming} />
          ) : null}

          {planningQuestion ? (
            <PlanningQuestionnaire
              question={planningQuestion}
              disabled={structuredQuestion ? false : sending}
              onSubmit={submitQuestion}
            />
          ) : msg.text ? (
            msg.command && looksLikeDocument(msg.text) ? (
              <CommandOutputCard
                command={msg.command}
                text={msg.text}
                streaming={msg.streaming}
                action={
                  msg.id === planToBuild && onBuildPlan
                    ? {
                        label: "Build plan",
                        hint:
                          approvalMode === "plan"
                            ? "Leaves Plan mode so the steps can run"
                            : undefined,
                        disabled: sending,
                        onClick: onBuildPlan,
                      }
                    : undefined
                }
              >
                <Markdown streaming={msg.streaming}>{msg.text}</Markdown>
                {msg.streaming ? (
                  <span className="ml-1.5 inline-flex items-center gap-1.5 align-middle">
                    <ZestPulse size={12} />
                    <span className="inline-block h-4 w-1.5 animate-pulse bg-foreground/70" />
                  </span>
                ) : null}
              </CommandOutputCard>
            ) : (
              <div className="group/assistant relative">
                <div className="relative">
                  <Markdown streaming={msg.streaming}>{msg.text}</Markdown>
                  {msg.streaming ? (
                    <span className="ml-1.5 inline-flex items-center gap-1.5 align-middle">
                      <ZestPulse size={12} />
                      <span className="inline-block h-4 w-1.5 animate-pulse bg-foreground/70" />
                    </span>
                  ) : null}
                </div>
                {!msg.streaming ? (
                  <div className="mt-2 flex items-center gap-0.5 text-muted-foreground opacity-70 transition-opacity hover:opacity-100 focus-within:opacity-100">
                    <MarkdownActions text={msg.text} />
                  </div>
                ) : null}
              </div>
            )
          ) : null}

          {msg.error ? (
            <Bubble variant="destructive" align="start">
              <BubbleContent>
                {msg.error}
                {msg.reconnectProvider && onReconnectProvider ? (
                  <div className="mt-2.5">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={() =>
                        onReconnectProvider(msg.reconnectProvider as string)
                      }
                    >
                      Reconnect {msg.reconnectProvider}
                    </Button>
                  </div>
                ) : null}
              </BubbleContent>
            </Bubble>
          ) : null}

          {msg.streaming &&
          !msg.text &&
          !msg.thinking &&
          msg.tools.length === 0 ? (
            <ThinkingTrace thinking="" streaming emptyLabel="Thinking..." />
          ) : null}

          {msg.streaming &&
          !msg.text &&
          msg.tools.length > 0 &&
          !msg.tools.some(
            (tool) =>
              tool.status === "running" || tool.status === "awaiting_approval"
          ) ? (
            <ThinkingTrace thinking="" streaming emptyLabel="Working..." />
          ) : null}
        </MessageContent>
      </Message>
    </MessageScrollerItem>
  );
});

export function ChatScreen({
  session,
  messages,
  draft,
  attachments,
  branch,
  gitContext,
  profile,
  sending,
  queuedMessages,
  onUpdateQueuedMessage,
  onRemoveQueuedMessage,
  threadActivity,
  model,
  effort,
  onDraftChange,
  onSend,
  onEditMessage,
  onStop,
  onNewChat,
  onForkThread,
  onRewindThread,
  workspaceReview,
  workspaceChange,
  onRefreshWorkspaceChanges,
  onVerifyWorkspace,
  compacting = false,
  onDeleteThread,
  onOpenProjectChat,
  providers,
  onSwitchProvider,
  onReloadSession,
  onReconnectProvider,
  onRefreshProviders,
  onReconnect,
  sessionWarning = null,
  onDismissWarning,
  onModelChange,
  onEffortChange,
  approvalMode,
  onApprovalModeChange,
  onBuildPlan,
  onResolveApproval,
  onResolveQuestion,
  onAttachFiles,
  onOpenFolder,
  onRemoveAttachment,
  onPasteImages,
  onProfileChange,
  onOpenProfile,
  onOpenUsage,
  settingsRequest = 0,
  optionsDisabled = false,
  delegationJobs,
  onApproveDelegation,
  onCancelDelegation,
  onRetryDelegation,
  onApplyDelegation,
}: Props) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [focusUser, setFocusUser] = useState(false);
  /** Bumped to open Settings with the Keyboard shortcuts section expanded. */
  const [shortcutsRequest, setShortcutsRequest] = useState(0);
  const [sidebarOpen, setSidebarOpen] = useState(readSidebarOpen);
  const [diffTarget, setDiffTarget] = useState<DiffViewerTarget | null>(null);
  const workspaceRefreshRef = useRef<{
    threadId: string;
    promise: Promise<WorkspaceChange>;
  } | null>(null);
  const diffWidthKey = `zest:diff-width:${session.threadId}`;
  const diffOpenKey = `zest:diff-open:${session.threadId}`;
  const dismissedDiffKey = `zest:dismissed-change:${session.threadId}`;
  const [diffWidth, setDiffWidth] = useState(() => {
    if (typeof window === "undefined") return 520;
    const saved = Number(window.localStorage.getItem(diffWidthKey));
    return Number.isFinite(saved) && saved >= 360 ? saved : 520;
  });
  const [dismissedChangeId, setDismissedChangeId] = useState<string | null>(() =>
    typeof window === "undefined" ? null : window.localStorage.getItem(dismissedDiffKey)
  );
  const rememberDiffOpen = useCallback(
    (open: boolean) => {
      if (typeof window !== "undefined") {
        window.localStorage.setItem(diffOpenKey, String(open));
      }
    },
    [diffOpenKey]
  );
  const [providerSwitchOpen, setProviderSwitchOpen] = useState(false);
  const [providerSwitchBusy, setProviderSwitchBusy] = useState(false);
  /**
   * The Workbench opens only when asked for.
   *
   * It used to open itself whenever a tool started running, which meant a
   * working turn threw a panel over the transcript unprompted — and since the
   * trigger changed on every tool call, closing it only lasted until the next
   * one. A review surface is something you reach for, not something that
   * interrupts you.
   */
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [editingMessageText, setEditingMessageText] = useState("");
  const [editingMessageBusy, setEditingMessageBusy] = useState(false);
  const closeWorkbench = useCallback(() => setWorkbenchOpen(false), []);
  const toggleWorkbench = useCallback(() => {
    if (session.isFreeChat) return;
    setWorkbenchOpen((value) => !value);
  }, [session.isFreeChat]);

  useEffect(() => {
    setDiffTarget(null);
    if (typeof window === "undefined") {
      setDiffWidth(520);
      setDismissedChangeId(null);
      return;
    }
    const savedWidth = Number(window.localStorage.getItem(diffWidthKey));
    setDiffWidth(Number.isFinite(savedWidth) && savedWidth >= 360 ? savedWidth : 520);
    setDismissedChangeId(window.localStorage.getItem(dismissedDiffKey));
  }, [diffWidthKey, dismissedDiffKey]);

  const openDiff = useCallback(
    (path: string, diff: string) => {
      rememberDiffOpen(true);
      setDiffTarget({ path, diff, source: "tool" });
    },
    [rememberDiffOpen]
  );

  const branchTarget = useCallback(
    (change: WorkspaceChange): DiffViewerTarget => ({
      path: "Branch changes",
      diff: change.diff,
      source: "branch",
      changeId: change.changeId,
    }),
    []
  );

  const refreshWorkspaceChanges = useCallback(() => {
    const existing = workspaceRefreshRef.current;
    if (existing?.threadId === session.threadId) return existing.promise;

    const promise = onRefreshWorkspaceChanges();
    const request = { threadId: session.threadId, promise };
    workspaceRefreshRef.current = request;
    void promise.then(
      () => {
        if (workspaceRefreshRef.current === request) workspaceRefreshRef.current = null;
      },
      () => {
        if (workspaceRefreshRef.current === request) workspaceRefreshRef.current = null;
      }
    );
    return promise;
  }, [onRefreshWorkspaceChanges, session.threadId]);

  useEffect(() => {
    if (
      session.isFreeChat ||
      typeof window === "undefined" ||
      window.localStorage.getItem(diffOpenKey) !== "true"
    ) {
      return;
    }
    let cancelled = false;
    void refreshWorkspaceChanges()
      .then((change) => {
        if (!cancelled) setDiffTarget(branchTarget(change));
      })
      .catch(() => {
        // A persisted open state is best-effort when the workspace is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, [branchTarget, diffOpenKey, refreshWorkspaceChanges, session.isFreeChat]);

  const openBranchChanges = useCallback(async () => {
    if (session.isFreeChat) return;
    try {
      const change = await refreshWorkspaceChanges();
      rememberDiffOpen(true);
      setDiffTarget(branchTarget(change));
    } catch {
      // Keep the existing review surface closed when Git cannot be inspected.
    }
  }, [branchTarget, refreshWorkspaceChanges, rememberDiffOpen, session.isFreeChat]);

  useEffect(() => {
    if (
      !workspaceChange ||
      workspaceChange.unavailable ||
      (!workspaceChange.changedFiles.length && !workspaceChange.diff)
    ) {
      return;
    }
    if (dismissedChangeId === workspaceChange.changeId) return;
    if (diffTarget?.source === "branch" && diffTarget.changeId === workspaceChange.changeId) return;
    rememberDiffOpen(true);
    setDiffTarget(branchTarget(workspaceChange));
  }, [branchTarget, diffTarget, dismissedChangeId, rememberDiffOpen, workspaceChange]);

  // The branch button carries counts only once there is something to review,
  // so a clean tree gets a plain icon button instead of a row of zeroes.
  const branchChangeCount = workspaceChange?.changedFiles.length ?? 0;
  const hasBranchChanges = branchChangeCount > 0;

  const openBranchChangeId = diffTarget?.source === "branch" ? diffTarget.changeId : null;
  useEffect(() => {
    if (!openBranchChangeId) return;
    let cancelled = false;
    const tick = async () => {
      try {
        const next = await refreshWorkspaceChanges();
        if (cancelled) return;
        setDiffTarget((current) =>
          current?.source === "branch" ? branchTarget(next) : current
        );
      } catch {
        // The last rendered snapshot stays visible when Git is temporarily unavailable.
      }
    };
    const interval = window.setInterval(tick, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [branchTarget, refreshWorkspaceChanges, openBranchChangeId]);

  const resizeDiff = useCallback(
    (next: number) => {
      setDiffWidth(next);
      if (typeof window !== "undefined") window.localStorage.setItem(diffWidthKey, String(next));
    },
    [diffWidthKey]
  );

  const closeDiff = useCallback(() => {
    rememberDiffOpen(false);
    if (diffTarget?.source === "branch" && diffTarget.changeId) {
      setDismissedChangeId(diffTarget.changeId);
      if (typeof window !== "undefined") {
        window.localStorage.setItem(dismissedDiffKey, diffTarget.changeId);
      }
    }
    setDiffTarget(null);
  }, [diffTarget, dismissedDiffKey, rememberDiffOpen]);
  const showPicker = sessionSupportsModelPicker(session.models);
  const folderLabel = session.isFreeChat ? "No workspace" : shortRoot(session.root);
  const planToBuild = useMemo(() => buildablePlanId(messages), [messages]);
  /**
   * Every tool still waiting on a decision, oldest first.
   *
   * Flattened across messages because the card is anchored to the composer, not
   * to the message that happens to own the call — a turn can leave approvals in
   * more than one assistant message.
   */
  const pendingApprovals = useMemo(
    () =>
      messages.flatMap((message) =>
        message.role === "assistant"
          ? message.tools.filter((tool) => tool.status === "awaiting_approval")
          : []
      ),
    [messages]
  );
  const pendingQuestion = useMemo(() => {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message.role === "assistant" && message.question) {
        return { messageId: message.id, question: message.question };
      }
    }
    return null;
  }, [messages]);
  const hasNeedsInput = pendingApprovals.length > 0 || pendingQuestion !== null;
  const pendingApprovalId = pendingApprovals[0]?.id;
  const pendingQuestionId = pendingQuestion?.messageId;
  const pendingQuestionText = pendingQuestion?.question;
  const needsInputCardRef = useRef<HTMLDivElement>(null);
  const [needsInputCardHeight, setNeedsInputCardHeight] = useState(0);

  useLayoutEffect(() => {
    if (!hasNeedsInput) {
      setNeedsInputCardHeight(0);
      return;
    }

    const card = needsInputCardRef.current;
    if (!card) return;

    const updateHeight = () => {
      setNeedsInputCardHeight(Math.ceil(card.getBoundingClientRect().height));
    };

    updateHeight();
    if (typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(updateHeight);
    observer.observe(card);
    return () => observer.disconnect();
  }, [
    hasNeedsInput,
    pendingApprovalId,
    pendingQuestionId,
    pendingQuestionText,
  ]);

  // The card is positioned above the composer, so shrink the transcript
  // viewport by the card's actual height and leave a small visual gap. The
  // fallback keeps the first paint safe before ResizeObserver reports it.
  const transcriptBottomPadding = hasNeedsInput
    ? Math.max(needsInputCardHeight, 160) + 152
    : undefined;

  const startEditingMessage = useCallback(
    (messageId: string, text: string) => {
      if (sending || editingMessageBusy) return;
      setEditingMessageId(messageId);
      setEditingMessageText(text);
    },
    [editingMessageBusy, sending]
  );

  const cancelEditingMessage = useCallback(() => {
    if (editingMessageBusy) return;
    setEditingMessageId(null);
    setEditingMessageText("");
  }, [editingMessageBusy]);

  const submitEditingMessage = useCallback(async () => {
    const messageId = editingMessageId;
    const text = editingMessageText.trim();
    if (!messageId || !text || sending || editingMessageBusy) return;

    setEditingMessageBusy(true);
    try {
      await onEditMessage(messageId, text);
      setEditingMessageId(null);
      setEditingMessageText("");
    } catch {
      // The parent owns the toast; keep the editor open so the user can retry.
    } finally {
      setEditingMessageBusy(false);
    }
  }, [editingMessageBusy, editingMessageId, editingMessageText, onEditMessage, sending]);

  /**
   * Hand focus back to the toggle when the Workbench closes.
   *
   * In an effect rather than in the close handler: the panel holds focus while
   * it is open, and it is only removed from the document during React's commit.
   * Focusing from the handler raced that — the toggle was focused, the panel was
   * then torn down, and the browser reset focus to `<body>`, stranding the
   * keyboard at the top of the document.
   */
  const workbenchWasOpen = useRef(false);
  useEffect(() => {
    if (session.isFreeChat) setWorkbenchOpen(false);
  }, [session.isFreeChat]);
  useEffect(() => {
    if (workbenchWasOpen.current && !workbenchOpen) {
      document.getElementById("workbench-toggle")?.focus();
    }
    workbenchWasOpen.current = workbenchOpen;
  }, [workbenchOpen]);

  const jumpToMessage = useCallback((messageId: string) => {
    document.getElementById(`message-${messageId}`)?.scrollIntoView({
      behavior: "smooth",
      block: "center",
    });
  }, []);

  const paletteActions = useMemo<PaletteAction[]>(
    () => [
      {
        id: "new-chat",
        label: "New chat",
        description: "Start a fresh conversation without a workspace",
        shortcut: "Ctrl+N",
        run: onNewChat,
      },
      ...(session.isFreeChat
        ? []
        : [
            {
              id: "toggle-workbench",
              label: workbenchOpen ? "Close workbench" : "Open workbench",
              description: "Inspect activity, outline, and recovery checkpoints",
              run: toggleWorkbench,
            },
          ]),
      {
        id: "open-provider",
        label: "Switch provider",
        description: "Choose a configured model provider",
        shortcut: "Ctrl+Shift+M",
        run: () => setProviderSwitchOpen(true),
      },
      {
        id: "open-settings",
        label: "Open settings",
        description: "Configure Zest and keyboard shortcuts",
        shortcut: "Ctrl+,",
        run: () => {
          setFocusUser(false);
          setSettingsOpen(true);
        },
      },
    ],
    [onNewChat, session.isFreeChat, toggleWorkbench, workbenchOpen]
  );

  // A bump means "open the User section". Zero is the initial value, so the
  // panel does not fly open on mount.
  useEffect(() => {
    if (settingsRequest <= 0) return;
    setFocusUser(true);
    setSettingsOpen(true);
  }, [settingsRequest]);

  function closeSettings() {
    setSettingsOpen(false);
    setFocusUser(false);
  }

  function setSidebar(next: boolean) {
    setSidebarOpen(next);
    writeSidebarOpen(next);
  }

  // Escape stays hand-written and is not rebindable: it means "dismiss what is
  // on top", so it has to read the stack of open surfaces in order.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (diffTarget) {
        e.preventDefault();
        closeDiff();
        return;
      }
      if (providerSwitchOpen) {
        e.preventDefault();
        if (!providerSwitchBusy) setProviderSwitchOpen(false);
        return;
      }
      if (settingsOpen) {
        e.preventDefault();
        closeSettings();
        return;
      }
      if (paletteOpen) {
        e.preventDefault();
        setPaletteOpen(false);
        return;
      }
      if (editingMessageId) {
        e.preventDefault();
        cancelEditingMessage();
        return;
      }
      if (sending && onStop) {
        e.preventDefault();
        onStop();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cancelEditingMessage, closeDiff, diffTarget, editingMessageId, onStop, paletteOpen, providerSwitchBusy, providerSwitchOpen, sending, settingsOpen]);

  // Everything else comes from the registry, so the shortcuts editor is the one
  // place that decides which key runs which command.
  useKeybindings({
    "chat.new": onNewChat,
    "chat.stop": () => {
      if (sending) onStop?.();
    },
    "focus.composer": focusComposer,
    "view.sidebar": () => setSidebar(!sidebarOpen),
    "view.settings": () => {
      setFocusUser(false);
      setSettingsOpen(true);
    },
    "view.shortcuts": () => {
      setFocusUser(false);
      setShortcutsRequest((n) => n + 1);
      setSettingsOpen(true);
    },
    "view.profile": () => onOpenProfile?.(),
    "view.usage": () => onOpenUsage?.(),
    "view.provider": () => setProviderSwitchOpen(true),
    "view.palette": () => setPaletteOpen(true),
  });

  return (
    <section className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden bg-[var(--chat-canvas)]">
      <ChatHistorySidebar
        open={sidebarOpen}
        activeThreadId={session.threadId}
        activeProjectPath={session.isFreeChat ? null : session.root}
        activeProviderId={session.provider}
        sending={sending}
        threadActivity={threadActivity}
        onOpenChange={setSidebar}
        onNewChat={onNewChat}
        onOpenProjectChat={onOpenProjectChat}
        onForkThread={onForkThread}
        onDeleteThread={onDeleteThread}
        onOpenFolder={onOpenFolder}
      />

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex min-w-0 shrink-0 items-center gap-2 border-b border-border/60 bg-[var(--chat-header)] px-4 py-2.5">
          <div className="flex min-w-0 flex-1 items-center gap-2.5">
            <UserAvatarButton
              avatarDataUrl={profile.avatarDataUrl}
              displayName={profile.displayName}
              title="Your profile"
              className="shrink-0"
              onClick={() => {
                if (onOpenProfile) {
                  onOpenProfile();
                  return;
                }
                setFocusUser(true);
                setSettingsOpen(true);
              }}
            />
            <div className="min-w-0 flex-1 leading-tight">
              <div
                className="truncate text-sm font-semibold tracking-[-0.2px]"
                title={profile.displayName.trim() || "Zest"}
              >
                {profile.displayName.trim() || "Zest"}
              </div>
              <div
                className="min-w-0 max-w-[48ch] truncate text-[11px] text-muted-foreground"
                title={`${session.isFreeChat ? "No workspace" : session.root}${branch ? ` · ${branch}` : ""}`}
              >
                {session.label} · {folderLabel}
                {branch ? ` · ${branch}` : ""}
              </div>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-0.5">
            {!session.isFreeChat ? (
              <Button
                type="button"
                variant="outline"
                size={hasBranchChanges ? "sm" : "icon-sm"}
                className={hasBranchChanges ? "text-[11px] font-normal tabular-nums" : undefined}
                title="Show branch diff"
                aria-label={
                  hasBranchChanges
                    ? `Show branch diff — ${branchChangeCount} ${
                        branchChangeCount === 1 ? "file" : "files"
                      } changed, ${workspaceChange?.additions ?? 0} added, ${
                        workspaceChange?.deletions ?? 0
                      } removed`
                    : "Show branch diff"
                }
                onClick={() => void openBranchChanges()}
              >
                <GitBranchIcon data-icon="inline-start" aria-hidden="true" />
                {hasBranchChanges && workspaceChange ? (
                  <>
                    <span className="text-muted-foreground">{branchChangeCount}</span>
                    <span className="text-primary">+{workspaceChange.additions}</span>
                    <span className="text-destructive">−{workspaceChange.deletions}</span>
                  </>
                ) : null}
              </Button>
            ) : null}
            <AgentQuotaButton providers={providers} refreshKey={`${session.threadId}:${messages.length}`} />
            <NowPlayingButton />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title="Command palette (Ctrl+K)"
              aria-label="Open command palette"
              aria-expanded={paletteOpen}
              onClick={() => setPaletteOpen(true)}
            >
              <CommandIcon />
            </Button>
            {!session.isFreeChat ? (
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title={workbenchOpen ? "Close Workbench" : "Open Workbench"}
                aria-label={workbenchOpen ? "Close Workbench" : "Open Workbench"}
                aria-controls="workbench-panel"
                aria-expanded={workbenchOpen}
                id="workbench-toggle"
                onClick={toggleWorkbench}
              >
                <PanelRightOpenIcon aria-hidden="true" />
              </Button>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title="Settings (Ctrl+,)"
              aria-label="Open settings"
              aria-expanded={settingsOpen}
              onClick={() => {
                setFocusUser(false);
                setSettingsOpen(true);
              }}
            >
              <SettingsIcon />
            </Button>
          </div>
        </header>

        {sessionWarning ? (
          <div
            role="status"
            className="flex items-start gap-2.5 border-b border-amber-500/30 bg-amber-500/10 px-4 py-2.5 animate-in fade-in slide-in-from-top-1 duration-200"
          >
            <TriangleAlertIcon className="mt-px size-3.5 shrink-0 text-amber-400" aria-hidden />
            <p className="m-0 min-w-0 flex-1 whitespace-pre-wrap text-[11px] leading-relaxed text-amber-200/90">
              {sessionWarning.message}
            </p>
            {sessionWarning.offerReconnect ? (
              <Button type="button" size="sm" variant="outline" onClick={onReconnect}>
                Reconnect
              </Button>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title="Dismiss"
              aria-label="Dismiss this warning"
              onClick={onDismissWarning}
            >
              <XIcon />
            </Button>
          </div>
        ) : null}

        <div className="relative min-h-0 flex-1">
          <CheckpointRail
            checkpoints={session.checkpoints}
            messages={messages}
            onJump={jumpToMessage}
          />
          <MessageScrollerProvider autoScroll scrollEdgeThreshold={24}>
            <MessageScroller
              className={cn("absolute inset-0", !hasNeedsInput && "pb-40")}
              style={{
                paddingBottom:
                  transcriptBottomPadding === undefined
                    ? undefined
                    : `${transcriptBottomPadding}px`,
              }}
            >
              <MessageScrollerViewport className="scroll-fade-b">
                <MessageScrollerContent className="mx-auto w-full max-w-[var(--chat-max)] gap-6 pl-10 pr-4 py-6">
                  {messages.length === 0 ? (
                    <MessageScrollerItem messageId="empty">
                      <div className="flex min-h-[42vh] flex-col items-center justify-center gap-4 text-center">
                        <p className="max-w-[34ch] text-sm text-muted-foreground">
                          Ask about this project — paste images, attach files, or
                          open another folder from +.
                        </p>
                        <div
                          className="flex flex-wrap items-center justify-center gap-2"
                          aria-label="Suggested prompts"
                        >
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              onDraftChange("/plan ");
                              requestAnimationFrame(() => focusComposer());
                            }}
                          >
                            Plan this repo
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              onDraftChange("Explain the structure and purpose of this project.");
                              requestAnimationFrame(() => focusComposer());
                            }}
                          >
                            Explain this project
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              onDraftChange("Review the current changes and point out anything risky or incomplete.");
                              requestAnimationFrame(() => focusComposer());
                            }}
                          >
                            Review current changes
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              onDraftChange("/");
                              requestAnimationFrame(() => focusComposer());
                            }}
                          >
                            / commands
                          </Button>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={onOpenFolder}
                          >
                            <FolderOpenIcon className="size-3.5" />
                            Open folder
                          </Button>
                        </div>
                      </div>
                    </MessageScrollerItem>
                  ) : null}

                  {messages.map((msg, index) => (
                    <ChatMessageRow
                      key={msg.id}
                      message={msg}
                      isLast={index === messages.length - 1}
                      sending={sending}
                      approvalMode={approvalMode}
                      planToBuild={planToBuild}
                      onBuildPlan={onBuildPlan}
                      onResolveApproval={onResolveApproval}
                      onOpenDiff={openDiff}
                      onReconnectProvider={onReconnectProvider}
                      onSend={onSend}
                      editing={editingMessageId === msg.id}
                      editingText={editingMessageText}
                      editingBusy={editingMessageBusy}
                      onStartEdit={startEditingMessage}
                      onChangeEdit={setEditingMessageText}
                      onCancelEdit={cancelEditingMessage}
                      onSubmitEdit={() => void submitEditingMessage()}
                      onResolveQuestion={onResolveQuestion}
                      pinQuestion={
                        pendingApprovals.length === 0 &&
                        pendingQuestion?.messageId === msg.id
                      }
                    />
                  ))}
                </MessageScrollerContent>
              </MessageScrollerViewport>
              <MessageScrollerButton />
            </MessageScroller>
          </MessageScrollerProvider>

          {/*
            One decision card, in one place, for the whole conversation.
            Anchored above the composer rather than inline in the transcript:
            inline it scrolled away mid-decision, and every pending call drew
            another copy of the same controls. The transcript still shows
            each pending call as a one-line "Awaiting approval" row, so the
            history stays readable — it just does not ask twice.
          */}
          {hasNeedsInput ? (
            <div className="pointer-events-none absolute inset-x-0 bottom-0 z-20 px-4 pb-[8.5rem]">
              <NeedsInputCard
                cardRef={needsInputCardRef}
                question={pendingApprovals.length === 0 ? pendingQuestion?.question : undefined}
                approval={pendingApprovals[0]}
                pendingApprovalCount={pendingApprovals.length}
                onResolveQuestion={onResolveQuestion}
                onResolveApproval={onResolveApproval}
                onOpenDiff={openDiff}
              />
            </div>
          ) : null}

          <Composer
            approvalMode={approvalMode}
            onApprovalModeChange={onApprovalModeChange}
            value={draft}
            model={model}
            effort={effort}
            models={session.models}
            defaultModel={session.defaultModel}
            folderLabel={folderLabel}
            branch={branch}
            gitContext={gitContext}
            contextRefreshKey={`${session.threadId}:${messages.length}:${session.checkpoints.length}:${sending ? 1 : 0}`}
            sending={sending}
            queuedMessages={queuedMessages}
            onUpdateQueuedMessage={onUpdateQueuedMessage}
            onRemoveQueuedMessage={onRemoveQueuedMessage}
            showModelPicker={showPicker}
            optionsDisabled={optionsDisabled}
            attachments={attachments}
            onChange={onDraftChange}
            onSubmit={onSend}
            onStop={onStop}
            onModelChange={onModelChange}
            onEffortChange={onEffortChange}
            onAttachFiles={onAttachFiles}
            onOpenFolder={onOpenFolder}
            onRemoveAttachment={onRemoveAttachment}
            onPasteImages={onPasteImages}
            compacting={compacting}
            aboveComposer={null}
          />
        </div>
      </div>

      <SettingsPanel
        open={settingsOpen}
        session={session}
        model={model}
        effort={effort}
        sending={sending}
        profile={profile}
        focusUser={focusUser}
        focusShortcuts={shortcutsRequest}
        onClose={closeSettings}
        onChangeProvider={() => {
          closeSettings();
          setProviderSwitchOpen(true);
        }}
        onReloadSession={onReloadSession}
        onReconnect={() => {
          closeSettings();
          onReconnect();
        }}
        onOpenFolder={() => {
          closeSettings();
          onOpenFolder();
        }}
        onProfileChange={onProfileChange}
      />

      <ProviderSwitchSheet
        open={providerSwitchOpen}
        providers={providers}
        currentProviderId={session.provider}
        busy={providerSwitchBusy}
        onClose={() => {
          if (!providerSwitchBusy) setProviderSwitchOpen(false);
        }}
        onSelect={(providerId) => {
          void (async () => {
            setProviderSwitchBusy(true);
            try {
              await onSwitchProvider(providerId);
              setProviderSwitchOpen(false);
            } catch {
              /* parent toasts */
            } finally {
              setProviderSwitchBusy(false);
            }
          })();
        }}
        onConnect={(providerId) => {
          setProviderSwitchOpen(false);
          onReconnectProvider?.(providerId);
        }}
        onRefresh={onRefreshProviders}
      />

      <WorkbenchPanel
        open={workbenchOpen}
        session={session}
        messages={messages}
        sending={sending}
        compacting={compacting}
        review={workspaceReview}
        onClose={closeWorkbench}
        onFork={onForkThread}
        onVerify={onVerifyWorkspace}
        onRewind={onRewindThread}
        onJump={jumpToMessage}
        delegationJobs={delegationJobs}
        onApproveDelegation={onApproveDelegation}
        onCancelDelegation={onCancelDelegation}
        onRetryDelegation={onRetryDelegation}
        onApplyDelegation={onApplyDelegation}
      />

      <CommandPalette
        open={paletteOpen}
        actions={paletteActions}
        onClose={() => setPaletteOpen(false)}
        onCommand={(name) => {
          setPaletteOpen(false);
          onDraftChange(`/${name} `);
          requestAnimationFrame(() => focusComposer());
        }}
      />

      <DiffViewer
        target={diffTarget}
        branch={gitContext?.branch ?? branch}
        baseBranch={gitContext?.baseBranch}
        width={diffWidth}
        onResize={resizeDiff}
        storageKey={`zest:diff-view:${session.threadId}`}
        onClose={closeDiff}
      />
    </section>
  );
}
