import {
  ArrowLeftIcon,
  CheckCircle2Icon,
  BotIcon,
  ChevronRightIcon,
  Clock3Icon,
  FileTextIcon,
  FolderIcon,
  GitMergeIcon,
  HistoryIcon,
  ListTreeIcon,
  PlusIcon,
  PanelRightCloseIcon,
  PlayIcon,
  RefreshCwIcon,
  SendIcon,
  TriangleAlertIcon,
  WrenchIcon,
  XIcon,
  XCircleIcon,
} from "lucide-react";
import { Blobatar } from "blobatar/react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { OrchestrationStatus } from "@/components/OrchestrationStatus";
import { getBackend } from "@/lib/backend";
import { cn } from "@/lib/utils";
import type {
  ChatMessage,
  DelegationCreateInput,
  DelegationJob,
  DelegationTarget,
  DelegationTargetOptionView,
  SessionInfo,
  WorkspaceFileContent,
  WorkspaceFileView,
  WorkspaceReview,
} from "@/lib/types";

type Props = PanelProps & { open: boolean };

type PanelProps = {
  session: SessionInfo;
  messages: ChatMessage[];
  sending: boolean;
  compacting: boolean;
  review: WorkspaceReview | null;
  onClose: () => void;
  onVerify: () => Promise<void>;
  onRewind: (checkpointId: string) => Promise<void>;
  onJump: (messageId: string) => void;
  delegationJobs: DelegationJob[];
  onCreateDelegation: (request: DelegationCreateInput) => Promise<DelegationJob>;
  onApproveDelegation: (jobId: string) => Promise<void>;
  onCancelDelegation: (jobId: string) => Promise<void>;
  onRetryDelegation: (jobId: string) => Promise<void>;
  onApplyDelegation: (jobId: string) => Promise<void>;
  onReconnectProvider?: (providerId: string) => void;
};

type Tab = "activity" | "outline" | "delegation" | "files";

function formatAge(epochSecs: number) {
  const delta = Math.max(0, Math.floor(Date.now() / 1000) - epochSecs);
  if (delta < 60) return "just now";
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
  return `${Math.floor(delta / 86400)}d ago`;
}

function statusIcon(status: string) {
  if (status === "done") return <CheckCircle2Icon className="text-primary" />;
  if (status === "error") return <XCircleIcon className="text-destructive" />;
  if (status === "awaiting_approval") {
    return <TriangleAlertIcon className="text-amber-400" />;
  }
  return <RefreshCwIcon className="animate-spin text-primary" />;
}

/**
 * Blobatars render backdrop-less, on a pinned light tone.
 *
 * The default backdrop is a near-white `#f4f4ed` tile, which is a white patch
 * in a dark row — the app is dark-only (`--card: #141516`, no `.dark` block,
 * `color-scheme: dark` in the document), so there is no light mode this pays
 * off in. Dropping it is not free: the library guarantees contrast *against
 * that backdrop*, and on the seeds this app actually uses, hashed tone put
 * three of twenty heads at ~1.6:1 against the card — invisible.
 *
 * Pinning the tone is what makes transparency safe. Tone is the only trait that
 * moves head lightness, so fixing it at the light end floors head-vs-card at
 * 12.5:1 across 500 seeds while hue stays seed-driven, which is the part that
 * tells two subagents apart. Eyes are enforced against the head, not the
 * backdrop, so they stay legible either way (worst measured 13.2:1).
 *
 * Overriding `palette.bg` instead does not work: an overridden color bypasses
 * the contrast pass rather than being enforced against, so the heads come back
 * tuned for the light backdrop they no longer sit on.
 */
const BLOBATAR_TONE = 0.2;

/**
 * Status as a dot on the corner of a subagent's blobatar.
 *
 * The blobatar occupies the slot the status icon used to have, and it earns it:
 * it identifies *which* subagent at a glance, which a shared spinner never
 * could. Status still has to show, but it is one bit of colour rather than a
 * second icon competing with the avatar.
 */
function statusDotClass(status: string) {
  if (status === "done") return "bg-primary";
  if (status === "error") return "bg-destructive";
  if (status === "awaiting_approval") return "bg-amber-400";
  return "animate-pulse bg-primary";
}

