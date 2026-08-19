import type { ChatEvent as GeneratedChatEvent } from "./generated/ChatEvent.ts";
import type { ModelCapability } from "./generated/ModelCapability.ts";
import type { CommandView } from "./generated/CommandView.ts";
import type { ExternalAgentCheckView } from "./generated/ExternalAgentCheckView.ts";
import type { ExternalAgentView } from "./generated/ExternalAgentView.ts";
import type { ProviderView as GeneratedProviderView } from "./generated/ProviderView.ts";
import type { SessionInfo as GeneratedSessionInfo } from "./generated/SessionInfo.ts";
import type { SessionMeta as GeneratedSessionMeta } from "./generated/SessionMeta.ts";
import type { ThreadCheckpointView } from "./generated/ThreadCheckpoint.ts";
import type { TurnRecoveryView } from "./generated/TurnRecovery.ts";
import type { ToolMetaView } from "./generated/ToolMetaView.ts";
import type { GitContextView as GeneratedGitContext } from "./generated/GitContext.ts";
import type { PullRequestView } from "./generated/PullRequestView.ts";
import type { WorkspaceReview as GeneratedWorkspaceReview } from "./generated/WorkspaceReview.ts";
import type { WorkspaceChange as GeneratedWorkspaceChange } from "./generated/WorkspaceChange.ts";
import type { DelegationEvent as GeneratedDelegationEvent } from "./generated/DelegationEvent.ts";
import type { DelegationJobView } from "./generated/DelegationJobView.ts";
import type { PlanningQuestion } from "./planningQuestion.ts";

export type StatusKind = "ready" | "unknown" | "not_logged_in" | "unconfigured";

/** Mirrors `ApprovalMode` in core; wire names must match `ApprovalMode::as_str`. */
export type ApprovalMode =
  | "manual"
  | "accept_edits"
  | "plan"
  | "auto"
  | "bypass";

export type { CommandView };

/** What the user clicked on an approval card. */
export type ApprovalChoice = "once" | "session" | "deny";

export const APPROVAL_MODES: {
  id: ApprovalMode;
  label: string;
  hint: string;
}[] = [
  { id: "manual", label: "Manual", hint: "Ask before every write and command" },
  {
    id: "accept_edits",
    label: "Accept edits",
    hint: "Apply file edits; still ask for commands",
  },
  // The hint says what the mode produces, not just what it forbids: it runs the
  // `plan` skill, so "read only" alone would undersell it and leave people
  // typing `/plan` inside plan mode.
  {
    id: "plan",
    label: "Plan",
    hint: "Research and write a plan — no writes, no commands",
  },
  {
    id: "auto",
    label: "Auto",
    hint: "Apply edits and safe commands; ask for the rest",
  },
  {
    id: "bypass",
    label: "Bypass permissions",
    hint: "Never ask. Use in a throwaway tree",
  },
];

/** Rust-authoritative provider row (auth + catalogue). */
export type ProviderRow = Omit<GeneratedProviderView, "statusKind"> & {
  statusKind: StatusKind;
};

export type ExternalAgentRow = ExternalAgentView;
export type ExternalAgentCheck = ExternalAgentCheckView;

export type LoginStarted = {
  browserTitle: string;
  browserBody: string;
};

export type LoginStatus = {
  state: "idle" | "running" | "exited";
  detail: string | null;
};

export type { ModelCapability };
export type WorkspaceReview = GeneratedWorkspaceReview;
export type WorkspaceChange = GeneratedWorkspaceChange;
export type DelegationEvent = GeneratedDelegationEvent;
export type DelegationJob = DelegationJobView;

export type ToolMetadata = ToolMetaView;

export type ToolPart = {
  id: string;
  name: string;
  status: "running" | "awaiting_approval" | "done" | "error";
  summary?: string;
  approvalId?: string;
  path?: string;
  diff?: string;
  metadata?: ToolMetadata;
};

/** Filename chips shown on a sent user bubble (UI-only; may be absent on reload). */
export type UserAttachmentChip = {
  name: string;
  kind: string;
};

