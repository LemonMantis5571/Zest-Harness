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
import { CustomizePanel } from "@/components/CustomizePanel";
import { CheckpointRail } from "@/components/CheckpointRail";
import { ConversationTurnHistory } from "@/components/ConversationTurnHistory";
import { CommandPalette, type PaletteAction } from "@/components/CommandPalette";
import { AgentQuotaButton } from "@/components/AgentQuotaButton";
import { BranchChangesBar } from "@/components/BranchChangesBar";
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
import { ProfileScreen } from "@/components/ProfileScreen";
import { ProviderSwitchSheet } from "@/components/ProviderSwitchSheet";
import { SettingsPanel } from "@/components/SettingsPanel";
import { ToolCallRow } from "@/components/ToolCallRow";
import { UsageScreen } from "@/components/UsageScreen";
import { ToolRunGroup } from "@/components/ToolRunGroup";
import { ThinkingReasoning } from "@/components/ThinkingReasoning";
import { WorkbenchPanel } from "@/components/WorkbenchPanel";
import { WorkingIndicator } from "@/components/WorkingIndicator";
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
  useMessageScroller,
  useMessageScrollerScrollable,
} from "@/components/ui/message-scroller";
import { ZestPulse } from "@/components/ZestPulse";
import { toast } from "@/components/ui/toast";
import { getBackend } from "@/lib/backend";
import { buildConversationTurns } from "@/lib/conversationTurns";
import { LinkifyText } from "@/lib/linkify";
import { sessionSupportsModelPicker, type EffortId } from "@/lib/models";
import type { CustomizeTab, ShellPanel } from "@/lib/navigationHistory";
import { collapseThresholdFor, groupToolRuns } from "@/lib/toolRuns";
import { currentTurnAction, type ThreadActivityMap } from "@/lib/threadActivity";
import type { QueuedTurn } from "@/lib/threadQueue";
import {
  TRANSCRIPT_REVEAL_STEP,
  clampTranscriptStart,
  initialTranscriptStart,
  revealEarlierTranscriptStart,
  shouldTrimTranscript,
  transcriptStartForTarget,
} from "@/lib/transcriptWindow";
import { useKeybindings } from "@/lib/useKeybindings";
import { useMediaQuery } from "@/lib/useMediaQuery";
import type {
  ApprovalChoice,
  ApprovalMode,
  ChatMessage,
  DelegationCreateInput,
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
  /** Show the Customize panel (MCP servers, skills, project instructions). */
  onOpenCustomize?: () => void;
  /** Which panel is showing in the transcript's place, or null for the transcript. */
  shellPanel?: ShellPanel | null;
  /** Record a Customize tab change in the app navigation history. */
  onCustomizeTabChange?: (tab: CustomizeTab) => void;
  /** Leave the open panel for wherever the user was before it. */
  onClosePanel?: () => void;
  /** Record and open the Settings view in the app navigation history. */
  onOpenSettings?: () => void;
  /** Remove Settings from the app navigation history when it closes. */
  onCloseSettings?: () => void;
  canNavigateBack: boolean;
  canNavigateForward: boolean;
  onNavigateBack: () => void;
  onNavigateForward: () => void;
  /** Bumped to refresh provider availability and open the provider sheet. */
  providerSwitchRequest?: number;
  /**
   * Bumped to request the User section of Settings — the profile screen sends
   * edits here rather than duplicating the form.
   */
  settingsRequest?: number;
  /** Bumped to open Settings without forcing the User section. */
  settingsOpenRequest?: number;
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
  onCreateDelegation: (request: DelegationCreateInput) => Promise<DelegationJob>;
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
  onOpenProviderSwitch?: () => void;
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
  showWorking?: boolean;
  workingStartedAt?: number;
  workingAction?: string;
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
  onOpenProviderSwitch,
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
  showWorking = false,
  workingStartedAt,
  workingAction,
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
            <ThinkingReasoning thinking={msg.thinking} streaming={msg.streaming} />
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
                {msg.providerSelection && onOpenProviderSwitch ? (
                  <div className="mt-2.5">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={onOpenProviderSwitch}
                    >
                      Choose provider or API key
                    </Button>
                  </div>
                ) : null}
              </BubbleContent>
            </Bubble>
          ) : null}

          {showWorking ? (
            <WorkingIndicator
              startedAt={workingStartedAt}
              action={workingAction}
            />
          ) : null}
        </MessageContent>
      </Message>
    </MessageScrollerItem>
  );
});