function subagentLabel(id: string) {
  if (id === "claude") return "Claude Code";
  if (id === "gemini") return "Gemini CLI";
  return id
    .replace(/[-_]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function subagentStatus(status: string) {
  if (status === "running") return "Working";
  if (status === "awaiting_approval") return "Needs approval";
  if (status === "error") return "Failed";
  return "Done";
}

function messagePreview(message: ChatMessage) {
  const text = message.text.trim().replace(/\s+/g, " ");
  if (text) return text.slice(0, 96) + (text.length > 96 ? "…" : "");
  if (message.role === "assistant" && message.tools.length) {
    return `${message.tools.length} tool ${message.tools.length === 1 ? "call" : "calls"}`;
  }
  return message.role === "user" ? "Attachment" : "Working…";
}

function targetKey(target: DelegationTarget): string {
  return target.kind === "provider" ? `provider:${target.providerId}` : `external:${target.agentId}`;
}

function targetProviderId(target: DelegationTarget | undefined): string | null {
  return target?.kind === "provider" ? target.providerId : null;
}

function isTargetAvailabilityError(error: string | undefined): boolean {
  if (!error) return false;
  const normalized = error.toLowerCase();
  return normalized.includes("unavailable")
    || normalized.includes("not configured")
    || normalized.includes("credential")
    || normalized.includes("connect")
    || normalized.includes("reconnect");
}

function lines(value: string): string[] {
  return value.split("\n").map((line) => line.trim()).filter(Boolean);
}

function compactTokenCount(value: bigint): string {
  const numeric = Number(value);
  if (Number.isSafeInteger(numeric)) {
    return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(numeric);
  }
  return `${value} tokens`;
}

function attemptUsageSummary(job: DelegationJob): string | null {
  let input = 0n;
  let output = 0n;
  let cacheRead = 0n;
  let cacheWrite = 0n;
  let measured = false;
  for (const attempt of job.attempts) {
    const usage = attempt.usage;
    if (!usage) continue;
    measured = true;
    input += usage.inputTokens ?? 0n;
    output += usage.outputTokens ?? 0n;
    cacheRead += usage.cacheReadTokens ?? 0n;
    cacheWrite += usage.cacheWriteTokens ?? 0n;
  }
  if (!measured) return null;
  return `usage ${compactTokenCount(input)} in · ${compactTokenCount(output)} out · cache ${compactTokenCount(cacheRead)} read / ${compactTokenCount(cacheWrite)} write`;
}

/**
 * Mounts the panel only while it is open.
 *
 * The body derives its task list, subagent list and outline from the whole
 * message array. Those ran on every streamed delta even with the panel closed,
 * because hooks cannot sit behind an early return — and since the panel no
 * longer opens itself, closed is the normal state, so that was very nearly all
 * the time. Splitting the component is what lets the work not happen: an
 * unmounted component has no hooks to run.
 */
export function WorkbenchPanel({ open, ...props }: Props) {
  if (!open) return null;
  return <WorkbenchBody {...props} />;
}

function FileBrowser() {
  const [directory, setDirectory] = useState("");
  const [entries, setEntries] = useState<WorkspaceFileView[]>([]);
  const [preview, setPreview] = useState<WorkspaceFileContent | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const directoryRequest = useRef(0);
  const previewRequest = useRef(0);

  const loadDirectory = useCallback(async (relativePath: string) => {
    const request = ++directoryRequest.current;
    setLoading(true);
    setError(null);
    try {
      const nextEntries = await getBackend().listWorkspaceFiles(relativePath || null);
      if (request !== directoryRequest.current) return;
      setEntries(nextEntries);
    } catch (cause) {
      if (request !== directoryRequest.current) return;
      setEntries([]);
      setError(cause instanceof Error ? cause.message : "Could not list workspace files.");
    } finally {
      if (request === directoryRequest.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadDirectory("");
  }, [loadDirectory]);

  async function openEntry(entry: WorkspaceFileView) {
    if (entry.kind === "directory") {
      previewRequest.current += 1;
      setDirectory(entry.path);
      setPreview(null);
      await loadDirectory(entry.path);
      return;
    }

    setError(null);
    const request = ++previewRequest.current;
    try {
      const nextPreview = await getBackend().readWorkspaceFile(entry.path);
      if (request === previewRequest.current) setPreview(nextPreview);
    } catch (cause) {
      if (request !== previewRequest.current) return;
      setPreview(null);
      setError(cause instanceof Error ? cause.message : "Could not preview this file.");
    }
  }

  async function goUp() {
    const parts = directory.split("/").filter(Boolean);
    parts.pop();
    const next = parts.join("/");
    previewRequest.current += 1;
    setDirectory(next);
    setPreview(null);
    await loadDirectory(next);
  }

  return (
    <section className="flex flex-col gap-2.5">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
            <FolderIcon className="size-3.5" aria-hidden="true" />
            Workspace files
          </div>
          <div className="mt-1 truncate font-mono text-[10px] text-foreground/80" title={directory || "."}>
            {directory || "."}
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          title="Refresh files"
          aria-label="Refresh files"
          disabled={loading}
          onClick={() => void loadDirectory(directory)}
        >
          <RefreshCwIcon className={cn(loading && "animate-spin")} aria-hidden="true" />
        </Button>
      </div>

      {directory ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="justify-start"
          disabled={loading}
          onClick={() => void goUp()}
        >
          <ArrowLeftIcon data-icon="inline-start" />
          Parent folder
        </Button>
      ) : null}

      {error ? (
        <p className="m-0 rounded-md border border-destructive/40 bg-destructive/5 px-2.5 py-2 text-[11px] text-destructive">
          {error}
        </p>
      ) : null}

      {loading && entries.length === 0 ? (
        <p className="m-0 px-1 py-3 text-[11px] text-muted-foreground">Reading workspace…</p>
      ) : entries.length ? (
        <div className="flex flex-col gap-0.5">
          {entries.map((entry) => (
            <button
              type="button"
              key={entry.path}
              className="flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[11px] transition-colors hover:bg-secondary/60"
              onClick={() => void openEntry(entry)}
            >
              {entry.kind === "directory" ? (
                <FolderIcon className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
              ) : (
                <FileTextIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
              )}
              <span className="min-w-0 flex-1 truncate font-mono" title={entry.name}>
                {entry.name}
              </span>
              {entry.kind === "file" && entry.size != null ? (
                <span
                  className="shrink-0 text-[10px] text-muted-foreground"
                  title={entry.modifiedAt != null ? `Modified ${formatFileDate(entry.modifiedAt)}` : undefined}
                >
                  {formatFileSize(entry.size)}
                  {entry.modifiedAt != null ? ` · ${formatFileDate(entry.modifiedAt)}` : null}
                </span>
              ) : null}
              {entry.kind === "directory" ? <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" /> : null}
            </button>
          ))}
        </div>
      ) : (
        <p className="m-0 rounded-md border border-dashed border-border/70 px-2.5 py-3 text-center text-[11px] text-muted-foreground">
          This folder is empty.
        </p>
      )}

      {preview ? (
        <article className="min-h-0 border-t border-border/60 pt-2.5">
          <div className="flex items-baseline justify-between gap-2">
            <h3 className="m-0 min-w-0 truncate text-[11px] font-medium" title={preview.path}>
              {preview.path}
            </h3>
            <span className="shrink-0 text-[10px] text-muted-foreground">
              {formatFileSize(preview.byteCount)}
            </span>
          </div>
          <pre className="mt-2 max-h-[260px] overflow-auto whitespace-pre-wrap break-words rounded-md bg-background/70 p-2 font-mono text-[10px] leading-relaxed text-foreground/85">
            {preview.content}
          </pre>
          {preview.truncated ? (
            <p className="m-0 mt-1 text-[10px] text-amber-300">
              Preview capped at 200 KB.
            </p>
          ) : null}
        </article>
      ) : null}
    </section>
  );
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatFileDate(epochSecs: number): string {
  const date = new Date(epochSecs * 1000);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function WorkbenchBody({
  session,
  messages,
  sending,
  compacting,
  review,
  onClose,
  onVerify,
  onRewind,
  onJump,
  delegationJobs,
  onCreateDelegation,
  onApproveDelegation,
  onCancelDelegation,
  onRetryDelegation,
  onApplyDelegation,
  onReconnectProvider,
}: PanelProps) {
  const [tab, setTab] = useState<Tab>("activity");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [workUnitsOpen, setWorkUnitsOpen] = useState(true);
  const [expandedDelegation, setExpandedDelegation] = useState<{
    jobId: string;
    section: "worker" | "review";
  } | null>(null);
  const [delegationFilter, setDelegationFilter] = useState<"project" | "chat">("project");
  const [targetOptions, setTargetOptions] = useState<DelegationTargetOptionView[]>([]);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [creatingJob, setCreatingJob] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [objective, setObjective] = useState("");
  const [lane, setLane] = useState("product");
  const [scope, setScope] = useState("");
  const [context, setContext] = useState("");
  const [dependsOn, setDependsOn] = useState("");
  const [acceptanceChecks, setAcceptanceChecks] = useState("");
  const [workerKey, setWorkerKey] = useState("");
  const [reviewerKey, setReviewerKey] = useState("same");
  const [workerModel, setWorkerModel] = useState("");
  const [workerEffort, setWorkerEffort] = useState("");
  const panelRef = useRef<HTMLElement>(null);
  const titleId = useId();
  const workUnitsId = useId();

  useEffect(() => {
    if (tab !== "delegation") return;
    let cancelled = false;
    void getBackend().listDelegationTargets().then((targets) => {
      if (cancelled) return;
      setTargetOptions(targets);
      setTargetError(null);
      setWorkerKey((current) => {
        if (current) return current;
        const firstAvailable = targets.find((target) => target.available);
        return firstAvailable ? targetKey(firstAvailable.target) : "";
      });
    }).catch((cause) => {
      if (!cancelled) setTargetError(cause instanceof Error ? cause.message : "Could not load delegation targets.");
    });
    return () => { cancelled = true; };
  }, [tab]);

  // Mount is open, because this component only exists while the panel is.
  useEffect(() => {
    // The panel only opens because someone asked for it, so moving focus into
    // it is following the user rather than stealing from them.
    panelRef.current?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const tasks = useMemo(
    () =>
      messages
        .flatMap((message) =>
          message.role === "assistant"
            ? message.tools.map((tool) => ({ ...tool, messageId: message.id }))
            : []
        )
        .reverse(),
    [messages]
  );

  const subagents = useMemo(() => {
    const latest = new Map<
      string,
      {
        id: string;
        label: string;
        model: string;
        status: string;
        messageId: string;
      }
    >();
    for (const task of tasks) {
      if (task.name !== "delegate_external" || task.metadata?.kind !== "delegation") {
        continue;
      }
      const id = task.metadata.provider_id;
      if (latest.has(id)) continue;
      latest.set(id, {
        id,
        label: subagentLabel(id),
        model: task.metadata.model,
        status: task.status,
        messageId: task.messageId,
      });
    }
    return [...latest.values()];
  }, [tasks]);

  const outline = useMemo(
    () => messages.filter((message) => message.text.trim() || message.role === "assistant"),
    [messages]
  );

  const jobMessageIds = useMemo(() => {
    const ids = new Map<string, string>();
    for (const message of messages) {
      if (message.role !== "assistant") continue;
      for (const tool of message.tools) {
        const jobId = tool.metadata?.kind === "delegation" ? tool.metadata.job_id : undefined;
        if (jobId && !ids.has(jobId)) ids.set(jobId, message.id);
      }
    }
    return ids;
  }, [messages]);

  async function runVerify() {
    setBusyAction("verify");
    try {
      await onVerify();
    } finally {
      setBusyAction(null);
    }
  }

  async function runRewind(id: string) {
    setBusyAction(id);
    try {
      await onRewind(id);
    } finally {
      setBusyAction(null);
    }
  }

  async function runDelegationAction(
    id: string,
    action: (jobId: string) => Promise<void>
  ) {
    setBusyAction(id);
    try {
      await action(id);
    } finally {
      setBusyAction(null);
    }
  }

  function delegationStatusLabel(status: DelegationJob["status"]) {
    return status
      .replaceAll("_", " ")
      .replace(/\b\w/g, (letter) => letter.toUpperCase());
  }

  async function createDelegation() {
    const selected = targetOptions.find((option) => targetKey(option.target) === workerKey);
    if (!objective.trim() || !selected?.available) return;
    setCreatingJob(true);
    try {
      const worker: DelegationTarget = selected.target.kind === "provider"
        ? { ...selected.target, model: workerModel.trim() || null, effort: workerEffort || null }
        : selected.target;
      const reviewer = reviewerKey === "same"
        ? { kind: "sameAsWorker" as const }
        : (() => {
            const option = targetOptions.find((candidate) => targetKey(candidate.target) === reviewerKey);
            return option ? { kind: "target" as const, target: option.target } : { kind: "sameAsWorker" as const };
          })();
      await onCreateDelegation({
        parentThreadId: session.threadId,
        chatId: session.threadId,
        title: objective.trim().split("\n")[0].slice(0, 120),
        objective: objective.trim(),
        lane: lane.trim() || "product",
        scope: lines(scope),
        context: lines(context),
        dependsOn: lines(dependsOn),
        acceptanceChecks: lines(acceptanceChecks),
        worker,
        reviewer,
      });
      setObjective("");
      setScope("");
      setContext("");
      setDependsOn("");
      setAcceptanceChecks("");
      setCreateOpen(false);
    } catch (cause) {
      setTargetError(cause instanceof Error ? cause.message : "Could not create delegation job.");
    } finally {
      setCreatingJob(false);
    }
  }

  async function sendDelegationToChat(jobId: string) {
    setBusyAction(`handoff:${jobId}`);
    try {
      const handoff = await getBackend().prepareDelegationHandoff(jobId);
      window.dispatchEvent(new CustomEvent("zest:delegation-handoff", { detail: handoff }));
    } finally {
      setBusyAction(null);
    }
  }

  const visibleDelegationJobs = delegationJobs.filter((job) =>
    delegationFilter === "project" || job.parentThreadId === session.threadId
  );


  return (
    // Non-modal: the panel floats over the transcript but claims none of it.
    //
    // There used to be a transparent full-size button here to catch clicks
    // outside. It also caught every wheel event, so the transcript could not be
    // scrolled while the panel was open — the page looked frozen. A review
    // surface has no business freezing the thing being reviewed, so the panel
    // is dismissed by its own close button, Escape, or the toggle, and the
    // wrapper stays `pointer-events-none` so everything behind it still works.
    <div className="pointer-events-none absolute inset-0 z-30 flex items-center justify-end p-3 sm:p-4">
      <aside
        ref={panelRef}
        id="workbench-panel"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="pointer-events-auto relative z-10 flex h-full max-h-[720px] w-[min(360px,calc(100%_-_24px))] min-w-0 flex-col overflow-hidden rounded-xl border border-border/70 bg-card text-card-foreground shadow-2xl outline-none"
      >
      <header className="flex shrink-0 items-center justify-between border-b border-border/60 px-3 py-2.5">
        <div>
          <h2 id={titleId} className="flex items-center gap-2 text-sm font-semibold">
            <WrenchIcon className="size-4 text-primary" aria-hidden="true" />
            Workbench
          </h2>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          title="Close Workbench"
          aria-label="Close Workbench"
          onClick={onClose}
        >
          <PanelRightCloseIcon aria-hidden="true" />
        </Button>
      </header>

      <div
        role="tablist"
        aria-label="Workbench views"
        className="grid grid-cols-4 gap-1 border-b border-border/60 p-1.5"
      >
        {([
          ["activity", "Activity", Clock3Icon],
          ["outline", "Outline", ListTreeIcon],
          ["delegation", "Delegation", GitMergeIcon],
          ["files", "Files", FileTextIcon],
        ] as const).map(([id, label, Icon]) => (
          <button
            key={id}
            type="button"
            id={`workbench-tab-${id}`}
            role="tab"
            aria-selected={tab === id}
            aria-controls="workbench-content"
            tabIndex={tab === id ? 0 : -1}
            className={cn(
              "flex items-center justify-center gap-1.5 rounded-md px-2 py-1.5 text-xs transition-colors",
              tab === id
                ? "bg-secondary text-foreground"
                : "text-muted-foreground hover:bg-secondary/60 hover:text-foreground"
            )}
            onClick={() => setTab(id)}
            onKeyDown={(event) => {
              if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
                return;
              }
              event.preventDefault();
              const next: Tab =
                event.key === "Home"
                  ? "activity"
                  : event.key === "End"
                    ? "files"
                    : event.key === "ArrowRight" || event.key === "ArrowDown"
                      ? id === "activity"
                        ? "outline"
                        : id === "outline"
                          ? "delegation"
                          : id === "delegation"
                            ? "files"
                            : "activity"
                      : id === "activity"
                        ? "files"
                        : id === "outline"
                          ? "activity"
                          : id === "delegation"
                            ? "outline"
                            : "delegation";
              setTab(next);
              requestAnimationFrame(() => {
                document.getElementById(`workbench-tab-${next}`)?.focus();
              });
            }}
          >
            <Icon className="size-3.5" aria-hidden="true" />
            {label}
          </button>
        ))}
      </div>

      <div
        id="workbench-content"
        role="tabpanel"
        aria-labelledby={`workbench-tab-${tab}`}
        tabIndex={0}
        className="min-h-0 flex-1 overflow-y-auto px-2.5 py-2.5 outline-none"
      >
        {tab === "activity" ? (
          <div className="flex flex-col gap-2.5">
            <section className="border-b border-border/60 pb-3">
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="truncate text-sm font-medium">{session.label}</div>
                  <div className="mt-0.5 truncate font-mono text-[10px] text-muted-foreground">
                    {session.model}
                  </div>
                </div>
                <span
                  className={cn(
                    "inline-flex items-center gap-1.5 rounded-full px-2 py-1 text-[10px] font-medium",
                    sending || compacting
                      ? "bg-primary/12 text-primary"
                      : "bg-secondary text-muted-foreground"
                  )}
                >
                  <span className={cn("size-1.5 rounded-full", sending || compacting ? "bg-primary" : "bg-muted-foreground")} />
                  {sending ? "Working" : compacting ? "Compacting" : "Ready"}
                </span>
              </div>
            </section>

            {subagents.length ? (
              <section className="border-b border-border/60 pb-2.5">
                <div className="mb-1.5 flex items-center justify-between px-1">
                  <h2 className="m-0 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                    <BotIcon className="size-3.5" aria-hidden="true" />
                    Subagents
                  </h2>
                  <span className="text-[10px] text-muted-foreground">{subagents.length}</span>
                </div>
                <div className="flex flex-col gap-1.5">
                  {subagents.map((subagent) => (
                    <button
                      type="button"
                      key={subagent.id}
                      className="group flex w-full items-center gap-2 border-b border-border/60 px-1 py-2 text-left transition-colors last:border-b-0 hover:bg-secondary/40"
                      onClick={() => onJump(subagent.messageId)}
                      aria-label={`${subagent.label}, ${subagentStatus(subagent.status)}`}
                    >
                      <span className="relative shrink-0" aria-hidden="true">
                        <Blobatar
                          name={subagent.id}
                          size={28}
                          background={false}
                          tone={BLOBATAR_TONE}
                          className="block"
                        />
                        <span
                          className={cn(
                            "absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full border-2 border-card",
                            statusDotClass(subagent.status)
                          )}
                        />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-xs font-medium">{subagent.label}</span>
                        <span className="mt-0.5 block truncate text-[10px] text-muted-foreground">
                          {subagent.model} / {subagentStatus(subagent.status)}
                        </span>
                      </span>
                      <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
                    </button>
                  ))}
                </div>
              </section>
            ) : null}

            <section className="border-b border-border/60 pb-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                    {review?.patchCheck === "issues" ? (
                      <TriangleAlertIcon className="size-3.5 text-amber-400" aria-hidden="true" />
                    ) : review ? (
                      <CheckCircle2Icon className="size-3.5 text-primary" aria-hidden="true" />
                    ) : null}
                    Workspace check
                  </div>
                  {review?.summary ? (
                    <div className="mt-1 text-xs text-foreground">{review.summary}</div>
                  ) : null}
                </div>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={sending || compacting || busyAction !== null}
                  onClick={() => void runVerify()}
                >
                  <RefreshCwIcon
                    data-icon="inline-start"
                    className={cn(busyAction === "verify" && "animate-spin")}
                    aria-hidden="true"
                  />
                  {review ? "Run again" : "Verify"}
                </Button>
              </div>
              {review ? (
                <div className="mt-3 flex flex-col gap-2 text-[11px]">
                  <div className="flex items-center justify-between gap-3 rounded-md bg-secondary/50 px-2 py-1.5">
                    <span className="text-muted-foreground">Patch check</span>
                    <span
                      className={cn(
                        "font-medium",
                        review.patchCheck === "clean"
                          ? "text-primary"
                          : review.patchCheck === "issues"
                            ? "text-amber-400"
                            : "text-muted-foreground"
                      )}
                    >
                      {review.patchCheck === "clean"
                        ? "Clear"
                        : review.patchCheck === "issues"
                          ? "Review needed"
                          : "Unavailable"}
                    </span>
                  </div>
                  {review.changedFiles.length > 0 ? (
                    <div className="flex flex-col gap-1 rounded-md bg-secondary/50 px-2 py-1.5">
                      <div className="text-muted-foreground">
                        Changed files ({review.changedFileCount})
                      </div>
                      <ul className="flex flex-col gap-0.5 font-mono text-[10px] text-foreground/80">
                        {review.changedFiles.slice(0, 5).map((file) => (
                          <li key={file} className="truncate" title={file}>
                            {file}
                          </li>
                        ))}
                      </ul>
                      {review.changedFileCount > review.changedFiles.length ? (
                        <div className="text-[10px] text-muted-foreground">
                          +{review.changedFileCount - review.changedFiles.length} more
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </section>

            <section>
              <h2 className="m-0">
                <button
                  type="button"
                  aria-expanded={workUnitsOpen}
                  aria-controls={workUnitsId}
                  onClick={() => setWorkUnitsOpen((open) => !open)}
                  className="mb-1.5 flex w-full items-center justify-between gap-2 rounded-md px-1 py-0.5 text-left text-[11px] font-medium uppercase tracking-wide text-muted-foreground outline-none transition-colors hover:bg-secondary/40 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
                >
                  <span className="flex items-center gap-1">
                    <ChevronRightIcon
                      className={cn(
                        "size-3 transition-transform",
                        workUnitsOpen && "rotate-90"
                      )}
                      aria-hidden="true"
                    />
                    Work units
                  </span>
                  <span className="text-[10px] tabular-nums">{tasks.length}</span>
                </button>
              </h2>
              {!workUnitsOpen ? null : tasks.length === 0 ? (
                <div className="px-1 py-1 text-[11px] text-muted-foreground">Nothing yet.</div>
              ) : (
                <div id={workUnitsId} className="flex flex-col gap-1.5">
                  {tasks.slice(0, 12).map((task) => (
                    <button
                      type="button"
                      key={task.id}
                      className="group flex w-full items-start gap-2 border-b border-border/60 px-1 py-2 text-left transition-colors last:border-b-0 hover:bg-secondary/40"
                      onClick={() => onJump(task.messageId)}
                    >
                      <span className="mt-0.5 shrink-0" aria-hidden="true">{statusIcon(task.status)}</span>
                      <span className="min-w-0 flex-1">
                        <span className="flex items-center gap-1.5">
                          <span className="truncate font-mono text-[11px]">{task.name}</span>
                          <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
                        </span>
                        <span className="mt-0.5 block truncate text-[10px] text-muted-foreground">
                          {task.summary || task.path || task.status.replaceAll("_", " ")}
                        </span>
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </section>

            <section>
              <div className="mb-1.5 flex items-center justify-between px-1">
                <h2 className="m-0 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                  Recovery
                </h2>
                <span className="text-[10px] text-muted-foreground">
                  {session.checkpoints.length} checkpoints
                </span>
              </div>
              {session.checkpoints.length ? (
                <div className="flex flex-col gap-1">
                  {session.checkpoints
                    .slice()
                    .reverse()
                    .slice(0, 6)
                    .map((checkpoint) => (
                      <div key={checkpoint.id} className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-secondary/50">
                        <HistoryIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-[11px]">{checkpoint.label}</span>
                          <span className="block text-[10px] text-muted-foreground">
                            {checkpoint.messageCount} messages · {formatAge(checkpoint.createdAt)}
                          </span>
                        </span>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-xs"
                          title={`Rewind to ${checkpoint.label}`}
                          aria-label={`Rewind to ${checkpoint.label}`}
                          disabled={sending || busyAction !== null}
                          onClick={() => void runRewind(checkpoint.id)}
                        >
                          <RefreshCwIcon aria-hidden="true" />
                        </Button>
                      </div>
                    ))}
                </div>
              ) : null}
            </section>
          </div>
        ) : tab === "outline" ? (
          <section>
            {outline.length === 0 ? (
              <div className="px-1 py-1 text-[11px] text-muted-foreground">Nothing yet.</div>
            ) : (
              <div className="flex flex-col gap-1">
                {outline.map((message, index) => (
                  <button
                    type="button"
                    key={message.id}
                    className="group flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-secondary/60"
                    onClick={() => onJump(message.id)}
                  >
                    <span className="mt-0.5 w-5 shrink-0 text-right font-mono text-[10px] text-muted-foreground">
                      {index + 1}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-[10px] uppercase tracking-wide text-muted-foreground">
                        {message.role === "user" ? "You" : "Zest"}
                      </span>
                      <span className="mt-0.5 block text-xs leading-relaxed text-foreground/85">
                        {messagePreview(message)}
                      </span>
                    </span>
                    <ChevronRightIcon className="mt-1 size-3 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" aria-hidden="true" />
                  </button>
                ))}
              </div>
            )}
          </section>
        ) : tab === "files" ? (
          <FileBrowser />
        ) : (
          <section className="flex flex-col gap-2">
            <div className="flex items-center justify-between gap-2 px-1">
              <div>
                <h2 className="m-0 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">Agent Board</h2>
                <p className="m-0 mt-0.5 text-[10px] text-muted-foreground">Project-level work, approval, and review.</p>
              </div>
              <Button type="button" size="sm" variant="outline" onClick={() => setCreateOpen((open) => !open)}><PlusIcon data-icon="inline-start" />New job</Button>
            </div>
            <div className="flex items-center gap-1 rounded-md border border-border/60 bg-background/40 p-1">
              {(["project", "chat"] as const).map((filter) => (
                <button key={filter} type="button" className={cn("flex-1 rounded px-2 py-1 text-[10px]", delegationFilter === filter ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground")} onClick={() => setDelegationFilter(filter)}>
                  {filter === "project" ? "All project" : "Current chat"}
                </button>
              ))}
            </div>
            {visibleDelegationJobs.length ? (
              <div className="flex flex-col gap-1.5">
                {visibleDelegationJobs.slice(0, 6).map((job) => (
                  <OrchestrationStatus key={`orchestration:${job.jobId}`} job={job} />
                ))}
              </div>
            ) : null}
            {createOpen ? (
              <div className="rounded-lg border border-primary/30 bg-primary/5 p-2.5">
                <div className="mb-2 text-[11px] font-medium">Create a bounded job</div>
                <div className="flex flex-col gap-2">
                  <textarea value={objective} onChange={(event) => setObjective(event.target.value)} placeholder="Objective" rows={3} className="resize-y rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" />
                  <div className="grid grid-cols-2 gap-2"><input value={lane} onChange={(event) => setLane(event.target.value)} placeholder="Lane" className="rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" /><input value={scope} onChange={(event) => setScope(event.target.value)} placeholder="Scope (one path per line)" className="rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" /></div>
                  <textarea value={context} onChange={(event) => setContext(event.target.value)} placeholder="Context (one item per line)" rows={2} className="resize-y rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" />
                  <div className="grid grid-cols-2 gap-2"><input value={dependsOn} onChange={(event) => setDependsOn(event.target.value)} placeholder="Dependencies (job IDs)" className="rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" /><input value={acceptanceChecks} onChange={(event) => setAcceptanceChecks(event.target.value)} placeholder="Acceptance checks (commands)" className="rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" /></div>
                  <label className="text-[10px] text-muted-foreground">Worker target<select value={workerKey} onChange={(event) => setWorkerKey(event.target.value)} className="mt-1 w-full rounded-md border border-border bg-background px-2 py-1.5 text-[11px] text-foreground"><option value="">Choose an available target</option>{(["provider", "externalAgent"] as const).map((kind) => <optgroup key={kind} label={kind === "provider" ? "Zest providers" : "External CLI agents"}>{targetOptions.filter((option) => option.target.kind === kind).map((option) => { const value = targetKey(option.target); return <option key={value} value={value} disabled={!option.available}>{option.label}{option.available ? "" : " — unavailable"}</option>; })}</optgroup>)}</select></label>
                  {targetOptions.find((option) => targetKey(option.target) === workerKey && !option.available)?.error ? <p className="m-0 text-[10px] text-amber-300">{targetOptions.find((option) => targetKey(option.target) === workerKey)?.error} Reconnect or choose another target; there is no automatic fallback.</p> : null}
                  {targetOptions.find((option) => targetKey(option.target) === workerKey)?.target.kind === "provider" ? <div className="grid grid-cols-2 gap-2"><input value={workerModel} onChange={(event) => setWorkerModel(event.target.value)} placeholder="Model (optional)" className="rounded-md border border-border bg-background/70 px-2 py-1.5 text-[11px] outline-none focus:border-primary" /><select value={workerEffort} onChange={(event) => setWorkerEffort(event.target.value)} className="rounded-md border border-border bg-background px-2 py-1.5 text-[11px] text-foreground"><option value="">Default effort</option><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option><option value="xhigh">XHigh</option><option value="max">Max</option></select></div> : null}
                  <label className="text-[10px] text-muted-foreground">Reviewer target<select value={reviewerKey} onChange={(event) => setReviewerKey(event.target.value)} className="mt-1 w-full rounded-md border border-border bg-background px-2 py-1.5 text-[11px] text-foreground"><option value="same">Same worker, fresh reviewer runtime</option>{targetOptions.map((option) => <option key={`reviewer:${targetKey(option.target)}`} value={targetKey(option.target)} disabled={!option.available}>{option.label}{option.available ? "" : " — unavailable"}</option>)}</select></label>
                  {targetError ? <p className="m-0 text-[10px] text-amber-300">{targetError}</p> : null}
                  {targetOptions.filter((option) => !option.available).map((option) => {
                    const providerId = option.target.kind === "provider" ? option.target.providerId : null;
                    return (
                      <div key={`unavailable:${targetKey(option.target)}`} className="flex items-center justify-between gap-2 rounded-md border border-amber-400/20 bg-amber-400/5 px-2 py-1.5 text-[10px] text-amber-200">
                        <span className="min-w-0">{option.label}: {option.error ?? "Unavailable"}</span>
                        {providerId && onReconnectProvider ? (
                          <Button type="button" variant="outline" size="sm" className="shrink-0" onClick={() => onReconnectProvider(providerId)}>Reconnect</Button>
                        ) : <span className="shrink-0 text-[10px] text-muted-foreground">Choose another target</span>}
                      </div>
                    );
                  })}
                  <div className="flex justify-end gap-1.5"><Button type="button" size="sm" variant="ghost" onClick={() => setCreateOpen(false)}>Cancel</Button><Button type="button" size="sm" disabled={creatingJob || !objective.trim() || !workerKey} onClick={() => void createDelegation()}>{creatingJob ? "Creating…" : "Create awaiting approval"}</Button></div>
                </div>
              </div>
            ) : null}
            {visibleDelegationJobs.length === 0 ? (
              <div className="px-1 py-1 text-[11px] text-muted-foreground">Nothing yet.</div>
            ) : (
              <div className="flex flex-col gap-2">
                {visibleDelegationJobs.map((job) => {
                  const sourceMessage = jobMessageIds.get(job.jobId);
                  const checksPassed = job.acceptanceChecks.filter(
                    (check) => check.status === "passed"
                  ).length;
                  const blocking = job.reviewerFindings.filter(
                    (finding) => finding.severity === "blocking"
                  );
                  const usage = attemptUsageSummary(job);
                  const reviewerTarget = job.reviewerTarget.kind === "target"
                    ? job.reviewerTarget.target
                    : undefined;
                  const reconnectProviderId = targetProviderId(job.workerTarget)
                    ?? targetProviderId(reviewerTarget);
                  return (
                    <article
                      key={job.jobId}
                      className="rounded-lg border border-border/70 bg-secondary/20 p-2.5"
                    >
                      <div className="flex items-start gap-2">
                        <span className="mt-0.5 shrink-0" aria-hidden="true">
                          {job.status === "awaiting_approval" ? (
                            <TriangleAlertIcon className="size-4 text-amber-400" />
                          ) : job.status === "queued" ? (
                            <Clock3Icon className="size-4 text-muted-foreground" />
                          ) : job.status === "accepted" || job.status === "ready_to_apply" ? (
                            <CheckCircle2Icon className="size-4 text-primary" />
                          ) : job.status === "failed" || job.status === "blocked" || job.status === "apply_conflict" ? (
                            <TriangleAlertIcon className="size-4 text-amber-400" />
                          ) : job.status === "cancelled" ? (
                            <XCircleIcon className="size-4 text-muted-foreground" />
                          ) : (
                            <RefreshCwIcon className="size-4 animate-spin text-primary" />
                          )}
                        </span>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-start justify-between gap-2">
                            <h3 className="truncate text-xs font-semibold">{job.title}</h3>
                            <span className="shrink-0 text-[10px] text-muted-foreground">
                              {delegationStatusLabel(job.status)}
                            </span>
                          </div>
                          <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10px] text-muted-foreground">
                            <span>{job.lane}</span>
                            <span className="inline-flex items-center gap-1">
                              <Blobatar
                                name={job.agent}
                                size={12}
                                background={false}
                                tone={BLOBATAR_TONE}
                                className="block"
                              />
                              {job.agent}
                            </span>
                            <span className="inline-flex items-center gap-1">
                              <Blobatar
                                name={job.reviewerAgent}
                                size={12}
                                background={false}
                                tone={BLOBATAR_TONE}
                                className="block"
                              />
                              {job.reviewerAgent}
                            </span>
                            <span>Attempt {job.attempt || 1}</span>
                          </div>
                        </div>
                      </div>
                      <p className="mt-2 line-clamp-3 text-[11px] leading-relaxed text-foreground/80">
                        {job.workerSummary ?? job.objective}
                      </p>
                      <div className="mt-2 flex flex-wrap gap-x-3 text-[10px] text-muted-foreground">
                        <span>
                          <span className="font-mono text-foreground">{job.changedFileCount}</span> files
                        </span>
                        <span>
                          <span className="font-mono text-foreground">
                            {checksPassed}/{job.acceptanceChecks.length || 0}
                          </span>{" "}
                          checks passed
                        </span>
                        {usage ? <span className="truncate" title={usage}>{usage}</span> : null}
                      </div>
                      {job.changedFiles.length > 0 ? (
                        <div className="mt-2 rounded-md bg-secondary/50 px-2 py-1.5 font-mono text-[10px] text-foreground/80">
                          {job.changedFiles.slice(0, 4).map((file) => (
                            <div key={file} className="truncate">{file}</div>
                          ))}
                          {job.changedFileCount > 4 ? <div className="text-muted-foreground">+{job.changedFileCount - 4} more</div> : null}
                        </div>
                      ) : null}
                      {blocking.length > 0 ? (
                        <div className="mt-2 rounded-md border border-amber-400/30 bg-amber-400/10 px-2 py-1.5 text-[10px] text-amber-200">
                          {blocking.slice(0, 2).map((finding) => (
                            <div key={`${finding.path}:${finding.message}`} className="line-clamp-2">
                              <span className="font-mono">{finding.path}</span>: {finding.message}
                            </div>
                          ))}
                        </div>
                      ) : null}
                      {job.error ? (
                        <div className="mt-2 flex items-start justify-between gap-2 rounded-md border border-amber-400/20 bg-amber-400/5 px-2 py-1.5 text-[10px] text-amber-300">
                          <span className="line-clamp-3 min-w-0">{job.error}</span>
                          {reconnectProviderId && onReconnectProvider && isTargetAvailabilityError(job.error) ? (
                            <Button type="button" variant="outline" size="sm" className="shrink-0" onClick={() => onReconnectProvider(reconnectProviderId)}>
                              Reconnect
                            </Button>
                          ) : null}
                        </div>
                      ) : null}
                      <div className="mt-2 flex flex-wrap gap-1.5">
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() =>
                            setExpandedDelegation((current) =>
                              current?.jobId === job.jobId && current.section === "worker"
                                ? null
                                : { jobId: job.jobId, section: "worker" }
                            )
                          }
                        >
                          <BotIcon data-icon="inline-start" />
                          Worker result
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() =>
                            setExpandedDelegation((current) =>
                              current?.jobId === job.jobId && current.section === "review"
                                ? null
                                : { jobId: job.jobId, section: "review" }
                            )
                          }
                        >
                          <TriangleAlertIcon data-icon="inline-start" />
                          Reviewer findings
                        </Button>
                        {sourceMessage ? (
                          <Button type="button" variant="ghost" size="sm" onClick={() => onJump(sourceMessage)}>
                            <ChevronRightIcon data-icon="inline-start" />
                            Coordinator message
                          </Button>
                        ) : null}
                        {job.status === "ready_to_apply" ? (
                          <Button
                            type="button"
                            size="sm"
                            disabled={busyAction !== null}
                            onClick={() => void runDelegationAction(job.jobId, onApplyDelegation)}
                          >
                            <GitMergeIcon data-icon="inline-start" />
                            Apply accepted changes
                          </Button>
                        ) : null}
                        {job.status === "changes_requested" || job.status === "apply_conflict" || job.status === "blocked" || job.status === "failed" ? (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={busyAction !== null}
                            onClick={() => void runDelegationAction(job.jobId, onRetryDelegation)}
                          >
                            <RefreshCwIcon data-icon="inline-start" />
                            Retry (await approval)
                          </Button>
                        ) : null}
                        {job.status === "awaiting_approval" ? (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            disabled={busyAction !== null}
                            onClick={() => void runDelegationAction(job.jobId, onApproveDelegation)}
                          >
                            <PlayIcon data-icon="inline-start" />
                            Approve
                          </Button>
                        ) : null}
                        {job.status !== "awaiting_approval" && job.status !== "planned" ? (
                          <Button type="button" variant="ghost" size="sm" disabled={busyAction !== null} onClick={() => void sendDelegationToChat(job.jobId)}>
                            <SendIcon data-icon="inline-start" />
                            Send to chat
                          </Button>
                        ) : null}
                        {!['accepted', 'cancelled', 'failed'].includes(job.status) ? (
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            disabled={busyAction !== null}
                            onClick={() => void runDelegationAction(job.jobId, onCancelDelegation)}
                          >
                            <XIcon data-icon="inline-start" />
                            Cancel
                          </Button>
                        ) : null}
                      </div>
                      {expandedDelegation?.jobId === job.jobId ? (
                        <div className="mt-2 rounded-md border border-border/60 bg-background/50 p-2 text-[10px]">
                          {expandedDelegation.section === "worker" ? (
                            <>
                              <div className="font-medium text-foreground">Worker result</div>
                              <div className="mt-1 whitespace-pre-wrap text-foreground/80">
                                {job.workerSummary ?? "No usable worker result was persisted."}
                              </div>
                              {job.acceptanceChecks.length > 0 ? (
                                <div className="mt-2 flex flex-col gap-1">
                                  {job.acceptanceChecks.map((check) => (
                                    <div key={check.command}>
                                      <div className="font-mono text-foreground/80">{check.command}</div>
                                      <div className="text-muted-foreground">
                                        {check.status}: {check.output || "no output"}
                                      </div>
                                    </div>
                                  ))}
                                </div>
                              ) : null}
                            </>
                          ) : (
                            <>
                              <div className="font-medium text-foreground">Reviewer findings</div>
                              {job.reviewerFindings.length > 0 ? (
                                <div className="mt-1 flex flex-col gap-1">
                                  {job.reviewerFindings.map((finding) => (
                                    <div key={`${finding.severity}:${finding.path}:${finding.message}`}>
                                      <span className="font-mono text-foreground/80">{finding.path}</span>{" "}
                                      <span className="text-muted-foreground">({finding.severity}) {finding.message}</span>
                                    </div>
                                  ))}
                                </div>
                              ) : (
                                <div className="mt-1 text-muted-foreground">
                                  No structured reviewer findings were persisted.
                                </div>
                              )}
                              {job.acceptanceChecks.length > 0 ? (
                                <div className="mt-2 flex flex-col gap-1">
                                  {job.acceptanceChecks.map((check) => (
                                    <div key={check.command} className="text-muted-foreground">
                                      <span className="font-mono text-foreground/80">{check.command}</span>: {check.status}
                                      {check.output ? ` — ${check.output}` : ""}
                                    </div>
                                  ))}
                                </div>
                              ) : null}
                            </>
                          )}
                        </div>
                      ) : null}
                    </article>
                  );
                })}
              </div>
            )}
          </section>
        )}
      </div>
      </aside>
    </div>
  );
}