export type ProviderActivityPart = {
  id: string;
  title: string;
  status: "running" | "done" | "error";
};

export type ChatMessage =
  | {
      id: string;
      role: "user";
      text: string;
      attachments?: UserAttachmentChip[];
    }
  | {
      id: string;
      role: "assistant";
      text: string;
      thinking: string;
      tools: ToolPart[];
      error?: string;
      /** Provider to offer a Reconnect for; only set on auth failures. */
      reconnectProvider?: string;
      /** Slash command that produced this turn, if any — titles the output. */
      command?: string;
      /** Live question requested by the model; not persisted in thread history. */
      question?: PlanningQuestion;
      /** Ephemeral activity from a provider-owned model/tool loop. */
      providerActivity?: ProviderActivityPart[];
      streaming: boolean;
    };

/** Wire shape from Rust `StoredMessage` (role-tagged). */
export type StoredMessage = ChatMessage;

/**
 * Wire chat events from Rust (`ChatEvent` in zest-desktop).
 * Regenerate: see `./generated/README.md`.
 */
export type ChatEvent = GeneratedChatEvent;

/** Session snapshot from Rust; messages are the UI ChatMessage projection. */
export type SessionInfo = Omit<GeneratedSessionInfo, "messages"> & {
  messages: ChatMessage[];
};

/**
 * A session without its transcript.
 *
 * What operations that do not touch the conversation reply with — changing a
 * model or effort level, for instance. Structurally a subset of SessionInfo,
 * so spreading one over the other is well defined.
 */
export type SessionMeta = GeneratedSessionMeta;

export type ThreadCheckpoint = ThreadCheckpointView;
export type TurnRecovery = TurnRecoveryView;
export type PullRequestLink = PullRequestView & {
  repository?: string;
};
export type GitContext = GeneratedGitContext;

/** Durable checkout/PR association stored on a chat summary. */
export type ThreadGitContext = {
  baseBranch?: string;
  branch?: string;
  startCommit?: string;
  pullRequest?: PullRequestLink;
};

export type ThreadSummary = {
  id: string;
  createdAt: number;
  updatedAt: number;
  title?: string;
  pinned: boolean;
  providerId?: string;
  messageCount: number;
  gitContext?: ThreadGitContext;
};

/** Sidebar grouping: one project folder + its chats, or the free-chat bucket. */
export type ProjectChats = {
  name: string;
  /** `null` marks the user-local free-chat bucket shown under RECENT. */
  path: string | null;
  active: boolean;
  threads: ThreadSummary[];
};

export type PreparedAttachment = {
  id: string;
  name: string;
  path: string;
  kind: string;
  status: string;
  detail: string;
  content?: string | null;
  mediaType?: string | null;
  dataBase64?: string | null;
};

export type PluginView = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  available: boolean;
  detail: string;
};

export type NowPlayingView = {
  status: "disabled" | "unavailable" | "idle" | "playing" | "paused" | "stopped";
  title?: string | null;
  artist?: string | null;
  album?: string | null;
  artworkDataUrl?: string | null;
  sourceApp?: string | null;
  positionSecs?: number | null;
  durationSecs?: number | null;
  volumePercent?: number | null;
  canPrevious?: boolean | null;
  canToggle?: boolean | null;
  canNext?: boolean | null;
  detail: string;
  observedAt: number;
};

export type WorkspaceFileView = {
  path: string;
  name: string;
  kind: "file" | "directory";
  size?: number | null;
  modifiedAt?: number | null;
};

export type WorkspaceFileContent = {
  path: string;
  content: string;
  truncated: boolean;
  byteCount: number;
};

export type AttachmentInput = {
  name: string;
  detail: string;
  content?: string | null;
  status: string;
  kind?: string | null;
  mediaType?: string | null;
  dataBase64?: string | null;
};

export type ContextUsage = {
  usedTokens: number;
  windowTokens: number;
  remainingTokens: number;
  percentFull: number;
  source: string;
  systemTokens: number;
  conversationTokens: number;
  /** Fresh input on the last measured turn. Zero when `source` is `estimate`. */
  inputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  messageCount: number;
  checkpointCount: number;
  canCompact: boolean;
  autoCompactThresholdPercent: number;
  shouldAutoCompact: boolean;
};

