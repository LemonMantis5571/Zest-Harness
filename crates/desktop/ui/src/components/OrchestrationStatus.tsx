import { ActivityIcon, GitBranchIcon, InboxIcon, ShieldCheckIcon, TimerIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { cn } from "@/lib/utils";
import type { DelegationJob } from "@/lib/types";

type Props = { job: DelegationJob };
type Orchestration = NonNullable<DelegationJob["orchestration"]>;

function label(value: string) {
  return value.replaceAll("_", " ");
}

function shortId(value: string) {
  if (value.length <= 18) return value;
  return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function heartbeatAge(timestamp: number | null | undefined, now: number) {
  if (!timestamp) return "no heartbeat";
  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000));
  if (seconds < 5) return "heartbeat now";
  if (seconds < 60) return `heartbeat ${seconds}s ago`;
  return `heartbeat ${Math.floor(seconds / 60)}m ago`;
}

function tone(value: string) {
  if (["failed", "blocked", "rejected", "error"].includes(value)) {
    return "border-amber-400/30 bg-amber-400/10 text-amber-200";
  }
  if (["accepted", "approved", "completed", "resolved"].includes(value)) {
    return "border-primary/25 bg-primary/10 text-primary";
  }
  if (["running", "worker_running", "review_running", "queued"].includes(value)) {
    return "border-primary/20 bg-primary/5 text-foreground";
  }
  return "border-border/60 bg-secondary/40 text-muted-foreground";
}

function StatePill({ value }: { value: string }) {
  return (
    <span className={cn("rounded border px-1.5 py-0.5 text-[9px] capitalize", tone(value))}>
      {label(value)}
    </span>
  );
}

function lineageSummary(worktree: Orchestration["worktree"]) {
  const branch = worktree.branch ?? "detached";
  const start = worktree.startRef ? shortId(worktree.startRef) : "unknown start";
  return `${branch} · from ${start}`;
}