type ScrollToTranscriptMessage = ReturnType<
  typeof useMessageScroller
>["scrollToMessage"];

function TranscriptScrollerControls({
  hiddenCount,
  onRevealEarlier,
  onAtEndChange,
  onRegisterScrollToMessage,
}: {
  hiddenCount: number;
  onRevealEarlier: () => void;
  onAtEndChange: (atEnd: boolean) => void;
  onRegisterScrollToMessage: (
    scrollToMessage: ScrollToTranscriptMessage | null
  ) => void;
}) {
  const { scrollToMessage } = useMessageScroller();
  const scrollable = useMessageScrollerScrollable();
  const [mounted, setMounted] = useState(false);

  useLayoutEffect(() => {
    onRegisterScrollToMessage(scrollToMessage);
    return () => onRegisterScrollToMessage(null);
  }, [onRegisterScrollToMessage, scrollToMessage]);

  useEffect(() => {
    setMounted(true);
  }, []);

  useEffect(() => {
    onAtEndChange(!scrollable.end);
  }, [onAtEndChange, scrollable.end]);

  if (!mounted || hiddenCount === 0 || scrollable.start) return null;
  const revealCount = Math.min(hiddenCount, TRANSCRIPT_REVEAL_STEP);
  return (
    <Button
      type="button"
      size="sm"
      variant="secondary"
      className="absolute left-1/2 top-3 z-20 -translate-x-1/2 border border-border/80 bg-background/95 shadow-md backdrop-blur-sm"
      onClick={onRevealEarlier}
    >
      Show {revealCount} earlier message{revealCount === 1 ? "" : "s"}
    </Button>
  );
}

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
  onOpenCustomize,
  shellPanel = null,
  onCustomizeTabChange,
  onClosePanel,
  onOpenSettings,
  onCloseSettings,
  canNavigateBack,
  canNavigateForward,
  onNavigateBack,
  onNavigateForward,
  providerSwitchRequest = 0,
  settingsRequest = 0,
  settingsOpenRequest = 0,
  optionsDisabled = false,
  delegationJobs,
  onCreateDelegation,
  onApproveDelegation,
  onCancelDelegation,
  onRetryDelegation,
  onApplyDelegation,
}: Props) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [focusUser, setFocusUser] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(readSidebarOpen);
  /**
   * Too narrow to hold the sidebar, a checkpoint rail, and a readable
   * transcript at the same time.
   *
   * 768px is where a 260px sidebar stops leaving the conversation enough room:
   * below it the transcript was down to a ~300px column, inset asymmetrically
   * by the rail's gutter.
   */
  const narrow = useMediaQuery("(max-width: 767px)");
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
  const refreshAndOpenProviderSwitch = useCallback(() => {
    void onRefreshProviders()
      .catch(() => {})
      .finally(() => setProviderSwitchOpen(true));
  }, [onRefreshProviders]);
  const refreshAndOpenProviderSwitchRef = useRef(refreshAndOpenProviderSwitch);
  refreshAndOpenProviderSwitchRef.current = refreshAndOpenProviderSwitch;

  useEffect(() => {
    if (providerSwitchRequest <= 0) return;
    refreshAndOpenProviderSwitchRef.current();
  }, [providerSwitchRequest]);
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
  const [transcriptWindow, setTranscriptWindow] = useState(() => ({
    threadId: session.threadId,
    start: initialTranscriptStart(messages.length),
  }));
  const scrollToTranscriptMessageRef =
    useRef<ScrollToTranscriptMessage | null>(null);
  const pendingTranscriptJumpRef = useRef<string | null>(null);

  const transcriptStart =
    transcriptWindow.threadId === session.threadId
      ? clampTranscriptStart(messages.length, transcriptWindow.start)
      : initialTranscriptStart(messages.length);
  const visibleMessages = useMemo(
    () => messages.slice(transcriptStart),
    [messages, transcriptStart]
  );
  const conversationTurns = useMemo(
    () => buildConversationTurns(messages, session.checkpoints),
    [messages, session.checkpoints]
  );

  useEffect(() => {
    setTranscriptWindow((current) => {
      const start =
        current.threadId === session.threadId
          ? clampTranscriptStart(messages.length, current.start)
          : initialTranscriptStart(messages.length);
      if (current.threadId === session.threadId && current.start === start) {
        return current;
      }
      return { threadId: session.threadId, start };
    });
  }, [messages.length, session.threadId]);

  useEffect(() => {
    pendingTranscriptJumpRef.current = null;
  }, [session.threadId]);

  const registerScrollToTranscriptMessage = useCallback(
    (scrollToMessage: ScrollToTranscriptMessage | null) => {
      scrollToTranscriptMessageRef.current = scrollToMessage;
    },
    []
  );

  const scrollToTranscriptMessage = useCallback((messageId: string) => {
    const scrolled = scrollToTranscriptMessageRef.current?.(messageId, {
      behavior: "smooth",
      align: "center",
    });
    if (scrolled) return;
    document.getElementById(`message-${messageId}`)?.scrollIntoView({
      behavior: "smooth",
      block: "center",
    });
  }, []);

  const revealEarlierMessages = useCallback(() => {
    setTranscriptWindow({
      threadId: session.threadId,
      start: revealEarlierTranscriptStart(transcriptStart),
    });
  }, [session.threadId, transcriptStart]);

  const handleTranscriptAtEndChange = useCallback(
    (atEnd: boolean) => {
      if (pendingTranscriptJumpRef.current || editingMessageId) return;
      if (!shouldTrimTranscript(messages.length, transcriptStart, atEnd)) return;
      setTranscriptWindow({
        threadId: session.threadId,
        start: initialTranscriptStart(messages.length),
      });
    },
    [editingMessageId, messages.length, session.threadId, transcriptStart]
  );

  useLayoutEffect(() => {
    const messageId = pendingTranscriptJumpRef.current;
    if (!messageId) return;
    const targetIndex = messages.findIndex((message) => message.id === messageId);
    if (targetIndex < transcriptStart) return;
    const frame = window.requestAnimationFrame(() => {
      scrollToTranscriptMessage(messageId);
      pendingTranscriptJumpRef.current = null;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [messages, scrollToTranscriptMessage, transcriptStart]);
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
        if (cancelled) return;
        // Restoring "this thread had the diff open" is only meaningful while
        // there is something to review. Without this guard, a tree that went
        // clean since the panel was last open — committed, stashed, or branch
        // switched — reopened an empty "+0 −0" panel on every mount, because
        // the persisted flag outlives the changes it was set for. Drop the flag
        // so it stops firing until the panel is deliberately opened again.
        if (
          change.unavailable ||
          (!change.changedFiles.length && !change.diff)
        ) {
          rememberDiffOpen(false);
          return;
        }
        setDiffTarget(branchTarget(change));
      })
      .catch(() => {
        // A persisted open state is best-effort when the workspace is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, [
    branchTarget,
    diffOpenKey,
    refreshWorkspaceChanges,
    rememberDiffOpen,
    session.isFreeChat,
  ]);

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

  // Finding changes no longer throws the diff panel open by itself. That put a
  // full-height review surface over the transcript uninvited, and the panel is
  // the wrong size for "something changed" — BranchChangesBar carries that
  // message in one row and opens the panel when the user actually asks.

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
  /**
   * Hide the branch strip for this exact set of changes.
   *
   * Keyed on the change id rather than a plain boolean so the strip comes back
   * by itself the next time the branch moves — dismissing "these 300 lines" is
   * not a request to stop being told about the next three hundred.
   */
  const dismissBranchBar = useCallback(() => {
    const changeId = workspaceChange?.changeId;
    if (!changeId) return;
    setDismissedChangeId(changeId);
    if (typeof window !== "undefined") {
      window.localStorage.setItem(dismissedDiffKey, changeId);
    }
  }, [dismissedDiffKey, workspaceChange]);

  const showPicker = sessionSupportsModelPicker(session.models);
  const folderLabel = session.isFreeChat ? "No workspace" : shortRoot(session.root);
  /**
   * The provider as a person would name it, falling back to the session's own
   * label when the picker row is not loaded yet. Shown under the name in the
   * sidebar's profile row and on the profile page.
   */
  const providerLabel =
    providers.find((row) => row.id === session.provider)?.label ?? session.label;
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
  const lastAssistant = useMemo(() => {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message.role === "assistant") return message;
    }
    return undefined;
  }, [messages]);
  const turnActivity = threadActivity[session.threadId];
  const workingAction = currentTurnAction(lastAssistant, turnActivity);
  const showTurnWorking = sending && !hasNeedsInput;
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

  const revealProjectFolder = useCallback(async () => {
    try {
      await getBackend().revealWorkspaceFolder();
    } catch (err) {
      toast.add({
        type: "warning",
        title: "Could not open the folder",
        description:
          err instanceof Error ? err.message : "The file manager did not start.",
      });
    }
  }, []);

  const openSettings = useCallback(
    (focusUser = false) => {
      setFocusUser(focusUser);
      setSettingsOpen(true);
      onOpenSettings?.();
    },
    [onOpenSettings]
  );

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

  const jumpToMessage = useCallback(
    (messageId: string) => {
      const targetIndex = messages.findIndex((message) => message.id === messageId);
      if (targetIndex < 0) return;
      if (targetIndex < transcriptStart) {
        pendingTranscriptJumpRef.current = messageId;
        setTranscriptWindow({
          threadId: session.threadId,
          start: transcriptStartForTarget(transcriptStart, targetIndex),
        });
        return;
      }
      scrollToTranscriptMessage(messageId);
    },
    [messages, scrollToTranscriptMessage, session.threadId, transcriptStart]
  );

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
      ...(onOpenCustomize
        ? [
            {
              id: "open-customize",
              label: "Open Customize",
              description: "MCP servers, skills, and project instructions",
              shortcut: "Ctrl+Shift+,",
              run: onOpenCustomize,
            },
          ]
        : []),
      {
        id: "open-settings",
        label: "Open settings",
        description: "Configure Zest and keyboard shortcuts",
        shortcut: "Ctrl+,",
        run: openSettings,
      },
    ],
    [
      onNewChat,
      onOpenCustomize,
      openSettings,
      session.isFreeChat,
      toggleWorkbench,
      workbenchOpen,
    ]
  );

  // A bump means "open the User section". Zero is the initial value, so the
  // panel does not fly open on mount.
  useEffect(() => {
    if (settingsRequest <= 0) return;
    setFocusUser(true);
    setSettingsOpen(true);
  }, [settingsRequest]);

  useEffect(() => {
    if (settingsOpenRequest <= 0) return;
    setFocusUser(false);
    setSettingsOpen(true);
  }, [settingsOpenRequest]);

  const closeSettings = useCallback(() => {
    setSettingsOpen(false);
    setFocusUser(false);
    onCloseSettings?.();
  }, [onCloseSettings]);

  /** An explicit toggle, so it is remembered. */
  function setSidebar(next: boolean) {
    setSidebarOpen(next);
    writeSidebarOpen(next);
  }

  // Collapse on the way into a narrow window and restore the remembered choice
  // on the way out. Deliberately not persisted: resizing a window is not the
  // user saying they prefer a collapsed sidebar.
  useEffect(() => {
    setSidebarOpen(narrow ? false : readSidebarOpen());
  }, [narrow]);

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
  }, [cancelEditingMessage, closeDiff, closeSettings, diffTarget, editingMessageId, onStop, paletteOpen, providerSwitchBusy, providerSwitchOpen, sending, settingsOpen]);

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
      openSettings();
    },
    "view.shortcuts": () => onCustomizeTabChange?.("shortcuts"),
    "view.profile": () => onOpenProfile?.(),
    "view.usage": () => onOpenUsage?.(),
    "view.customize": () => onOpenCustomize?.(),
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
        onOpenCustomize={onOpenCustomize}
        customizeActive={shellPanel?.kind === "customize"}
        profileActive={shellPanel?.kind === "profile"}
        profile={profile}
        providerLabel={providerLabel}
        onOpenProfile={onOpenProfile}
        canNavigateBack={canNavigateBack}
        canNavigateForward={canNavigateForward}
        onNavigateBack={onNavigateBack}
        onNavigateForward={onNavigateForward}
      />

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex min-w-0 shrink-0 items-center gap-2 border-b border-border/60 bg-[var(--chat-header)] px-4 py-2.5">
          {/*
           * The header says where you are; who you are moved to the bottom of
           * the sidebar. So the project leads, with the provider and branch
           * under it, instead of the display name leading and the project
           * being crowded onto the second line.
           */}
          <div className="flex min-w-0 flex-1 items-center gap-2.5">
            <div className="min-w-0 flex-1 leading-tight">
              <div
                className="truncate text-sm font-semibold tracking-[-0.2px]"
                title={session.isFreeChat ? "No workspace" : session.root}
              >
                {session.isFreeChat ? "No workspace" : folderLabel}
              </div>
              <div
                className="min-w-0 max-w-[48ch] truncate text-[11px] text-muted-foreground"
                title={`${session.label}${branch ? ` · ${branch}` : ""}`}
              >
                {session.label}
                {branch ? ` · ${branch}` : ""}
              </div>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-0.5">
            {/* Branch changes moved out of this row and into BranchChangesBar
                below the header, where the counts can say which project and
                branch they belong to. */}
            <ConversationTurnHistory
              turns={conversationTurns}
              messageCount={messages.length}
              onJump={jumpToMessage}
            />
            <AgentQuotaButton providers={providers} refreshKey={`${session.threadId}:${messages.length}`} />
            <NowPlayingButton />
            {/* Only with a project: a projectless chat has no folder to show,
                and the backend refuses to reveal Zest's own free-chat store. */}
            {!session.isFreeChat ? (
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title={`Show ${folderLabel} in the file manager`}
                aria-label="Show the project folder in the file manager"
                onClick={() => void revealProjectFolder()}
              >
                <FolderOpenIcon aria-hidden="true" />
              </Button>
            ) : null}
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
              onClick={() => openSettings()}
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

        {/*
         * Panels reached from the sidebar take the transcript's place inside
         * this shell rather than replacing the whole window: the sidebar and
         * header stay, so the chat is one click away and the nav item can show
         * as current. The transcript block below is left at its original
         * indentation so wrapping it stays a two-line diff.
         */}
        {shellPanel ? (
          shellPanel.kind === "customize" ? (
            <CustomizePanel
              tab={shellPanel.tab}
              sending={sending}
              providerOwnsAgentLoop={session.ownsAgentLoop}
              providerLabel={providerLabel}
              onTabChange={(next) => onCustomizeTabChange?.(next)}
              onBack={() => onClosePanel?.()}
            />
          ) : (
            // Profile and Usage were written as whole pages, so they get the
            // scroll container the transcript would have had.
            <div className="min-h-0 flex-1 overflow-y-auto">
              {shellPanel.kind === "profile" ? (
                <ProfileScreen
                  profile={profile}
                  providerLabel={providerLabel}
                  onBack={() => onClosePanel?.()}
                  // Editing name and avatar stays in Settings; the profile
                  // page reports, it does not duplicate the form.
                  onEditProfile={() => openSettings(true)}
                  onOpenUsage={() => onOpenUsage?.()}
                />
              ) : (
                <UsageScreen onBack={() => onClosePanel?.()} />
              )}
            </div>
          )
        ) : (
        <div className="relative min-h-0 flex-1">
          {/* The rail needs a gutter of its own; a narrow window has none to
              spare, so it steps aside and the transcript takes the width. */}
          {narrow ? null : (
            <CheckpointRail
              turns={conversationTurns}
              onJump={jumpToMessage}
            />
          )}
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
                <MessageScrollerContent
                  className={cn(
                    "mx-auto w-full max-w-[var(--chat-max)] gap-6 py-6",
                    // The extra left inset exists only to clear the rail.
                    narrow ? "px-3" : "pl-10 pr-4"
                  )}
                >
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

                  {visibleMessages.map((msg, index) => (
                    <ChatMessageRow
                      key={msg.id}
                      message={msg}
                      isLast={index === visibleMessages.length - 1}
                      sending={sending}
                      approvalMode={approvalMode}
                      planToBuild={planToBuild}
                      onBuildPlan={onBuildPlan}
                      onResolveApproval={onResolveApproval}
                      onOpenDiff={openDiff}
                      onReconnectProvider={onReconnectProvider}
                      onOpenProviderSwitch={refreshAndOpenProviderSwitch}
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
                      showWorking={
                        showTurnWorking &&
                        msg.role === "assistant" &&
                        index === visibleMessages.length - 1
                      }
                      workingStartedAt={turnActivity?.startedAt}
                      workingAction={workingAction}
                    />
                  ))}
                  {showTurnWorking &&
                  visibleMessages[visibleMessages.length - 1]?.role !==
                    "assistant" ? (
                    <MessageScrollerItem
                      id="turn-working"
                      messageId="turn-working"
                    >
                      <Message align="start">
                        <MessageContent className="w-full max-w-full gap-2.5">
                          <div className="text-[11px] font-medium tracking-wide text-muted-foreground/80">
                            Zest
                          </div>
                          <WorkingIndicator
                            startedAt={turnActivity?.startedAt}
                            action={workingAction}
                          />
                        </MessageContent>
                      </Message>
                    </MessageScrollerItem>
                  ) : null}
                </MessageScrollerContent>
              </MessageScrollerViewport>
              <TranscriptScrollerControls
                hiddenCount={transcriptStart}
                onRevealEarlier={revealEarlierMessages}
                onAtEndChange={handleTranscriptAtEndChange}
                onRegisterScrollToMessage={registerScrollToTranscriptMessage}
              />
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
            aboveComposer={
              /*
               * The branch strip rides above the composer rather than under the
               * top bar.
               *
               * Under the top bar it was a full-width row wedged between the
               * header and the transcript, so it pushed the conversation down
               * every time it appeared and sat nowhere near anything the user
               * was doing. Above the composer it is attached to the thing being
               * acted on, and it overlays rather than reflows.
               *
               * `!sending` is the other half of the fix. The change id moves on
               * every write, so a strip dismissed mid-turn came straight back
               * — several times per turn while an agent edited files. Holding
               * it until the turn settles means it can reappear at most once
               * per turn, and it reappears against a diff that has stopped
               * moving, which is the only point at which reviewing it is
               * worthwhile.
               */
              !session.isFreeChat &&
              hasBranchChanges &&
              !diffTarget &&
              !sending &&
              dismissedChangeId !== workspaceChange?.changeId ? (
                <BranchChangesBar
                  projectLabel={folderLabel}
                  branch={gitContext?.branch ?? branch}
                  workspaceChange={workspaceChange}
                  gitContext={gitContext}
                  onOpen={() => void openBranchChanges()}
                  onDismiss={dismissBranchBar}
                />
              ) : null
            }
          />
        </div>
        )}
      </div>

      <SettingsPanel
        open={settingsOpen}
        session={session}
        model={model}
        effort={effort}
        sending={sending}
        profile={profile}
        focusUser={focusUser}
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
        onProviderKeyRemoved={() => {
          closeSettings();
          refreshAndOpenProviderSwitch();
        }}
        onOpenFolder={() => {
          closeSettings();
          onOpenFolder();
        }}
        onOpenCustomize={() => {
          closeSettings();
          onOpenCustomize?.();
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
        onVerify={onVerifyWorkspace}
        onRewind={onRewindThread}
        onJump={jumpToMessage}
        delegationJobs={delegationJobs}
        onCreateDelegation={onCreateDelegation}
        onApproveDelegation={onApproveDelegation}
        onCancelDelegation={onCancelDelegation}
        onRetryDelegation={onRetryDelegation}
        onApplyDelegation={onApplyDelegation}
        onReconnectProvider={onReconnectProvider}
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