export type CompactionResult = {
  usage: ContextUsage;
  /** True when trimming long tool results was enough and no summary was written. */
  prunedOnly: boolean;
  resultsPruned: number;
};

export type UserProfile = {
  displayName: string;
  avatarDataUrl: string;
};

export type WorkspacePickResult = {
  path: string;
  sessionEnded: boolean;
};

/** Identity fields present on most chat-event variants. */
export type EventIdentity = {
  session_id: string;
  thread_id: string;
  turn_id: string;
};

export type MeasuredUsage = {
  label: string;
  requests: number;
  inputTokens: number;
  outputTokens: number;
  cacheWriteTokens: number;
  cacheReadTokens: number;
  totalTokens: number;
};

export type HeadroomView =
  | {
      kind: "provider_reported";
      label: string;
      ageSecs?: number | null;
      requestsLimit?: number | null;
      requestsRemaining?: number | null;
      requestsReset?: string | null;
      tokensLimit?: number | null;
      tokensRemaining?: number | null;
      inputTokensRemaining?: number | null;
      outputTokensRemaining?: number | null;
      tokensReset?: string | null;
      retryAfterSecs?: number | null;
      quotaWindow?: string | null;
      quotaStatus?: string | null;
      quotaUsedPercent?: number | null;
      quotaResetAt?: number | null;
      quotaOverageStatus?: string | null;
      quotaOverageResetAt?: number | null;
      quotaIsUsingOverage?: boolean | null;
    }
  | { kind: "not_reported"; label: string };

export type ProviderUsageView = {
  providerId: string;
  measured: MeasuredUsage;
  headroom: HeadroomView;
};

export type ProviderQuotaView = {
  providerId: string;
  kind: "balance" | "rate_limit" | "unavailable" | "error";
  detail: string;
  available?: boolean | null;
  balances: Array<{
    currency: string;
    totalBalance: string;
    grantedBalance: string;
    toppedUpBalance: string;
  }>;
  windows: Array<{
    label: string;
    usedPercent: number;
    windowMinutes?: number | null;
    resetsAt?: number | null;
  }>;
  plan?: string | null;
  spendLimit?: {
    used: string;
    limit: string;
    remainingPercent: number;
    resetsAt?: number | null;
  } | null;
};

export type ProviderQuotaSnapshot = {
  checkedAt: number;
  providers: ProviderQuotaView[];
};

export type UsageSnapshot = {
  providers: ProviderUsageView[];
  externalWorkers: ExternalWorkerUsageView[];
};

export type RangeTotals = {
  /** Known-cost traffic; read it next to `CostQuality`, never alone. */
  costUsd: number;
  requests: number;
  processedTokens: number;
  uncachedInputTokens: number;
  cachedInputTokens: number;
  cacheWriteTokens: number;
  outputTokens: number;
  cacheSavingsUsd: number;
  activeDays: number;
  tokensPerActiveDay: number;
  /**
   * Where every prompt token went, as three shares adding up to 100%. A hit
   * rate alone counts cache writes as failures, which makes a session that is
   * busy filling its cache look like one whose cache never worked.
   */
  servedFromCachePercent: number;
  writtenToCachePercent: number;
  readFreshPercent: number;
  /**
   * Cache reads per cache write — the number the pricing turns on. Absent
   * rather than zero when nothing was ever written.
   */
  cacheReuseRatio?: number | null;
  /** Same value as `servedFromCachePercent`, kept under its original name. */
  cacheHitPercent: number;
  /** Metered before per-model attribution existed, so unpriceable. */
  unattributedTokens: number;
};

export type ProviderDayPoint = {
  providerId: string;
  costUsd: number;
  tokens: number;
};

export type DayCostPoint = {
  date: string;
  costUsd: number;
  tokens: number;
  requests: number;
  byProvider: ProviderDayPoint[];
};

export type ProviderCostRow = {
  providerId: string;
  costUsd: number;
  tokens: number;
  sharePercent: number;
};

