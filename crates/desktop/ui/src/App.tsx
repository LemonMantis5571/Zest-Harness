import { useCallback, useEffect, useRef, useState } from "react";

import { AuthSuccess } from "@/components/AuthSuccess";
import { ChatScreen } from "@/components/ChatScreen";
import { ChatSkeleton } from "@/components/ChatSkeleton";
import { ConversationRecoveryDialog } from "@/components/ConversationRecoveryDialog";
import { ProviderPicker } from "@/components/ProviderPicker";
import { WaitingScreen } from "@/components/WaitingScreen";
import { toast, Toaster } from "@/components/ui/toast";
import { admitAttachments } from "@/lib/attachmentLimits";
import { getBackend } from "@/lib/backend";
import {
  fallbackOnFailure,
  ignoreExpectedFailure,
} from "@/lib/backgroundFailure";
import {
  findApprovalTool,
  initialChatUiState,
  markApprovalRunning,
  reduceChatEvent,
  reduceChatEvents,
  restoreApprovalCard,
  retireApprovalCard,
  type ChatUiState,
} from "@/lib/chatReducer";
import { loadDraft, saveDraft } from "@/lib/drafts";
import {
  busyTurnMessage,
  conversationRecovery,
  isWorkspaceProblem,
  rawInvokeError,
  shouldOfferProviderReconnect,
  workspaceProblemMessage,
  type ConversationRecovery,
} from "@/lib/invokeErrors";
import { isLongTurn } from "@/lib/notificationPolicy";
import { isWindowActuallyActive, notifyWhenAway } from "@/lib/notifications";
import { revealCount } from "@/lib/reveal";
import {
  DEFAULT_CODEX_MODEL,
  DEFAULT_EFFORT,
  type EffortId,
} from "@/lib/models";
import {
  markProviderVerified,
  markProviderVerifyFailed,
  recentVerifyFailed,
} from "@/lib/providerVerify";
import {
  isProviderReady,
  pickProviderFallback,
  pickReadyProvider,
} from "@/lib/providerSelection";
import {
  effortFromSession,
  mergeSessionOptions,
  rollbackSessionOptions,
} from "@/lib/sessionOptions";
import {
  createNavigationHistory,
  pushNavigation,
  travelNavigation,
  type NavigationDestination,
  type NavigationHistory,
  type ShellPanel,
} from "@/lib/navigationHistory";
import { markStartup, measureStartup } from "@/lib/startupPerf";
import {
  reduceThreadActivity,
  type ThreadActivityMap,
} from "@/lib/threadActivity";
import {
  pendingInputToQueuedTurn,
  updateThreadTurn,
  removeThreadTurn,
  type ThreadQueueMap,
} from "@/lib/threadQueue";
import type {
  ApprovalChoice,
  ApprovalMode,
  ChatEvent,
  ChatMessage,
  DelegationCreateInput,
  DelegationEvent,
  DelegationJob,
  GitContext,
  PreparedAttachment,
  ProviderRow,
  SessionInfo,
  SessionMeta,
  SessionWarning,
  ToolPart,
  UserAttachmentChip,
  UserProfile,
  WorkspaceChange,
  WorkspaceReview,
} from "@/lib/types";
import { applyFont, getSavedFontId } from "@/lib/fonts";
import { cn } from "@/lib/utils";

type Screen =
  | "boot"
  | "picker"
  | "waiting"
  | "auth-success"
  /** The chat shell. Profile, Usage, and Customize render inside it. */
  | "chat";

const POLL_MS = 1500;
const POLL_MAX_TICKS = 120;
/** A broken desktop command must not leave the launch skeleton on screen forever. */
const BOOT_TIMEOUT_MS = 15_000;
/** Consecutive status failures are enough to stop a sign-in that can no longer be observed. */
const LOGIN_STATUS_FAILURE_LIMIT = 5;
/** How often to re-read .git/HEAD while a chat is open. */
const BRANCH_POLL_MS = 2000;
/** PR metadata is remote-backed and changes much less often than .git/HEAD. */
const GIT_CONTEXT_POLL_MS = 30_000;

/**
 * Bound the UI wait even when a Tauri command or an embedded runtime never
 * resolves. The underlying command cannot be cancelled from the webview, but
 * callers only act on the bounded promise, so a late result cannot keep the
 * launch screen stuck.
 */
function withTimeout<T>(promise: Promise<T>, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(
      () => reject(new Error(`zest_boot_timeout: ${label}`)),
      BOOT_TIMEOUT_MS
    );
    Promise.resolve(promise).then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      }
    );
  });
}

async function showAttention(
  title: string,
  description: string,
  type: "warning" | "success"
) {
  if (await isWindowActuallyActive()) {
    toast.add({ type, title, description });
  } else {
    await notifyWhenAway(title, description);
  }
}

/**
 * What "Build plan" says on the user's behalf.
 *
 * It lands in the transcript as their message, so it is worded as something a
 * person would say — clicking the button *is* saying this, and the transcript
 * should not contain instructions they never gave.
 */
const BUILD_PLAN_PROMPT =
  "Build the plan. Delegate the steps that suit a configured external worker; " +
  "build the rest here.";

function newId(prefix: string) {
  return `${prefix}-${crypto.randomUUID()}`;
}

function normalizedWorkspacePath(value: string) {
  return value
    .replace(/^\\\\\?\\/, "")
    .replaceAll("\\", "/")
    .replace(/\/+$/, "")
    .toLowerCase();
}

/** Collapse adjacent text/thinking deltas for the same message before reduce. */
type ChatDeltaEvent = Extract<
  ChatEvent,
  { kind: "text_delta" | "thinking_delta" }
>;

function mergeAdjacentDeltas(events: ChatDeltaEvent[]): ChatDeltaEvent[] {
  const out: ChatDeltaEvent[] = [];
  for (const event of events) {
    const last = out[out.length - 1];
    if (
      last &&
      (event.kind === "text_delta" || event.kind === "thinking_delta") &&
      last.kind === event.kind &&
      "message_id" in last &&
      "message_id" in event &&
      last.message_id === event.message_id &&
      last.turn_id === event.turn_id &&
      last.thread_id === event.thread_id &&
      last.session_id === event.session_id &&
      "text" in last &&
      "text" in event
    ) {
      out[out.length - 1] = { ...last, text: last.text + event.text };
    } else {
      out.push(event);
    }
  }
  return out;
}

function normalizeMessages(raw: ChatMessage[] | undefined): ChatMessage[] {
  if (!raw?.length) return [];
  // Rust terminalizes interrupted tools on load; keep a belt-and-suspenders pass.
  return raw.map((msg) => {
    if (msg.role === "user") {
      return {
        id: msg.id,
        role: "user",
        text: msg.text ?? "",
        attachments: msg.attachments,
      };
    }
    return {
      id: msg.id,
      role: "assistant",
      text: msg.text ?? "",
      thinking: msg.thinking ?? "",
      tools: (msg.tools ?? []).map((t): ToolPart => {
        const status =
          t.status === "awaiting_approval" || t.status === "running"
            ? "error"
            : t.status === "done" || t.status === "error"
              ? t.status
              : "done";
        return {
          id: t.id,
          name: t.name,
          status,
          summary:
            t.status === "awaiting_approval"
              ? t.summary
                ? `${t.summary} (approval interrupted)`
                : "approval interrupted"
              : t.status === "running"
                ? t.summary
                  ? `${t.summary} (interrupted)`
                  : "tool interrupted"
                : t.summary,
          path: t.path,
          diff: t.diff,
          metadata: t.metadata,
        };
      }),
      error: msg.error,
      providerSelection: msg.providerSelection ?? msg.provider_selection,
      // Persisted, so a reopened plan still renders as a plan.
      command: msg.command,
      streaming: false,
    };
  });
}

function formatInvokeError(err: unknown): string {
  // Ahead of everything else: a folder Zest cannot write in fails whatever it
  // was asked to do, so the generic classifications below would only describe
  // the symptom. It also has to outrank the "permission" branch further down,
  // which says nothing about which folder or how to change it.
  if (isWorkspaceProblem(err)) return workspaceProblemMessage(err);
  const raw = rawInvokeError(err).toLowerCase();
  if (raw.includes("busy") || raw.includes("already in progress")) {
    return busyTurnMessage(err);
  }
  if (raw.includes("thread_provider_unknown")) {
    return "Choose a provider before reopening this older chat.";
  }
  if (raw.includes("is not configured for this project")) {
    return "The original provider is not configured for this project.";
  }
  if (raw.includes("model") && raw.includes("not supported")) {
    return "The selected model is unavailable for this account. Choose another model.";
  }
  if (
    raw.includes("does not include the selected provider")
  ) {
    return "This project uses its own provider settings. Choose a provider configured for this project, or add it to zest.toml.";
  }
  if (raw.includes("no provider configured") || raw.includes("unknown provider")) {
    return "This project has no usable provider configured. Add one in Settings or zest.toml, then try again.";
  }
  if (
    raw.includes("not configured") ||
    raw.includes("configure") ||
    raw.includes("set an api key") ||
    raw.includes("add an api key")
  ) {
    return "Configure this provider in Settings, then try again.";
  }
  if (
    raw.includes("rate limit") ||
    raw.includes("too many requests") ||
    raw.includes("overloaded") ||
    raw.includes("429")
  ) {
    return "This provider is busy or rate-limited. Wait a moment and try again.";
  }
  if (
    raw.includes("could not reach") ||
    raw.includes("connection refused") ||
    raw.includes("unreachable")
  ) {
    return "Could not reach the provider. Check your connection and try again.";
  }
  if (
    raw.includes("auth") ||
    raw.includes("sign in") ||
    raw.includes("connect again") ||
    raw.includes("api key") ||
    raw.includes("credential")
  ) {
    return "This provider needs to be connected before continuing.";
  }
  if (raw.includes("context") || raw.includes("token limit") || raw.includes("too long")) {
    return "This conversation is too long for the selected model. Start a new conversation or shorten the request.";
  }
  if (raw.includes("permission") || raw.includes("access denied")) {
    return "Zest does not have permission to complete that action.";
  }
  if (isDroppedApprovalError(raw)) {
    return "That request is no longer waiting — the turn it belonged to has ended.";
  }
  return "Something went wrong. Try again.";
}

/**
 * The backend rejected an interaction because its waiter is gone: the turn
 * ended or the one-shot request was already taken or cleared. Neither is
 * retryable, so the UI should retire it rather than raise another alert.
 */