export function OrchestrationStatus({ job }: Props) {
  const state = job.orchestration;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 5_000);
    return () => window.clearInterval(timer);
  }, []);

  if (!state) return null;
  const openGates = state.decisionGates.filter((gate) => gate.status === "open");
  const recentMessages = state.inbox.slice(-2).reverse();
  const recentLifecycle = state.lifecycle.slice(-3).reverse();
  const sessions = (state.externalSessionHistory.length
    ? state.externalSessionHistory
    : state.externalSession
      ? [state.externalSession]
      : []).slice(-2).reverse();
  const dispatch = state.dispatch;

  return (
    <article className="rounded-lg border border-border/60 bg-background/40 px-2.5 py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[10px]">
        <span className="font-medium text-foreground">Run {shortId(state.runId)}</span>
        <span className="text-muted-foreground">Task {shortId(state.taskId)}</span>
        <StatePill value={state.phase} />
        {dispatch ? (
          <span className="inline-flex items-center gap-1 text-muted-foreground">
            <ActivityIcon className="size-3" aria-hidden="true" />
            {label(dispatch.role)} → {dispatch.target}
            <StatePill value={dispatch.status} />
          </span>
        ) : null}
        <span className="ml-auto inline-flex items-center gap-1 text-muted-foreground">
          <TimerIcon className="size-3" aria-hidden="true" />
          {heartbeatAge(state.heartbeatAt, now)}
        </span>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[10px] text-muted-foreground">
        <span className="inline-flex min-w-0 items-center gap-1 truncate" title={state.worktree.checkoutPath ?? undefined}>
          <GitBranchIcon className="size-3 shrink-0" aria-hidden="true" />
          {lineageSummary(state.worktree)}
        </span>
        {openGates.length ? (
          <span className="inline-flex items-center gap-1 text-amber-200">
            <ShieldCheckIcon className="size-3" aria-hidden="true" />
            {openGates.length} gate{openGates.length === 1 ? "" : "s"} open
          </span>
        ) : null}
        {state.retry.attempt > 0 ? (
          <span>retry {state.retry.attempt}</span>
        ) : null}
        {state.inbox.length ? (
          <span className="inline-flex items-center gap-1">
            <InboxIcon className="size-3" aria-hidden="true" />
            {state.inbox.length} message{state.inbox.length === 1 ? "" : "s"}
          </span>
        ) : null}
      </div>

      <details className="mt-1.5 border-t border-border/50 pt-1.5">
        <summary className="cursor-pointer text-[10px] text-muted-foreground hover:text-foreground">
          Lifecycle and worker evidence
        </summary>
        <div className="mt-2 flex flex-col gap-2 text-[10px]">
          <div className="grid grid-cols-2 gap-1.5 text-muted-foreground">
            <div className="rounded bg-secondary/40 px-2 py-1.5">
              <div className="text-[9px] uppercase tracking-wide">Lineage</div>
              <div className="mt-0.5 truncate font-mono text-foreground/80" title={state.worktree.checkoutPath ?? undefined}>
                {state.worktree.checkoutPath ?? "checkout unavailable"}
              </div>
              <div className="mt-0.5 truncate">base {state.worktree.baseRef ?? "unknown"}</div>
            </div>
            <div className="rounded bg-secondary/40 px-2 py-1.5">
              <div className="text-[9px] uppercase tracking-wide">Decision gates</div>
              {state.decisionGates.length ? (
                <div className="mt-0.5 flex flex-wrap gap-1">
                  {state.decisionGates.map((gate) => (
                    <StatePill key={gate.id} value={gate.status} />
                  ))}
                </div>
              ) : (
                <div className="mt-0.5 text-foreground/70">None recorded</div>
              )}
            </div>
          </div>

          {state.retry.attempt > 0 ? (
            <div className="rounded border border-amber-400/20 bg-amber-400/5 px-2 py-1.5 text-amber-200">
              Retry {state.retry.attempt}: {state.retry.lastError ?? state.retry.nextAction ?? "pending"}
            </div>
          ) : null}

          {recentLifecycle.length ? (
            <div className="flex flex-col gap-1 rounded bg-secondary/30 px-2 py-1.5">
              <div className="text-[9px] uppercase tracking-wide text-muted-foreground">Recent lifecycle</div>
              {recentLifecycle.map((entry, index) => (
                <div key={`${entry.at}-${entry.phase}-${index}`} className="flex gap-1.5">
                  <StatePill value={entry.phase} />
                  <span className="line-clamp-2 text-foreground/75">{entry.detail}</span>
                </div>
              ))}
            </div>
          ) : null}

          {recentMessages.length ? (
            <div className="flex flex-col gap-1 rounded bg-secondary/30 px-2 py-1.5">
              <div className="text-[9px] uppercase tracking-wide text-muted-foreground">Inbox</div>
              {recentMessages.map((message) => (
                <div key={message.id} className="flex gap-1.5">
                  <StatePill value={message.kind} />
                  <span className="min-w-0"><span className="text-muted-foreground">{message.sender}:</span> {message.body}</span>
                </div>
              ))}
            </div>
          ) : null}

          {sessions.length ? (
            <div className="flex flex-col gap-1 rounded border border-border/50 bg-secondary/20 px-2 py-1.5">
              <div className="text-[9px] uppercase tracking-wide text-muted-foreground">External worker evidence · not chat history</div>
              {sessions.map((session, index) => (
                <div key={`${session.capturedAt}-${session.sessionId ?? index}`} className="min-w-0 text-foreground/75">
                  <div className="flex flex-wrap gap-x-2">
                    <span>{session.workerId}</span>
                    {session.model ? <span className="text-muted-foreground">{session.model}</span> : null}
                    {session.sessionId ? <span className="font-mono text-muted-foreground">session {shortId(session.sessionId)}</span> : null}
                  </div>
                  <div className="truncate font-mono text-[9px] text-muted-foreground" title={session.cwd ?? undefined}>
                    {session.cwd ?? session.command}
                  </div>
                  {session.preview ? <div className="mt-0.5 line-clamp-2">{session.preview}</div> : null}
                </div>
              ))}
            </div>
          ) : null}
        </div>
      </details>
    </article>
  );
}