/**
 * Where a cost figure came from, in descending order of authority.
 *
 * `providerReported` is what a CLI recorded being charged; `modelPriced` is
 * multiplied out of a rate table; `mixed` combines those sources or includes
 * an unpriced portion.
 */
export type CostSource = "providerReported" | "modelPriced" | "mixed" | "unpriced";

export type ModelCostRow = {
  providerId: string;
  modelId: string;
  /** `null` when nothing could price it. Not zero — the cost is unknown. */
  costUsd?: number | null;
  costSource: CostSource;
  sharePercent: number;
  requests: number;
  tokens: number;
  inputTokens: number;
  outputTokens: number;
  cacheWriteTokens: number;
  cacheReadTokens: number;
};

export type CostQuality = {
  /** Share of tokens whose cost a CLI recorded. Measured, not estimated. */
  providerReportedPercent: number;
  pricedPercent: number;
  unpricedPercent: number;
  unattributedPercent: number;
  unpricedModels: string[];
  cacheSavingsUsd: number;
  savingsMultiple?: number | null;
};

export type UsageReport = {
  days: number;
  startDate: string;
  endDate: string;
  totals: RangeTotals;
  /** One entry per day in the window, oldest first, including quiet days. */
  series: DayCostPoint[];
  providers: ProviderCostRow[];
  models: ModelCostRow[];
  quality: CostQuality;
  externalWorkers: ExternalWorkerUsageView[];
  pricesPath?: string | null;
  rates: RatesStatus;
  scan: ScanStatus;
};

/** How the CLI-transcript scan went. All zeroes means no scan ran. */
export type ScanStatus = {
  filesScanned: number;
  filesCached: number;
  filesSkipped: number;
  filesFailed: number;
  records: number;
  duplicatesDropped: number;
  roots: { providerId: string; path: string; exists: boolean }[];
};

/** Where the rates behind a report came from, and how old they are. */
export type RatesStatus = {
  /** Models in the published catalogue. Zero means it has never been fetched. */
  catalogModels: number;
  /** Hand-set rates, which outrank the catalogue. */
  overrides: number;
  /** Unix seconds of the last successful fetch. */
  fetchedAt?: number | null;
  /** A refresh is due. Not an error — stale rates still price. */
  stale: boolean;
  sourceUrl: string;
};

export type ExternalCost = {
  amount: string;
  currency: string;
};

export type ExternalWorkerUsageView = {
  workerId: string;
  invocations: number;
  usageReports: number;
  tokenReports: number;
  inputTokens?: number | null;
  outputTokens?: number | null;
  thoughtTokens?: number | null;
  cachedReadTokens?: number | null;
  cachedWriteTokens?: number | null;
  reportedTokenTotal?: number | null;
  contextUsed?: number | null;
  contextSize?: number | null;
  lastCost?: ExternalCost | null;
  lastSeen: number;
};

/**
 * One day of the activity heatmap.
 *
 * `tokens` is optional on purpose: a day before token metering existed has real
 * chat counts and no spend figure, which is not the same as a metered day that
 * spent nothing. The heatmap draws those two differently.
 */
export type DayPoint = {
  date: string;
  chats: number;
  messages: number;
  tokens?: number;
  requests?: number;
};

export type ProfileStats = {
  totalChats: number;
  totalMessages: number;
  /** Lifetime, from per-provider totals that predate daily buckets. */
  totalTokens: number;
  totalRequests: number;
  peakDayTokens: number;
  longestChatSecs: number;
  currentStreakDays: number;
  longestStreakDays: number;
  firstActivity?: number;
  days: DayPoint[];
  /** ISO date metering began; earlier cells have no token figure. */
  meteringSince?: string;
};

/**
 * A provider problem found *after* the chat was already usable.
 *
 * Since opening a chat no longer waits on a live turn, verification happens in
 * the background — and a failure has to be reported without throwing the user
 * out of a session that is otherwise working.
 */
export type SessionWarning = {
  providerId: string;
  message: string;
  /** Whether signing in again is the actual fix. */
  offerReconnect: boolean;
};