function isDroppedApprovalError(raw: string): boolean {
  return (
    raw.includes("no pending approval") ||
    raw.includes("no active turn for approval") ||
    raw.includes("no pending question") ||
    raw.includes("no active turn for question") ||
    raw.includes("no turn in progress")
  );
}

/**
 * The account signed in fine, but cannot use the model that was asked for.
 *
 * Worth telling apart from every other verification failure: the credentials
 * are good, so "connect again" is useless advice and marking the provider as
 * unverified only disables Continue. A ChatGPT-account sign-in is refused
 * `gpt-5.6-sol` outright, which used to leave first-run users with no way past
 * the picker at all.
 */
function isModelUnsupported(err: unknown): boolean {
  const message = rawInvokeError(err).toLowerCase();
  return (
    message.includes("is not supported when using") ||
    (message.includes("model") && message.includes("not supported"))
  );
}

/**
 * A picker failure plus the one thing the picker needs to decide from it:
 * whether the way out is a different folder or a different account.
 */
type PickerError = {
  message: string;
  workspace: boolean;
};

type EnterChatOptions = {
  /** Prevent a superseded login callback from applying a late session. */
  isCurrent?: () => boolean;
};

function pickerErrorFrom(err: unknown): PickerError {
  return { message: formatInvokeError(err), workspace: isWorkspaceProblem(err) };
}

function startupPickerErrorFrom(err: unknown): PickerError {
  const raw = rawInvokeError(err).toLowerCase();
  if (raw.includes("zest_boot_timeout")) {
    return {
      message:
        "Zest could not finish opening its desktop runtime. Restart Zest, then try again.",
      workspace: false,
    };
  }
  if (
    raw.includes("404") ||
    raw.includes("unknown command") ||
    raw.includes("command not found") ||
    raw.includes("failed to fetch") ||
    raw.includes("ipc")
  ) {
    return {
      message:
        "Zest could not reach its desktop runtime. Restart Zest, then try again.",
      workspace: false,
    };
  }
  return pickerErrorFrom(err);
}

const backend = getBackend();

export default function App() {
  const [screen, setScreen] = useState<Screen>("boot");
  /** Bumped to ask ChatScreen to open Settings at the User section. */
  const [settingsRequest, setSettingsRequest] = useState(0);
  /** Bumped to open Settings without forcing the User section. */
  const [settingsOpenRequest, setSettingsOpenRequest] = useState(0);
  /**
   * Which panel is showing in place of the transcript, or null for the
   * transcript itself. Each Customize tab is its own history entry.
   */
  const [shellPanel, setShellPanel] = useState<ShellPanel | null>(null);
  const [navigation, setNavigation] = useState<NavigationHistory>(createNavigationHistory);
  const navigationRef = useRef<NavigationHistory>(createNavigationHistory());
  const [providerSwitchRequest, setProviderSwitchRequest] = useState(0);
  const [providers, setProviders] = useState<ProviderRow[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  /**
   * A background verification that failed after the chat had already opened.
   *
   * Shown in the chat rather than bouncing back to the picker: the session is
   * real and the transcript is readable, so throwing the user out would lose
   * more than the warning gains.
   */
  const [sessionWarning, setSessionWarning] = useState<SessionWarning | null>(null);
  const [pickerError, setPickerError] = useState<PickerError | null>(null);
  const [continuing, setContinuing] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [pendingConversationRecovery, setPendingConversationRecovery] = useState<{
    recovery: ConversationRecovery;
    root: string | null;
  } | null>(null);
  const [conversationRecoveryBusy, setConversationRecoveryBusy] = useState(false);

  const [waitingTitle, setWaitingTitle] = useState("Sign in");
  const [waitingBody, setWaitingBody] = useState(
    "Finish in your browser. This window will update when you’re done."
  );
  const [waitingHint, setWaitingHint] = useState("Waiting for browser sign-in…");
  const [waitingError, setWaitingError] = useState<string | null>(null);

  const [session, setSession] = useState<SessionInfo | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<PreparedAttachment[]>([]);
  const [workspacePath, setWorkspacePath] = useState<string | null>(null);
  const [workspaceReview, setWorkspaceReview] = useState<WorkspaceReview | null>(null);
  const [workspaceChange, setWorkspaceChange] = useState<WorkspaceChange | null>(null);
  const [branch, setBranch] = useState<string | null>(null);
  const [gitContext, setGitContext] = useState<GitContext | null>(null);
  const [profile, setProfile] = useState<UserProfile>({
    displayName: "",
    avatarDataUrl: "",
  });
  const [sending, setSending] = useState(false);
  const [threadActivity, setThreadActivity] = useState<ThreadActivityMap>({});
  const [delegationJobs, setDelegationJobs] = useState<DelegationJob[]>([]);
  const [threadQueues, setThreadQueues] = useState<ThreadQueueMap>({});
  const [resumingQueuedThread, setResumingQueuedThread] = useState<string | null>(null);
  const [compacting, setCompacting] = useState(false);
  const [model, setModel] = useState(DEFAULT_CODEX_MODEL);
  const [effort, setEffort] = useState<EffortId>(DEFAULT_EFFORT);
  // Mirrors DESKTOP_DEFAULT_MODE in Rust; reconciled on session start.
  const [approvalModeState, setApprovalModeState] =
    useState<ApprovalMode>("auto");
  const [optionsUpdating, setOptionsUpdating] = useState(false);

  const commitNavigation = useCallback((next: NavigationHistory) => {
    navigationRef.current = next;
    setNavigation(next);
  }, []);

  const applyNavigationDestination = useCallback(
    (destination: NavigationDestination) => {
      switch (destination.kind) {
        case "chat":
          setShellPanel(null);
          setScreen("chat");
          break;
        // Profile, Usage, and Customize are all reached from the sidebar, so
        // they render inside the chat shell rather than replacing it.
        case "profile":
        case "usage":
        case "customize":
          setShellPanel(destination);
          setScreen("chat");
          break;
        case "settings":
          // Settings is an overlay, so whichever panel is open stays behind it
          // and Back returns there.
          setScreen("chat");
          if (destination.focusUser) {
            setSettingsRequest((request) => request + 1);
          } else {
            setSettingsOpenRequest((request) => request + 1);
          }
          break;
      }
    },
    []
  );

  const navigateTo = useCallback(
    (destination: NavigationDestination) => {
      const next = pushNavigation(navigationRef.current, destination);
      if (next === navigationRef.current) return;
      commitNavigation(next);
      applyNavigationDestination(destination);
    },
    [applyNavigationDestination, commitNavigation]
  );

  const navigateHistory = useCallback(
    (direction: -1 | 1) => {
      const moved = travelNavigation(navigationRef.current, direction);
      if (!moved) return;
      commitNavigation(moved.history);
      applyNavigationDestination(moved.destination);
    },
    [applyNavigationDestination, commitNavigation]
  );

  const navigateBack = useCallback(() => navigateHistory(-1), [navigateHistory]);
  const navigateForward = useCallback(() => navigateHistory(1), [navigateHistory]);
  const closeSettingsFromHistory = useCallback(() => {
    if (navigationRef.current.current?.kind === "settings") navigateBack();
  }, [navigateBack]);
  /**
   * Leaving a panel walks back past the run of Customize tabs it may have
   * opened.
   *
   * A plain Back inside Customize would land on the previous tab, which reads
   * as a Back button that does nothing — the user asked to leave, not to change
   * tab. Other panels are a single entry, so one step is enough.
   */
  const closeShellPanel = useCallback(() => {
    if (navigationRef.current.current?.kind !== "customize") {
      navigateBack();
      return;
    }
    while (navigationRef.current.current?.kind === "customize") {
      const before = navigationRef.current;
      navigateBack();
      if (navigationRef.current === before) {
        // Nothing behind it in history — Customize was the first view.
        navigateTo({ kind: "chat" });
        return;
      }
    }
  }, [navigateBack, navigateTo]);

  // The first loaded chat establishes the root of the app-view history. Boot,
  // provider selection, and sign-in progress are lifecycle states, not places
  // users should be sent back to by these controls.
  useEffect(() => {
    if (screen !== "chat" || navigationRef.current.current) return;
    commitNavigation(pushNavigation(navigationRef.current, { kind: "chat" }));
  }, [commitNavigation, screen]);

  useEffect(() => {
    applyFont(getSavedFontId());
  }, []);
  /**
   * The mode Plan mode interrupted, restored by Build.
   *
   * `null` means planning was never entered from somewhere else this session —
   * the app opened in Plan, or it was restored from disk. Build then falls back
   * to the desktop default rather than inventing a permission level.
   */
  const modeBeforePlanRef = useRef<ApprovalMode | null>(null);

  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  const pollRef = useRef<number | null>(null);
  /** Invalidates callbacks from a canceled or superseded browser login. */
  const loginAttemptRef = useRef(0);
  /** Prevent duplicate vendor login processes while the first spawn is pending. */
  const loginStartingRef = useRef(false);
  const activeAssistantId = useRef<string | null>(null);
  /** Live UI projections for chats that continue while another one is open. */
  const chatStatesRef = useRef(new Map<string, ChatUiState>());
  /** Keep terminal change snapshots available while another chat is visible. */
  const workspaceChangesRef = useRef(new Map<string, WorkspaceChange>());
  const messagesRef = useRef<ChatMessage[]>([]);
  messagesRef.current = messages;
  const sendingRef = useRef(sending);
  sendingRef.current = sending;
  const threadActivityRef = useRef<ThreadActivityMap>({});
  const threadQueuesRef = useRef<ThreadQueueMap>({});
  const threadIdRef = useRef<string | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const currentTurnIdRef = useRef<string | null>(null);
  const turnStartedAtByThreadRef = useRef(new Map<string, number>());
  const notifiedApprovalIdsByThreadRef = useRef(new Map<string, Set<string>>());
  const resolvingApprovalIdsRef = useRef(new Set<string>());
  const resolvingQuestionIdsRef = useRef(new Set<string>());
  const notifiedDelegationEventsRef = useRef(new Set<string>());
  const compactionInFlightRef = useRef(false);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const attachmentsRef = useRef(attachments);
  attachmentsRef.current = attachments;
  const optionsUpdatingRef = useRef(false);
  const modelRef = useRef(model);
  modelRef.current = model;
  const effortRef = useRef(effort);
  effortRef.current = effort;
  /** Set just before send; applied when the matching user_message event arrives. */
  const pendingUserAttachmentsRef = useRef(new Map<string, UserAttachmentChip[]>());
  const enterChatRef = useRef<
    (providerId: string, options?: EnterChatOptions) => Promise<SessionInfo>
  >(
    async () => {
      throw new Error("session start is not ready yet");
    }
  );

  const recordThreadActivity = useCallback((event: ChatEvent) => {
    const previous = threadActivityRef.current;
    const next = reduceThreadActivity(previous, event, Date.now());
    if (next === previous) return;
    threadActivityRef.current = next;
    setThreadActivity(next);
  }, []);

  const replaceDelegationJob = useCallback((job: DelegationJob) => {
    setDelegationJobs((current) => {
      const next = current.filter((candidate) => candidate.jobId !== job.jobId);
      next.push(job);
      next.sort((left, right) => right.updatedAt - left.updatedAt);
      return next;
    });
  }, []);

  const handleDelegationEvent = useCallback(
    (event: DelegationEvent) => {
      const activeRoot = session?.root;
      if (
        !activeRoot ||
        normalizedWorkspacePath(event.job.projectRoot) !== normalizedWorkspacePath(activeRoot)
      ) {
        return;
      }
      replaceDelegationJob(event.job);
      if (
        !["ready_to_apply", "changes_requested", "blocked", "failed", "applied", "cancelled"].includes(
          event.kind
        )
      ) {
        return;
      }
      const notificationId = `${event.kind}:${event.job.jobId}:${event.job.updatedAt}`;
      if (notifiedDelegationEventsRef.current.has(notificationId)) return;
      notifiedDelegationEventsRef.current.add(notificationId);
      const copy: Record<string, [string, "warning" | "success"]> = {
        ready_to_apply: ["Changes ready to apply", "success"],
        changes_requested: ["Review requested changes", "warning"],
        blocked: ["Delegation needs attention", "warning"],
        failed: ["Delegation failed", "warning"],
        applied: ["Accepted changes applied", "success"],
        cancelled: ["Delegation cancelled", "warning"],
      };
      const [title, type] = copy[event.kind as keyof typeof copy];
      void showAttention(title, event.job.title, type);
    },
    [replaceDelegationJob, session?.root]
  );

  const updateQueuedTurn = useCallback(
    (threadId: string, turnId: string, text: string) => {
      void backend.updateQueuedInput(threadId, turnId, text).catch((error) => {
        toast.add({
          type: "error",
          title: "Could not edit queued message",
          description: formatInvokeError(error),
        });
      });
    },
    []
  );

  const discardQueuedTurn = useCallback(
    (threadId: string, turnId: string) => {
      void backend.removeQueuedInput(threadId, turnId).catch((error) => {
        toast.add({
          type: "error",
          title: "Could not remove queued message",
          description: formatInvokeError(error),
        });
      });
    },
    []
  );

  const resumeQueuedMessages = useCallback((threadId: string) => {
    if (resumingQueuedThread !== null) return;
    setResumingQueuedThread(threadId);
    void backend
      .resumeQueuedInputs(threadId)
      .catch((error) => {
        toast.add({
          type: "error",
          title: "Could not resume queued messages",
          description: formatInvokeError(error),
        });
      })
      .finally(() => setResumingQueuedThread(null));
  }, [resumingQueuedThread]);

  const loadProviders = useCallback(async (prefer?: string | null) => {
    const rows = await withTimeout(backend.listProviders(), "provider list");
    setProviders(rows);
    setSelectedId((current) => {
      const preferId = prefer ?? current;
      if (preferId && rows.some((p) => p.id === preferId)) return preferId;
      const ready = rows.find((row) => isProviderReady(row, recentVerifyFailed));
      return ready?.id ?? rows[0]?.id ?? null;
    });
    return rows;
  }, []);

  const stopPolling = useCallback(() => {
    if (pollRef.current != null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, []);

  const finishVerifiedLogin = useCallback(
    async (row: ProviderRow, attempt?: number) => {
      const isCurrent = () =>
        attempt == null || attempt === loginAttemptRef.current;
      if (!isCurrent()) return;

      // File presence is not a working session — prove it, then open chat
      // without an extra Continue click.
      setWaitingHint("Checking the sign-in works…");
      try {
        await withTimeout(backend.verifyProvider(row.id), "provider check");
        if (!isCurrent()) return;
      } catch (err) {
        if (!isCurrent()) return;
        // A refused *model* is not a refused sign-in. Signing in again cannot
        // change which models the plan includes, so send them into chat — where
        // the model picker is — instead of back to a Reconnect button that
        // cannot help.
        if (isModelUnsupported(err)) {
          markProviderVerified(row.id);
          setSessionWarning({
            providerId: row.id,
            message: "You're signed in, but this account cannot use that model. Choose another model below.",
            offerReconnect: false,
          });
          setWaitingHint("Opening chat…");
          try {
            await enterChatRef.current(row.id, { isCurrent });
            if (!isCurrent()) return;
          } catch {
            if (!isCurrent()) return;
            setPickerError({
              message: "Could not open this provider. Try again.",
              workspace: false,
            });
            setScreen("picker");
          }
          return;
        }
        markProviderVerifyFailed(row.id);
        setWaitingHint("Provider unavailable");
        setWaitingError(`Could not connect to ${row.label}. Try connecting again.`);
        return;
      }
      markProviderVerified(row.id);
      setWaitingHint("Opening chat…");
      try {
        await enterChatRef.current(row.id, { isCurrent });
        if (!isCurrent()) return;
      } catch (err) {
        if (!isCurrent()) return;
        markProviderVerifyFailed(row.id);
        setPickerError(startupPickerErrorFrom(err));
        setScreen("picker");
      }
    },
    []
  );

  const startWaitingPoll = useCallback(() => {
    stopPolling();
    const attempt = ++loginAttemptRef.current;
    let ticks = 0;
    setWaitingHint("Waiting for browser sign-in…");
    setWaitingError(null);

    let consecutiveFailures = 0;
    let pollInFlight = false;
    pollRef.current = window.setInterval(async () => {
      if (pollInFlight || attempt !== loginAttemptRef.current) return;
      pollInFlight = true;
      ticks += 1;
      try {
        const rows = await loadProviders(selectedIdRef.current);
        if (attempt !== loginAttemptRef.current) return;
        consecutiveFailures = 0;
        const row = rows.find((p) => p.id === selectedIdRef.current);
        // Ready, or a session file appeared but looked incomplete — either way
        // prove it with a probe instead of spinning on "Waiting…".
        const fileAppeared =
          row?.statusKind === "ready" ||
          (row?.statusKind === "not_logged_in" &&
            row.detail.toLowerCase().includes("incomplete"));
        if (row && fileAppeared) {
          stopPolling();
          await finishVerifiedLogin(row, attempt);
          return;
        }

        const login = await withTimeout(backend.loginStatus(), "login status");
        if (attempt !== loginAttemptRef.current) return;
        if (login.state === "exited") {
          stopPolling();
          setWaitingHint("Sign-in stopped");
          setWaitingError(
            login.detail ??
              "The sign-in did not finish. Try again."
          );
          return;
        }
      } catch {
        if (attempt !== loginAttemptRef.current) return;
        consecutiveFailures += 1;
        if (consecutiveFailures >= LOGIN_STATUS_FAILURE_LIMIT) {
          stopPolling();
          setWaitingHint("Could not check sign-in");
          setWaitingError("Zest could not check the sign-in status. Return to providers and try again.");
          return;
        }
      } finally {
        pollInFlight = false;
      }

      if (attempt !== loginAttemptRef.current) return;
      if (ticks >= POLL_MAX_TICKS) {
        stopPolling();
        setWaitingHint("Still waiting");
        setWaitingError("Complete sign-in in your browser or cancel.");
      }
    }, POLL_MS);
  }, [finishVerifiedLogin, loadProviders, stopPolling]);

  const deltaQueueRef = useRef<ChatDeltaEvent[]>([]);
  const deltaRafRef = useRef<number | null>(null);

  const refreshCheckpointMetadata = useCallback(() => {
    const attempt = (remaining: number) => {
      void backend
        .sessionInfo()
        .then((info) => {
          if (info && info.sessionId === sessionIdRef.current) {
            setSession((current) =>
              current
                ? { ...current, checkpoints: info.checkpoints }
                : current
            );
            return;
          }
          // `done` is emitted just before the Rust session slot is released.
          // Retry briefly so the new checkpoint is visible without racing the
          // turn finalizer.
          if (remaining > 0) {
            window.setTimeout(() => attempt(remaining - 1), 40);
          }
        })
        .catch((error) => {
          /* checkpoint metadata is best-effort UI state */
          ignoreExpectedFailure(error, "refresh checkpoint metadata");
        });
    };

    window.setTimeout(() => attempt(4), 0);
  }, []);

  const maybeAutoCompact = useCallback(() => {
    const targetSessionId = sessionIdRef.current;
    if (!targetSessionId || compactionInFlightRef.current || sendingRef.current) {
      return;
    }

    const attempt = (remaining: number) => {
      if (
        targetSessionId !== sessionIdRef.current ||
        compactionInFlightRef.current ||
        sendingRef.current
      ) {
        return;
      }

      void backend
        .contextUsage()
        .then((usage) => {
          if (
            targetSessionId !== sessionIdRef.current ||
            compactionInFlightRef.current ||
            sendingRef.current
          ) {
            return;
          }
          if (!usage.shouldAutoCompact) return;

          compactionInFlightRef.current = true;
          setCompacting(true);
          void backend
            .compactContext()
            .then(async (result) => {
              try {
                const info = await backend.sessionInfo();
                if (info && info.sessionId === sessionIdRef.current) {
                  setSession((current) =>
                    current
                      ? { ...current, checkpoints: info.checkpoints }
                      : current
                  );
                }
              } catch {
                /* checkpoint metadata refresh is best-effort */
              }
              // Trimming long tool output and summarizing are different things
              // to have happened to a conversation, so they get different copy.
              toast.add(
                result.prunedOnly
                  ? {
                      type: "success",
                      title: "Trimmed long tool output",
                      description: `Shortened ${result.resultsPruned} long tool ${
                        result.resultsPruned === 1 ? "result" : "results"
                      }. The conversation was kept as it was; the full output is in the restore point.`,
                    }
                  : {
                      type: "success",
                      title: "Conversation compacted automatically",
                      description:
                        "You can restore the conversation from before compaction.",
                    }
              );
            })
            .catch((err) => {
              toast.add({
                type: "warning",
                title: "Automatic compaction paused",
                description: formatInvokeError(err),
              });
            })
            .finally(() => {
              compactionInFlightRef.current = false;
              setCompacting(false);
            });
        })
        .catch(() => {
          // `done` arrives just before Rust releases the turn slot. Retry the
          // read briefly instead of treating that expected race as a failure.
          if (remaining > 0) {
            window.setTimeout(() => attempt(remaining - 1), 60);
          }
        });
    };

    window.setTimeout(() => attempt(6), 0);
  }, []);

  const applyChatEventNow = useCallback((event: ChatEvent) => {
    const threadKey = event.thread_id;
    const isCurrent = threadIdRef.current === threadKey;

    // Queue and job events are authoritative projections, not transcript
    // events. Handle them before the chat reducer so an old turn id cannot
    // make a durable queue item disappear from the compact composer list.
    if (event.kind === "input_queued") {
      const current = threadQueuesRef.current[threadKey] ?? [];
      if (!current.some((turn) => turn.id === event.input.id)) {
        const next = {
          ...threadQueuesRef.current,
          [threadKey]: [
            ...current,
            pendingInputToQueuedTurn(event.input, threadKey),
          ],
        };
        threadQueuesRef.current = next;
        setThreadQueues(next);
      }
      return;
    }
    if (event.kind === "input_updated") {
      const next = updateThreadTurn(
        threadQueuesRef.current,
        threadKey,
        event.input_id,
        event.text
      );
      if (next !== threadQueuesRef.current) {
        threadQueuesRef.current = next;
        setThreadQueues(next);
      }
      return;
    }
    if (event.kind === "input_removed") {
      const next = removeThreadTurn(
        threadQueuesRef.current,
        threadKey,
        event.input_id
      );
      if (next !== threadQueuesRef.current) {
        threadQueuesRef.current = next;
        setThreadQueues(next);
      }
      return;
    }
    if (event.kind === "job_completed") {
      void showAttention(
        "Background job finished",
        `${event.label} · ${event.status}`,
        event.status === "completed" ? "success" : "warning"
      );
      return;
    }

    if (isCurrent && event.kind === "workspace_changed") {
      setWorkspaceChange(event.change);
    }
    if (event.kind === "workspace_changed") {
      workspaceChangesRef.current.set(threadKey, event.change);
    }
    const previous =
      chatStatesRef.current.get(threadKey) ??
      initialChatUiState([], { threadId: threadKey });
    const { state: reduced, effects } = reduceChatEvent(
      {
        ...previous,
        // Session ids are runtime identities. A chat may be reopened with a
        // new runtime while its old turn is still finishing, so thread id is
        // the durable routing key for streamed events.
        sessionId: null,
        threadId: threadKey,
      },
      event,
      { newId }
    );
    // Attach filename chips from the send that produced this user event.
    // History reload omits them until thread persistence gains attachments.
    let nextMessages = reduced.messages;
    if (event.kind === "user") {
      const chips = pendingUserAttachmentsRef.current.get(threadKey);
      pendingUserAttachmentsRef.current.delete(threadKey);
      if (chips) {
        nextMessages = nextMessages.map((m) =>
          m.role === "user" && m.id === event.message_id
            ? { ...m, attachments: chips }
            : m
        );
      }
    }

    const state: ChatUiState = {
      ...reduced,
      messages: nextMessages,
      sessionId: null,
      threadId: threadKey,
    };
    chatStatesRef.current.set(threadKey, state);

    if (isCurrent) {
      const prevSending = sendingRef.current;
      messagesRef.current = nextMessages;
      activeAssistantId.current = state.activeAssistantId;
      currentTurnIdRef.current = state.currentTurnId;
      setMessages(nextMessages);
      if (state.sending !== prevSending) {
        sendingRef.current = state.sending;
        setSending(state.sending);
      }
    }
    if (effects.errorToast) {
      toast.add({
        type: "error",
        title: "Request failed",
        description: effects.errorToast,
      });
    }
    if (effects.warningToast) {
      toast.add({
        type: "warning",
        title: "Chat history not saved",
        description: effects.warningToast,
      });
    }

    if (event.kind === "user" && state.currentTurnId === event.turn_id) {
      turnStartedAtByThreadRef.current.set(threadKey, Date.now());
      notifiedApprovalIdsByThreadRef.current.set(threadKey, new Set());
    }

    if (event.kind === "approval_needed") {
      const notified =
        notifiedApprovalIdsByThreadRef.current.get(threadKey) ?? new Set<string>();
      notifiedApprovalIdsByThreadRef.current.set(threadKey, notified);
      if (!notified.has(event.approval_id)) {
        notified.add(event.approval_id);
        const description = event.summary
          ? `${event.tool_name}: ${event.summary}`
          : `${event.tool_name} is waiting for your approval.`;
        void showAttention("Approval needed", description, "warning");
      }
    }

    if (event.kind === "question_needed") {
      void showAttention("Input needed", event.prompt, "warning");
    }

    if (event.kind === "done") {
      const startedAt = turnStartedAtByThreadRef.current.get(threadKey);
      if (startedAt != null && isLongTurn(Date.now() - startedAt)) {
        void showAttention("Response ready", "Zest finished the turn.", "success");
      }
      turnStartedAtByThreadRef.current.delete(threadKey);
      notifiedApprovalIdsByThreadRef.current.delete(threadKey);
      if (isCurrent) {
        refreshCheckpointMetadata();
        maybeAutoCompact();
      }
    }

    if (event.kind === "error" || event.kind === "cancelled") {
      turnStartedAtByThreadRef.current.delete(threadKey);
      notifiedApprovalIdsByThreadRef.current.delete(threadKey);
    }

  }, [maybeAutoCompact, refreshCheckpointMetadata]);

  /**
   * Publish at most one transcript update per thread for a rendered frame.
   * Delta ordering is preserved within each thread, while background chats
   * continue to advance in their own projection without rerendering the open
   * transcript.
   */
  const applyChatDeltasNow = useCallback((events: readonly ChatDeltaEvent[]) => {
    if (events.length === 0) return;

    const byThread = new Map<string, ChatDeltaEvent[]>();
    for (const event of events) {
      const threadEvents = byThread.get(event.thread_id);
      if (threadEvents) threadEvents.push(event);
      else byThread.set(event.thread_id, [event]);
    }

    const currentThread = threadIdRef.current;
    let currentState: ChatUiState | null = null;
    for (const [threadKey, threadEvents] of byThread) {
      const previous =
        chatStatesRef.current.get(threadKey) ??
        initialChatUiState([], { threadId: threadKey });
      const { state: reduced } = reduceChatEvents(
        {
          ...previous,
          sessionId: null,
          threadId: threadKey,
        },
        threadEvents,
        { newId }
      );
      const state: ChatUiState = {
        ...reduced,
        sessionId: null,
        threadId: threadKey,
      };
      chatStatesRef.current.set(threadKey, state);
      if (threadKey === currentThread) currentState = state;
    }

    if (!currentState) return;
    messagesRef.current = currentState.messages;
    activeAssistantId.current = currentState.activeAssistantId;
    currentTurnIdRef.current = currentState.currentTurnId;
    setMessages(currentState.messages);
  }, []);

  const flushDeltaQueue = useCallback(
    (drainAll = false) => {
      deltaRafRef.current = null;
      const queued = deltaQueueRef.current;
      deltaQueueRef.current = [];
      // Merge adjacent text/thinking deltas before React reduce to cut renders.
      const merged = mergeAdjacentDeltas(queued);
      const frameEvents: ChatDeltaEvent[] = [];

      for (let i = 0; i < merged.length; i += 1) {
        const event = merged[i];
        if (!drainAll && event.kind === "text_delta") {
          const reveal = revealCount(event.text.length);
          if (reveal < event.text.length) {
            frameEvents.push({ ...event, text: event.text.slice(0, reveal) });
            // Order matters: the remainder and everything queued behind it
            // wait a frame together, or later text would overtake earlier text.
            deltaQueueRef.current = [
              { ...event, text: event.text.slice(reveal) },
              ...merged.slice(i + 1),
            ];
            break;
          }
        }
        frameEvents.push(event);
      }

      applyChatDeltasNow(frameEvents);

      if (deltaQueueRef.current.length > 0 && deltaRafRef.current == null) {
        deltaRafRef.current = window.requestAnimationFrame(() =>
          flushDeltaQueue()
        );
      }
    },
    [applyChatDeltasNow]
  );

  const handleChatEvent = useCallback(
    (event: ChatEvent) => {
      // This projection is intentionally ahead of the current-thread reducer:
      // events from a chat left running in the background are stale for the
      // transcript, but are the useful part of the sidebar status card.
      recordThreadActivity(event);
      if (event.kind === "text_delta" || event.kind === "thinking_delta") {
        deltaQueueRef.current.push(event);
        if (deltaRafRef.current == null) {
          deltaRafRef.current = window.requestAnimationFrame(() =>
            flushDeltaQueue()
          );
        }
        return;
      }
      // Non-delta events must see coalesced text first — and all of it, so a
      // partially revealed buffer never lands after the `done` that ended it.
      if (deltaQueueRef.current.length > 0) {
        if (deltaRafRef.current != null) {
          window.cancelAnimationFrame(deltaRafRef.current);
          deltaRafRef.current = null;
        }
        flushDeltaQueue(true);
      }
      applyChatEventNow(event);
    },
    [applyChatEventNow, flushDeltaQueue, recordThreadActivity]
  );

  /**
   * Keep the branch chip under the composer honest when the repo changes
   * branch outside Zest (`git checkout` in a terminal). Rust has no filesystem
   * watcher, so re-read .git/HEAD on an interval while a chat is open: a
   * ~50-byte read, and setBranch only fires when the name actually changes.
   */
  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    const tick = async () => {
      let next: string | null = null;
      try {
        next = await backend.gitBranch();
      } catch {
        /* same fallback the one-shot call sites use */
      }
      if (!cancelled) {
        setBranch((current) => (current === next ? current : next));
      }
    };
    void tick();
    const id = window.setInterval(tick, BRANCH_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [session]);

  const activeThreadId = session?.threadId;
  useEffect(() => {
    if (!activeThreadId) {
      setGitContext(null);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const next = await backend.gitContext();
        if (cancelled) return;
        setGitContext(next);
        const nextBranch = next.branch;
        if (nextBranch) {
          setBranch((current) => (current === nextBranch ? current : nextBranch));
        }
      } catch {
        /* GitHub CLI is optional; keep the last known context visible. */
      }
    };
    void tick();
    const id = window.setInterval(tick, GIT_CONTEXT_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [activeThreadId]);

  const applySession = useCallback((info: SessionInfo, opts?: { clearDraft?: boolean }) => {
    const prevThread = threadIdRef.current;
    if (prevThread && prevThread !== info.threadId) {
      saveDraft(prevThread, draftRef.current);
    }

    setSession(info);
    setWorkspacePath(info.isFreeChat ? null : info.root);
    setWorkspaceReview(null);
    setWorkspaceChange(workspaceChangesRef.current.get(info.threadId) ?? null);
    setGitContext(null);
    void backend.gitBranch().then(setBranch).catch(() => setBranch(null));
    void backend.gitContext().then(setGitContext).catch(() => setGitContext(null));
    setSelectedId(info.provider);
    setModel(info.model);
    setEffort(effortFromSession(info.effort, DEFAULT_EFFORT));
    // Rust owns the mode and it survives project switches, so read it back
    // rather than assuming the chip still matches.
    void backend
      .approvalMode()
      .then((mode) => setApprovalModeState(mode as ApprovalMode))
      .catch((error) => {
        /* keep the current chip; the picker is not worth an error toast */
        ignoreExpectedFailure(error, "restore approval mode chip");
      });
    const loadedMessages = normalizeMessages(info.messages);
    const cachedState = chatStatesRef.current.get(info.threadId);
    const liveState = cachedState?.sending ? cachedState : undefined;
    const chatState =
      liveState ?? initialChatUiState(loadedMessages, { threadId: info.threadId });
    const messages = liveState ? liveState.messages : loadedMessages;
    chatStatesRef.current.set(info.threadId, {
      ...chatState,
      messages,
      sessionId: null,
      threadId: info.threadId,
    });
    setMessages(messages);
    messagesRef.current = messages;
    activeAssistantId.current = chatState.activeAssistantId;
    currentTurnIdRef.current = chatState.currentTurnId;
    setSending(chatState.sending);
    sendingRef.current = chatState.sending;
    threadIdRef.current = info.threadId;
    sessionIdRef.current = info.sessionId;
    setAttachments([]);

    // Hydrate the compact queue from the Rust snapshot. React only renders
    // this projection; restart, navigation, and delivery remain core-owned.
    const hydratedQueue = info.pendingInputs.map((input) =>
      pendingInputToQueuedTurn(input, info.threadId)
    );
    const nextQueues = { ...threadQueuesRef.current };
    if (hydratedQueue.length > 0) {
      nextQueues[info.threadId] = hydratedQueue;
    } else {
      delete nextQueues[info.threadId];
    }
    threadQueuesRef.current = nextQueues;
    setThreadQueues(nextQueues);

    const savedDraft = opts?.clearDraft ? "" : loadDraft(info.threadId);
    const recoveryMessage = info.recovery
      ? messages.find(
          (message) =>
            message.role === "user" && message.id === info.recovery?.userMessageId
        )
      : undefined;
    const draft =
      !opts?.clearDraft && !savedDraft.trim() && recoveryMessage?.role === "user"
        ? recoveryMessage.text
        : savedDraft;

    if (opts?.clearDraft) {
      saveDraft(info.threadId, "");
      setDraft("");
    } else {
      if (draft !== savedDraft) saveDraft(info.threadId, draft);
      setDraft(draft);
    }

    if (info.warning) {
      toast.add({
        type: "warning",
        title: recoveryMessage ? "Previous turn ready to retry" : "Conversation updated",
        description: recoveryMessage
          ? "Its message is in the composer. Send it to try the turn again."
          : info.warning,
      });
    }

    setPickerError(null);
    setScreen("chat");
  }, []);

  const enterChat = useCallback(
    async (providerId: string, options?: EnterChatOptions) => {
      try {
        const info = await withTimeout(
          backend.startSession(providerId),
          "session start"
        );
        if (options?.isCurrent && !options.isCurrent()) return info;
        stopPolling();
        applySession(info);
        return info;
      } catch (err) {
        // Setup failures (for example, a new folder without a provider config)
        // are not authentication failures. Marking every start error as a
        // failed verification made the picker incorrectly say "Reconnect".
        if (providerId === "codex" && shouldOfferProviderReconnect(err)) {
          markProviderVerifyFailed(providerId);
        }
        throw err;
      }
    },
    [applySession, stopPolling]
  );
  enterChatRef.current = enterChat;

  const bootStarted = useRef(false);
  useEffect(() => {
    if (bootStarted.current) return;
    bootStarted.current = true;
    markStartup("boot-effect");

    // Before any turn is recorded: core buckets usage by day, and only the
    // The webview knows the local day. If this fails, keep booting and accept
    // UTC buckets for this session.
    void backend
      .setLocalOffset()
      .catch((error) => ignoreExpectedFailure(error, "set local timezone offset"));

    if (backend.mode === "fixture") {
      void (async () => {
        try {
          const info = await withTimeout(
            backend.startSession("fixture"),
            "fixture session"
          );
          applySession(info);
          setSending(true);
          sendingRef.current = true;
          await withTimeout(
            Promise.resolve(backend.boot?.(handleChatEvent)),
            "fixture boot"
          );
        } catch (err) {
          setPickerError(startupPickerErrorFrom(err));
          setScreen("picker");
        } finally {
          setSending(false);
          sendingRef.current = false;
        }
      })();
      return;
    }

    (async () => {
      try {
        const [rows, prefer, folder, userProfile] = await Promise.all([
          withTimeout(backend.listProviders(), "provider list"),
          withTimeout(backend.lastProvider(), "saved provider").catch((error) =>
            fallbackOnFailure(error, null, "load saved provider")
          ),
          withTimeout(backend.getWorkspaceFolder(), "workspace folder").catch((error) =>
            fallbackOnFailure(error, null, "load workspace folder")
          ),
          withTimeout(backend.getUserProfile(), "user profile").catch((error) =>
            fallbackOnFailure(
              error,
              { displayName: "", avatarDataUrl: "" },
              "load user profile"
            )
          ),
        ]);
        setProviders(rows);
        markStartup("backend-ready");
        measureStartup("backend-ready", "boot-effect");
        if (folder) setWorkspacePath(folder);
        setProfile(userProfile);
        void backend.gitBranch().then(setBranch).catch(() => setBranch(null));

        const ready = pickReadyProvider(rows, prefer, recentVerifyFailed);
        if (ready) {
          setSelectedId(ready.id);
          try {
            // startSession prepares gateway providers; catch here so a dead Codex
            // session lands on the picker with Connect instead of a chat error.
            await enterChat(ready.id);
            markStartup("session-ready");
            measureStartup("session-ready", "boot-effect");
            return;
          } catch (err) {
            setPickerError(startupPickerErrorFrom(err));
            setScreen("picker");
            markStartup("picker-error");
            measureStartup("picker-error", "boot-effect");
            return;
          }
        }

        const fallback = pickProviderFallback(rows, prefer);
        setSelectedId(fallback?.id ?? null);
        setScreen("picker");
        markStartup("picker-ready");
        measureStartup("picker-ready", "boot-effect");
      } catch (err) {
        setPickerError(startupPickerErrorFrom(err));
        setScreen("picker");
        markStartup("picker-error");
        measureStartup("picker-error", "boot-effect");
      }
    })();
  }, [applySession, enterChat, handleChatEvent]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    backend.onChatEvent(handleChatEvent).then((fn) => {
      if (disposed) {
        // Strict Mode: dispose late-resolving subscriptions immediately.
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleChatEvent]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    backend.onDelegationEvent(handleDelegationEvent).then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleDelegationEvent]);

  useEffect(() => {
    const onHandoff = (event: Event) => {
      const detail = (event as CustomEvent<{ summary?: string }>).detail;
      const summary = detail?.summary?.trim();
      if (!summary) return;
      setDraft(summary);
    };
    window.addEventListener("zest:delegation-handoff", onHandoff);
    return () => window.removeEventListener("zest:delegation-handoff", onHandoff);
  }, []);

  const activeDelegationRoot = session?.root;
  useEffect(() => {
    const activeRoot = activeDelegationRoot;
    if (!activeRoot) {
      setDelegationJobs([]);
      return;
    }
    let cancelled = false;
    void backend
      .listDelegationJobs()
      .then((jobs) => {
        if (!cancelled) setDelegationJobs(jobs);
      })
      .catch(() => {
        if (!cancelled) setDelegationJobs([]);
      });
    return () => {
      cancelled = true;
    };
  }, [activeDelegationRoot]);

  // Persist sticky draft for the active thread.
  useEffect(() => {
    const threadId = session?.threadId;
    if (!threadId || screen !== "chat") return;
    saveDraft(threadId, draft);
  }, [draft, screen, session?.threadId]);

  useEffect(() => {
    const onFocus = () => {
      if (screen === "waiting") {
        void (async () => {
          const attempt = loginAttemptRef.current;
          try {
            const rows = await loadProviders(selectedIdRef.current);
            if (attempt !== loginAttemptRef.current) return;
            const row = rows.find((p) => p.id === selectedIdRef.current);
            if (!row) return;
            const fileAppeared =
              row.statusKind === "ready" ||
              (row.statusKind === "not_logged_in" &&
                row.detail.toLowerCase().includes("incomplete"));
            if (!fileAppeared) return;
            stopPolling();
            await finishVerifiedLogin(row, attempt);
          } catch {
            /* keep waiting */
          }
        })();
        return;
      }
      if (screen === "picker") {
        loadProviders(selectedIdRef.current).catch((error) =>
          ignoreExpectedFailure(error, "refresh providers on window focus")
        );
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [finishVerifiedLogin, loadProviders, screen, stopPolling]);

  useEffect(() => () => stopPolling(), [stopPolling]);

  async function goContinue() {
    const row = providers.find((p) => p.id === selectedId);
    if (!row) return;
    setPickerError(null);
    setContinuing(true);
    try {
      await enterChat(row.id);
    } catch (err) {
      setScreen("picker");
      setPickerError(startupPickerErrorFrom(err));
    } finally {
      setContinuing(false);
    }
  }

  async function goConnect() {
    const row = providers.find((p) => p.id === selectedId);
    if (!row || loginStartingRef.current) return;
    loginStartingRef.current = true;
    setConnecting(true);
    setPickerError(null);
    try {
      const started = await withTimeout(
        backend.startLogin(row.id),
        "sign-in start"
      );
      setWaitingTitle(started.browserTitle);
      setWaitingBody(started.browserBody);
      setScreen("waiting");
      startWaitingPoll();
    } catch (err) {
      setPickerError(startupPickerErrorFrom(err));
    } finally {
      loginStartingRef.current = false;
      setConnecting(false);
    }
  }

  async function cancelWait() {
    loginAttemptRef.current += 1;
    stopPolling();
    await withTimeout(backend.cancelLogin(), "cancel sign-in").catch((error) =>
      ignoreExpectedFailure(error, "cancel sign-in")
    );
    setWaitingError(null);
    if (session) {
      setScreen("chat");
      return;
    }
    setScreen("picker");
    await loadProviders(selectedId).catch((err) => {
      setPickerError(startupPickerErrorFrom(err));
    });
  }

  async function switchProvider(providerId: string) {
    if (!providerId) return;
    if (session?.threadId) {
      saveDraft(session.threadId, draftRef.current);
    }
    setSelectedId(providerId);
    try {
      await enterChat(providerId);
    } catch (err) {
      setPickerError(pickerErrorFrom(err));
      toast.add({
        type: "error",
        title: "Could not switch provider",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  /** `only` targets a specific provider — used by the Reconnect on an auth
   *  failure, which knows exactly which account the gateway rejected. */
  async function reconnectProvider(only?: string) {
    const providerId = only ?? session?.provider ?? selectedId;
    if (!providerId || backend.mode === "fixture" || loginStartingRef.current) return;
    loginStartingRef.current = true;
    setPickerError(null);
    try {
      const started = await withTimeout(
        backend.startLogin(providerId),
        "sign-in start"
      );
      setSelectedId(providerId);
      setWaitingTitle(started.browserTitle);
      setWaitingBody(started.browserBody);
      setScreen("waiting");
      startWaitingPoll();
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not start sign-in",
        description: startupPickerErrorFrom(err).message,
      });
    } finally {
      loginStartingRef.current = false;
    }
  }

  async function onNewChat() {
    try {
      if (session?.threadId) {
        saveDraft(session.threadId, draftRef.current);
      }
      const info = await backend.openProjectChat({
        // The main New chat action creates a free chat. A project's + button
        // remains the explicit way to start one inside that project.
        root: null,
        newThread: true,
        providerId: session?.provider ?? selectedId ?? undefined,
      });
      applySession(info, { clearDraft: true });
    } catch (err) {
      const recovery = conversationRecovery(err);
      if (recovery) {
        setPendingConversationRecovery({ recovery, root: null });
        return;
      }
      toast.add({
        type: "error",
        title: "Could not start new chat",
        description: formatInvokeError(err),
      });
    }
  }

  async function onForkThread() {
    if (sendingRef.current) {
      toast.add({
        type: "warning",
        title: "Finish this turn first",
        description: "Stop the current turn before forking the conversation.",
      });
      return;
    }
    try {
      const info = await backend.forkThread();
      applySession(info, { clearDraft: true });
      toast.add({
        type: "success",
        title: "Fork created",
        description: "You are now in a new conversation with the same history.",
      });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not fork conversation",
        description: formatInvokeError(err),
      });
    }
  }

  const refreshWorkspaceChanges = useCallback(async () => {
    const requestedThreadId = threadIdRef.current;
    const next = await backend.workspaceChanges();
    if (!requestedThreadId || threadIdRef.current !== requestedThreadId) {
      throw new Error("workspace changed while reading Git changes");
    }
    workspaceChangesRef.current.set(requestedThreadId, next);
    setWorkspaceChange(next);
    return next;
  }, []);

  const activeIsFreeChat = session?.isFreeChat ?? true;
  /**
   * Read the branch's changes when a project chat opens.
   *
   * The branch strip is a proactive surface, so it has to know before the user
   * asks. Nothing else established that: the diff panel only reads Git when it
   * is opened, and `workspace_changed` only fires when a turn edits something —
   * so opening a chat on a branch that was already dirty said nothing at all.
   */
  useEffect(() => {
    if (!activeThreadId || activeIsFreeChat) return;
    void refreshWorkspaceChanges().catch((error) => {
      // Best effort: a workspace Git cannot read simply shows no strip.
      ignoreExpectedFailure(error, "refresh workspace changes");
    });
  }, [activeThreadId, activeIsFreeChat, refreshWorkspaceChanges]);


  async function onVerifyWorkspace() {
    try {
      const review = await backend.verifyWorkspace();
      setWorkspaceReview(review);
      toast.add({
        type: review.patchCheck === "issues" ? "warning" : "success",
        title: review.patchCheck === "issues" ? "Review needed" : "Workspace checked",
        description: review.summary,
      });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not check workspace",
        description: formatInvokeError(err),
      });
    }
  }

  async function onRewindThread(checkpointId: string) {
    if (sendingRef.current) {
      toast.add({
        type: "warning",
        title: "Finish this turn first",
        description: "Stop the current turn before rewinding.",
      });
      return;
    }
    try {
      const info = await backend.rewindThread(checkpointId);
      applySession(info, { clearDraft: true });
      toast.add({
        type: "success",
        title: "Conversation rewound",
        description: "Conversation restored. Your files were not changed.",
      });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not rewind conversation",
        description: formatInvokeError(err),
      });
    }
  }

  async function onEditMessage(messageId: string, text: string) {
    if (sendingRef.current || compactionInFlightRef.current) return;
    const editedText = text.trim();
    if (!editedText) return;

    try {
      const info = await backend.editMessage(messageId);
      applySession(info, { clearDraft: true });
      await submitTurn(editedText, [], { restoreDraftOnFailure: true });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not edit message",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onDeleteThread(
    id: string,
    projectPath: string | null,
    freeChat: boolean
  ) {
    try {
      const deletedActive = session?.threadId === id;
      const info = await backend.deleteThread(id, projectPath, freeChat);
      saveDraft(id, "");
      // Only the deleted open chat should replace the transcript. Applying a
      // busy-route snapshot here would wipe the waiting chat's live messages.
      if (deletedActive) {
        applySession(info, { clearDraft: true });
      }
      setWorkspacePath(info.isFreeChat ? null : info.root);
      void backend.gitBranch().then(setBranch).catch(() => setBranch(null));
      toast.add({
        type: "success",
        title: "Chat deleted",
        description: deletedActive
          ? "No new chat saved — type to start one"
          : undefined,
      });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not delete chat",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onOpenProjectChat(options: {
    root: string | null;
    threadId?: string;
    newThread?: boolean;
    providerId?: string;
    copyThread?: boolean;
  }): Promise<boolean> {
    try {
      if (session?.threadId) {
        saveDraft(session.threadId, draftRef.current);
      }
      const info = await backend.openProjectChat(options);
      setPendingConversationRecovery(null);
      applySession(info, { clearDraft: Boolean(options.newThread) });
      // Refresh the picker catalogue so the model list and key status match the
      // project we actually opened instead of the project we just left.
      void loadProviders(info.provider).catch((error) =>
        ignoreExpectedFailure(error, "refresh providers after opening chat")
      );
      setWorkspacePath(info.isFreeChat ? null : info.root);
      void backend.gitBranch().then(setBranch).catch(() => setBranch(null));
      return true;
    } catch (err) {
      const recovery = conversationRecovery(err);
      if (recovery) {
        setPendingConversationRecovery({ recovery, root: options.root });
        return false;
      }
      toast.add({
        type: "error",
        title: "Could not open project chat",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function chooseConversationProvider(providerId: string) {
    const pending = pendingConversationRecovery;
    if (!pending || conversationRecoveryBusy) return;

    setConversationRecoveryBusy(true);
    try {
      const info = await backend.openProjectChat({
        root: pending.root,
        threadId: pending.recovery.threadId ?? undefined,
        newThread: pending.recovery.kind === "new_chat_unavailable",
        providerId,
        copyThread: pending.recovery.kind === "owner_unavailable",
      });
      const provider = pending.recovery.providers.find((item) => item.id === providerId);
      setPendingConversationRecovery(null);
      applySession(info);
      void loadProviders(info.provider).catch((error) =>
        ignoreExpectedFailure(error, "refresh providers after switching chat")
      );
      setWorkspacePath(info.isFreeChat ? null : info.root);
      void backend.gitBranch().then(setBranch).catch(() => setBranch(null));
      toast.add({
        type: "success",
        title:
          pending.recovery.kind === "owner_unavailable"
            ? "Copy opened"
            : pending.recovery.kind === "new_chat_unavailable"
              ? "Chat opened"
              : "Provider saved",
        description:
          pending.recovery.kind === "owner_unavailable"
            ? `Opened a copy with ${provider?.label ?? providerId}. The original chat was kept.`
            : pending.recovery.kind === "new_chat_unavailable"
              ? `Opened a new chat with ${provider?.label ?? providerId}.`
              : `This chat now uses ${provider?.label ?? providerId}.`,
      });
    } catch (err) {
      const recovery = conversationRecovery(err);
      if (recovery) {
        setPendingConversationRecovery({ recovery, root: pending.root });
      } else {
        toast.add({
          type: "error",
          title: "Could not open conversation",
          description: formatInvokeError(err),
        });
      }
    } finally {
      setConversationRecoveryBusy(false);
    }
  }

  async function configureConversationProvider() {
    const pending = pendingConversationRecovery;
    if (!pending) return;
    if (pending.root === null) {
      setPendingConversationRecovery(null);
      setPickerError({
        message: "Choose a provider or add one with an API key to start this chat.",
        workspace: false,
      });
      setScreen("picker");
      void loadProviders(selectedIdRef.current).catch((error) =>
        ignoreExpectedFailure(error, "refresh providers after recovery")
      );
      return;
    }

    if (pending.recovery.kind !== "unknown_owner" && pending.recovery.configured) {
      // The provider is present in the project but no longer ready, which is
      // the API-key-deleted case. Open the provider sheet so the user can
      // replace the key or explicitly choose another provider; project config
      // is only needed when the provider entry itself is missing.
      setPendingConversationRecovery(null);
      setProviderSwitchRequest((request) => request + 1);
      return;
    }

    try {
      await backend.openProjectConfig(pending.root);
      setPendingConversationRecovery(null);
      toast.add({
        type: "success",
        title: "Project configuration opened",
        description:
          pending.recovery.kind === "owner_unavailable"
            ? `Add ${pending.recovery.providerLabel} to this project's zest.toml, then open the chat again.`
            : pending.recovery.kind === "new_chat_unavailable"
              ? `Configure ${pending.recovery.providerLabel} or another provider in this project's zest.toml, then open the chat again.`
              : "Add a provider to this project's zest.toml, then open the chat again.",
      });
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not open project configuration",
        description: formatInvokeError(err),
      });
    }
  }

  function mergeAttachments(files: PreparedAttachment[]) {
    // Rust caps each batch, which is all it can see. The ceiling that matters
    // is across batches — ten single-file picks are ten batches — and the list
    // only exists here.
    const { accepted, rejected } = admitAttachments(attachments, files);
    for (const refusal of rejected) {
      toast.add({
        type: "error",
        title: refusal.name,
        description: refusal.reason,
      });
    }

    setAttachments((prev) => {
      const seen = new Set(prev.map((a) => a.path + a.name + (a.dataBase64?.slice(0, 32) ?? "")));
      const next = accepted.filter(
        (f) => !seen.has(f.path + f.name + (f.dataBase64?.slice(0, 32) ?? ""))
      );
      return [...prev, ...next];
    });
    for (const file of accepted) {
      if (file.status === "error") {
        toast.add({
          type: "error",
          title: file.name,
          description: file.detail,
        });
      }
    }
  }

  async function onAttachFiles() {
    try {
      const files = await backend.pickFiles();
      if (!files.length) return;
      mergeAttachments(files);
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not attach files",
        description: formatInvokeError(err),
      });
    }
  }

  async function onPasteImages(files: File[]) {
    try {
      const prepared: PreparedAttachment[] = [];
      for (const file of files) {
        const dataUrl = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () =>
            resolve(typeof reader.result === "string" ? reader.result : "");
          reader.onerror = () => reject(reader.error ?? new Error("read failed"));
          reader.readAsDataURL(file);
        });
        const base64 = dataUrl.includes(",") ? dataUrl.split(",").pop()! : dataUrl;
        const att = await backend.preparePastedImage({
          dataBase64: base64,
          mediaType: file.type || "image/png",
          name: file.name || undefined,
        });
        prepared.push(att);
      }
      if (prepared.length) mergeAttachments(prepared);
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not paste image",
        description: formatInvokeError(err),
      });
    }
  }

  async function onOpenFolder() {
    try {
      const result = await backend.pickWorkspaceFolder();
      if (!result) return;
      setWorkspacePath(result.path);
      void backend.gitBranch().then(setBranch).catch(() => setBranch(null));
      if (result.sessionEnded || session) {
        const providerId = session?.provider ?? selectedId;
        if (providerId) await loadProviders(providerId);
        setSession(null);
        sessionIdRef.current = null;
        threadIdRef.current = null;
        setMessages([]);
        setAttachments([]);
        if (providerId) {
          try {
            await enterChat(providerId);
          } catch (err) {
            setPickerError(pickerErrorFrom(err));
            setScreen("picker");
          }
        } else {
          setScreen("picker");
        }
      }
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not open folder",
        description: formatInvokeError(err),
      });
    }
  }

  async function onSend(textOverride?: string) {
    const directAnswer = textOverride !== undefined;
    const text = (textOverride ?? draft).trim();
    const pending = directAnswer ? [] : attachmentsRef.current;
    const hasOk = pending.some(
      (a) =>
        a.status === "done" &&
        (Boolean(a.content?.trim()) || (a.kind === "image" && Boolean(a.dataBase64)))
    );
    if ((!text && !hasOk) || compacting || compactionInFlightRef.current) {
      return;
    }
    const queueThreadId = threadIdRef.current;
    const shouldQueue =
      queueThreadId !== null &&
      (sendingRef.current ||
        (threadQueuesRef.current[queueThreadId]?.length ?? 0) > 0);

    if (shouldQueue && queueThreadId) {
      try {
        await backend.sendMessage(
          text,
          pending.map((a) => ({
            name: a.name,
            detail: a.detail,
            content: a.content,
            status: a.status,
            kind: a.kind,
            mediaType: a.mediaType,
            dataBase64: a.dataBase64,
          })),
          "followup"
        );
        if (!directAnswer) {
          setDraft("");
          setAttachments([]);
          saveDraft(queueThreadId, "");
        }
      } catch (error) {
        toast.add({
          type: "error",
          title: "Could not queue message",
          description: formatInvokeError(error),
        });
      }
      return;
    }

    if (!directAnswer) {
      setDraft("");
      setAttachments([]);
      if (session?.threadId) {
        saveDraft(session.threadId, "");
      }
    }
    await submitTurn(text, pending, { restoreDraftOnFailure: !directAnswer });
  }

  /**
   * Send one turn. Split out of `onSend` so a button can start a turn without
   * going through the composer — the composer owns the draft and attachments,
   * and a turn does not have to come from either.
   */
  async function submitTurn(
    text: string,
    pending: PreparedAttachment[],
    { restoreDraftOnFailure }: { restoreDraftOnFailure: boolean }
  ): Promise<{ accepted: boolean; retryable: boolean }> {
    if (compactionInFlightRef.current) {
      return { accepted: false, retryable: false };
    }
    const turnThreadId = threadIdRef.current;
    const chips: UserAttachmentChip[] = pending
      .filter((a) => a.status === "done")
      .map((a) => ({ name: a.name, kind: a.kind }));
    if (turnThreadId && chips.length > 0) {
      pendingUserAttachmentsRef.current.set(turnThreadId, chips);
    }
    // Stay busy until an authoritative done/cancelled/error chat-event arrives.
    setSending(true);
    sendingRef.current = true;
    activeAssistantId.current = null;
    if (turnThreadId) {
      const current = chatStatesRef.current.get(turnThreadId);
      if (current) {
        chatStatesRef.current.set(turnThreadId, {
          ...current,
          activeAssistantId: null,
          sending: true,
        });
      }
    }
    try {
      await backend.sendMessage(
        text,
        pending.map((a) => ({
          name: a.name,
          detail: a.detail,
          content: a.content,
          status: a.status,
          kind: a.kind,
          mediaType: a.mediaType,
          dataBase64: a.dataBase64,
        }))
      );
      return { accepted: true, retryable: false };
    } catch (err) {
      if (turnThreadId) {
        pendingUserAttachmentsRef.current.delete(turnThreadId);
      }
      if (turnThreadId === threadIdRef.current) {
        setSending(false);
        sendingRef.current = false;
        const current = turnThreadId
          ? chatStatesRef.current.get(turnThreadId)
          : undefined;
        if (current && turnThreadId) {
          chatStatesRef.current.set(turnThreadId, {
            ...current,
            sending: false,
          });
        }
      }
      // Only text the user typed goes back in the composer. Putting a
      // button's prompt there would leave them holding words they never wrote.
      if (restoreDraftOnFailure) {
        if (turnThreadId === threadIdRef.current) {
          setDraft(text);
          setAttachments(pending);
        }
        if (turnThreadId) {
          saveDraft(turnThreadId, text);
        }
      }
      const message = formatInvokeError(err);
      const retryable =
        message.toLowerCase().includes("busy") ||
        message.includes("already in progress");
      if (!message.includes("already in progress") && !message.includes('"busy"')) {
        toast.add({
          type: "error",
          title: "Could not send",
          description: message,
        });
      } else {
        toast.add({
          type: "error",
          title: "Busy",
          description: message,
        });
      }
      return { accepted: false, retryable };
    }
  }

  /**
   * Leave Plan mode and tell the model to build what it just planned.
   *
   * Delegation happens here rather than during planning: there is nothing to
   * hand a worker until the plan exists, and a worker sees none of this
   * conversation. The plan already names which steps suit another worker, so
   * the prompt only has to say "use it where you marked it".
   */
  async function onBuildPlan() {
    if (sendingRef.current) return;

    if (approvalModeState === "plan") {
      // Restore rather than escalate. Auto is the fallback only because it is
      // the mode the desktop opens in, so it is the one the user has already
      // consented to by default.
      const target = modeBeforePlanRef.current ?? "auto";
      modeBeforePlanRef.current = null;
      try {
        await backend.setApprovalMode(target);
        setApprovalModeState(target);
      } catch (err) {
        // Building under Plan mode would fail every write, so stop here and
        // leave the user in a mode they can see.
        toast.add({
          type: "error",
          title: "Could not leave Plan mode",
          description: formatInvokeError(err),
        });
        return;
      }
    }

    await submitTurn(BUILD_PLAN_PROMPT, [], { restoreDraftOnFailure: false });
  }

  async function onStop() {
    if (!sendingRef.current) return;
    try {
      await backend.cancelTurn(threadIdRef.current ?? undefined);
      // Keep sending=true until the Cancelled chat-event clears busy state.
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not stop",
        description: formatInvokeError(err),
      });
    }
  }

  async function onResolveApproval(
    approvalId: string,
    decision: ApprovalChoice
  ) {
    const resolving = resolvingApprovalIdsRef.current;
    if (resolving.has(approvalId)) return;

    const allow = decision !== "deny";
    const snapshot = findApprovalTool(messagesRef.current, approvalId);
    if (!snapshot || snapshot.status !== "awaiting_approval") return;
    resolving.add(approvalId);
    const threadId = threadIdRef.current;
    if (allow) {
      const next = markApprovalRunning(messagesRef.current, approvalId);
      messagesRef.current = next;
      setMessages(next);
    }
    try {
      await backend.resolveApproval(
        approvalId,
        decision,
        threadId ?? undefined
      );
    } catch (err) {
      // A dropped waiter is permanent: putting the card back would leave a
      // button that can only fail, and because the approval queue is ordered by
      // transcript position it would keep shadowing the live card behind it.
      // Retire it instead so the queue advances.
      if (isDroppedApprovalError(rawInvokeError(err).toLowerCase())) {
        const retired = retireApprovalCard(messagesRef.current, approvalId, snapshot.id);
        messagesRef.current = retired;
        setMessages(retired);
        return;
      } else if (allow && snapshot) {
        const restored = restoreApprovalCard(messagesRef.current, snapshot);
        messagesRef.current = restored;
        setMessages(restored);
      }
      toast.add({
        type: "error",
        title: allow ? "Could not allow tool" : "Could not deny tool",
        description: formatInvokeError(err),
      });
      throw err;
    } finally {
      resolving.delete(approvalId);
    }
  }

  async function onResolveQuestion(questionId: string, answer: string) {
    const resolving = resolvingQuestionIdsRef.current;
    if (resolving.has(questionId)) return;
    const isPending = messagesRef.current.some(
      (message) =>
        message.role === "assistant" && message.question?.questionId === questionId
    );
    if (!isPending) return;
    resolving.add(questionId);
    const threadId = threadIdRef.current;
    try {
      await backend.resolveQuestion(questionId, answer, threadId ?? undefined);
    } catch (err) {
      if (isDroppedApprovalError(rawInvokeError(err).toLowerCase())) return;
      toast.add({
        type: "error",
        title: "Could not send answer",
        description: formatInvokeError(err),
      });
      throw err;
    } finally {
      resolving.delete(questionId);
    }
  }

  async function onApprovalModeChange(next: ApprovalMode) {
    const previous = approvalModeState;
    // Remember what planning interrupted, so Build can put it back rather than
    // picking a permission level on the user's behalf.
    if (next === "plan" && previous !== "plan") {
      modeBeforePlanRef.current = previous;
    }
    setApprovalModeState(next);
    try {
      await backend.setApprovalMode(next);
    } catch (err) {
      // Rust is authoritative — put the picker back if it refused.
      setApprovalModeState(previous);
      toast.add({
        type: "error",
        title: "Could not change mode",
        description: formatInvokeError(err),
      });
    }
  }

  async function commitSessionOptions(
    next: { model?: string; effort?: EffortId },
    request: () => Promise<SessionMeta>,
    errorTitle: string
  ) {
    if (optionsUpdatingRef.current) return;
    if (screen !== "chat") {
      if (next.model) setModel(next.model);
      if (next.effort) setEffort(next.effort);
      return;
    }
    const snapshot = { model: modelRef.current, effort: effortRef.current };
    const optimisticModel = next.model ?? snapshot.model;
    const optimisticEffort = next.effort ?? snapshot.effort;
    optionsUpdatingRef.current = true;
    setOptionsUpdating(true);
    setModel(optimisticModel);
    setEffort(optimisticEffort);
    setSession((prev) =>
      prev
        ? { ...prev, model: optimisticModel, effort: optimisticEffort }
        : prev
    );
    try {
      const info = await request();
      setSession((prev) => mergeSessionOptions(prev, info));
      setModel(info.model);
      setEffort(effortFromSession(info.effort, optimisticEffort));
    } catch (err) {
      setSession((prev) => rollbackSessionOptions(prev, snapshot));
      setModel(snapshot.model);
      setEffort(snapshot.effort);
      toast.add({
        type: "error",
        title: errorTitle,
        description: formatInvokeError(err),
      });
    } finally {
      optionsUpdatingRef.current = false;
      setOptionsUpdating(false);
    }
  }

  async function onModelChange(next: string) {
    if (typeof next !== "string" || !next.trim()) return;
    await commitSessionOptions(
      { model: next },
      () => backend.updateSessionOptions({ model: next }),
      "Could not update model"
    );
  }

  async function onEffortChange(next: EffortId) {
    await commitSessionOptions(
      { effort: next },
      () => backend.updateSessionOptions({ effort: next }),
      "Could not update effort"
    );
  }

  async function onResetOptions() {
    const resetModel = session?.defaultModel ?? DEFAULT_CODEX_MODEL;
    await commitSessionOptions(
      { model: resetModel, effort: DEFAULT_EFFORT },
      () => backend.resetSessionOptions(),
      "Could not reset model and effort"
    );
  }

  async function onApproveDelegation(jobId: string) {
    try {
      const job = await backend.approveDelegationJob(jobId);
      replaceDelegationJob(job);
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not start feature card",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onCreateDelegation(request: DelegationCreateInput) {
    try {
      const job = await backend.createDelegationJob(request);
      replaceDelegationJob(job);
      return job;
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not create feature card",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onCancelDelegation(jobId: string) {
    try {
      const job = await backend.cancelDelegationJob(jobId);
      replaceDelegationJob(job);
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not cancel feature card",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onRetryDelegation(jobId: string) {
    try {
      const job = await backend.retryDelegationJob(jobId);
      replaceDelegationJob(job);
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not retry feature card",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  async function onApplyDelegation(jobId: string) {
    try {
      const job = await backend.applyDelegationJob(jobId);
      replaceDelegationJob(job);
      if (job.status === "accepted") {
        toast.add({
          type: "success",
          title: "Accepted changes applied",
          description: `${job.title} is now in the active workspace.`,
        });
      } else if (job.status === "apply_conflict") {
        toast.add({
          type: "warning",
          title: "Apply conflict",
          description: "The workspace changed. Review the conflicting card before retrying.",
        });
      }
    } catch (err) {
      toast.add({
        type: "error",
        title: "Could not apply accepted changes",
        description: formatInvokeError(err),
      });
      throw err;
    }
  }

  const authMode = screen !== "chat";

  return (
    <Toaster>
      <div
        className={cn(
          "h-full min-h-0 w-full min-w-0",
          authMode &&
            cn(
              "relative flex overflow-auto px-6 py-8 before:pointer-events-none before:absolute before:inset-0 before:z-0 before:bg-[radial-gradient(ellipse_at_50%_0%,color-mix(in_srgb,var(--primary)_10%,transparent),transparent_55%)] [&>*]:relative [&>*]:z-10",
              "items-center justify-center"
            ),
          !authMode && "flex flex-1 flex-col overflow-hidden"
        )}
      >
        {/* Boot is short now that session start no longer waits on the network. */}
        {screen === "boot" ? <ChatSkeleton /> : null}

        {screen === "picker" ? (
          <ProviderPicker
            providers={providers}
            selectedId={selectedId}
            workspacePath={workspacePath}
            error={pickerError}
            onSelect={setSelectedId}
            onContinue={goContinue}
            onConnect={goConnect}
            onOpenFolder={onOpenFolder}
            onRefresh={async () => {
              try {
                await loadProviders(selectedIdRef.current);
                setPickerError(null);
              } catch (err) {
                setPickerError(startupPickerErrorFrom(err));
              }
            }}
            continuing={continuing}
            connecting={connecting}
          />
        ) : null}

        {screen === "waiting" ? (
          <WaitingScreen
            title={waitingTitle}
            body={waitingBody}
            hint={waitingHint}
            error={waitingError}
            onCancel={cancelWait}
          />
        ) : null}

        {screen === "auth-success" ? (
          <AuthSuccess onContinue={goContinue} continuing={continuing} />
        ) : null}

        {screen === "chat" && session ? (
          <ChatScreen
            session={session}
            messages={messages}
            draft={draft}
            attachments={attachments}
            branch={branch}
            gitContext={gitContext}
            profile={profile}
            sending={sending}
            queuedMessages={threadQueues[session.threadId] ?? []}
            onUpdateQueuedMessage={(turnId, text) =>
              updateQueuedTurn(session.threadId, turnId, text)
            }
            onRemoveQueuedMessage={(turnId) =>
              discardQueuedTurn(session.threadId, turnId)
            }
            onResumeQueuedMessages={() => resumeQueuedMessages(session.threadId)}
            resumingQueuedMessages={resumingQueuedThread === session.threadId}
            threadActivity={threadActivity}
            model={model}
            effort={effort}
            optionsDisabled={optionsUpdating}
            onDraftChange={setDraft}
            onSend={onSend}
            onEditMessage={onEditMessage}
            onStop={onStop}
            onNewChat={onNewChat}
            onForkThread={onForkThread}
            onRewindThread={onRewindThread}
            workspaceReview={workspaceReview}
            workspaceChange={workspaceChange}
            onRefreshWorkspaceChanges={refreshWorkspaceChanges}
            onVerifyWorkspace={onVerifyWorkspace}
            compacting={compacting}
            onDeleteThread={onDeleteThread}
            onOpenProjectChat={onOpenProjectChat}
            providers={providers}
            onSwitchProvider={switchProvider}
            onRefreshProviders={() =>
              loadProviders(session?.provider ?? selectedIdRef.current).then(() => undefined)
            }
            onReconnect={reconnectProvider}
            onAttachFiles={onAttachFiles}
            onOpenFolder={onOpenFolder}
            onRemoveAttachment={(id) =>
              setAttachments((prev) => prev.filter((a) => a.id !== id))
            }
            onPasteImages={onPasteImages}
            onProfileChange={setProfile}
            onModelChange={onModelChange}
            onEffortChange={onEffortChange}
            onResetOptions={onResetOptions}
            onResolveApproval={onResolveApproval}
            onResolveQuestion={onResolveQuestion}
            onReconnectProvider={(providerId) => {
              // Same path as the picker Connect: spawns the vendor/gateway
              // login and shows the waiting screen until it resolves.
              void reconnectProvider(providerId);
            }}
            onReloadSession={async () => {
              // Rebuilds the runtime so a replaced provider key or ACP worker
              // change takes effect. The sticky thread is reloaded, so the
              // open chat survives.
              const id = session?.provider ?? selectedIdRef.current;
              if (!id) return;
              try {
                await enterChat(id);
              } catch (err) {
                setPickerError(pickerErrorFrom(err));
                setScreen("picker");
              }
            }}
            approvalMode={approvalModeState}
            onApprovalModeChange={onApprovalModeChange}
            onBuildPlan={() => void onBuildPlan()}
            onOpenProfile={() => navigateTo({ kind: "profile" })}
            onOpenUsage={() => navigateTo({ kind: "usage" })}
            onOpenCustomize={() =>
              navigateTo({
                kind: "customize",
                tab: shellPanel?.kind === "customize" ? shellPanel.tab : "mcp",
              })
            }
            shellPanel={shellPanel}
            onCustomizeTabChange={(tab) => navigateTo({ kind: "customize", tab })}
            onClosePanel={closeShellPanel}
            onOpenSettings={() => navigateTo({ kind: "settings", focusUser: false })}
            onCloseSettings={closeSettingsFromHistory}
            canNavigateBack={navigation.back.length > 0}
            canNavigateForward={navigation.forward.length > 0}
            onNavigateBack={navigateBack}
            onNavigateForward={navigateForward}
            providerSwitchRequest={providerSwitchRequest}
            settingsOpenRequest={settingsOpenRequest}
            delegationJobs={delegationJobs}
            onCreateDelegation={onCreateDelegation}
            onApproveDelegation={onApproveDelegation}
            onCancelDelegation={onCancelDelegation}
            onRetryDelegation={onRetryDelegation}
            onApplyDelegation={onApplyDelegation}
            settingsRequest={settingsRequest}
            sessionWarning={sessionWarning}
            onDismissWarning={() => setSessionWarning(null)}
          />
        ) : null}

      </div>

      <ConversationRecoveryDialog
        recovery={pendingConversationRecovery?.recovery ?? null}
        busy={conversationRecoveryBusy}
        onClose={() => {
          if (!conversationRecoveryBusy) setPendingConversationRecovery(null);
        }}
        onConfigure={() => void configureConversationProvider()}
        onChooseProvider={(providerId) => void chooseConversationProvider(providerId)}
      />
    </Toaster>
  );
}
