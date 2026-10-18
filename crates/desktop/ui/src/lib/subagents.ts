/**
 * What a subagent line says about the run behind it: who ran it, how long it
 * took, where a board job is, and what the worker reported.
 */
import type { StepStatus } from "./assistantTrace.ts";
import type { DelegationJob, ToolPart } from "./types.ts";

/** A worker's display name from its configured id. */
export function agentLabel(id: string): string {
  if (id === "claude") return "Claude Code";
  if (id === "gemini") return "Gemini CLI";
  if (id === "cursor") return "Cursor CLI";
  return id.replace(/[-_]+/g, " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

/** `48s`, `3m 12s`, `1h 4m`. */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    const rest = seconds % 60;
    return rest === 0 ? `${minutes}m` : `${minutes}m ${rest}s`;
  }
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}

/** Elapsed time for a step, or nothing when it was never timed. */
export function stepDuration(
  startedAt: number | undefined,
  finishedAt: number | undefined,
  now: number
): string | undefined {
  if (startedAt == null) return undefined;
  const end = finishedAt ?? now;
  if (end < startedAt) return undefined;
  return formatDuration(end - startedAt);
}

/** A board job's state, in the terms of a subagent line. */
export type JobProgress = {
  status: StepStatus;
  /** Says where the job is, ahead of its title: `Worker running: …`. */
  label: string;
};

export function jobProgress(job: DelegationJob): JobProgress {
  switch (job.status) {
    case "planned":
    case "awaiting_approval":
      return { status: "pending", label: "Waiting for approval" };
    case "queued":
      return { status: "pending", label: "Queued" };
    case "worker_running":
      return { status: "running", label: "Worker running" };
    case "review_running":
      return { status: "running", label: "Reviewing" };
    case "ready_to_apply":
      return { status: undefined, label: "Ready to apply" };
    case "accepted":
      return { status: undefined, label: "Applied" };
    case "changes_requested":
      return { status: "error", label: "Changes requested" };
    case "blocked":
      return { status: "error", label: "Blocked" };
    case "apply_conflict":
      return { status: "error", label: "Could not apply" };
    case "failed":
      return { status: "error", label: "Failed" };
    case "cancelled":
      return { status: undefined, label: "Cancelled" };
  }
}

/** One step of a board job, shown nested under its subagent line. */
export type JobStep = {
  key: string;
  title: string;
  status: StepStatus;
  description?: string;
  detail?: string;
};

function attemptDuration(
  startedAt: bigint | number,
  finishedAt: bigint | number | null,
  now: number
): string | undefined {
  return stepDuration(
    Number(startedAt),
    finishedAt == null ? undefined : Number(finishedAt),
    now
  );
}

/**
 * The lifecycle of a feature card as Trace lines: who ran it, who reviewed
 * it, which checks passed. Read from the live board, so the chat row moves
 * with the job instead of freezing at "card created".
 */
export function jobSteps(job: DelegationJob, now: number): JobStep[] {
  const steps: JobStep[] = [];
  steps.push({
    key: "approval",
    title: job.approved ? "Approved the card" : "Waiting for approval",
    status: job.approved ? undefined : "pending",
  });
  for (const attempt of job.attempts) {
    const reviewing = attempt.role === "reviewer";
    const running = attempt.finishedAt == null;
    const live =
      running &&
      (reviewing ? job.status === "review_running" : job.status === "worker_running");
    const who = agentLabel(attempt.agent);
    steps.push({
      key: attempt.attemptId,
      title: reviewing
        ? live
          ? `Reviewing with ${who}`
          : `Reviewed with ${who}`
        : live
          ? `Running ${who}`
          : `Ran ${who}`,
      status: live ? "running" : running ? "error" : undefined,
      description: attemptDuration(attempt.startedAt, attempt.finishedAt, now),
    });
  }
  job.acceptanceChecks.forEach((check, index) => {
    steps.push({
      key: `check-${index}`,
      title:
        check.status === "passed"
          ? `Passed ${check.command}`
          : check.status === "failed"
            ? `Failed ${check.command}`
            : check.status === "skipped"
              ? `Skipped ${check.command}`
              : `Will check ${check.command}`,
      status:
        check.status === "failed" ? "error" : check.status === "pending" ? "pending" : undefined,
      detail: check.output.trim() || undefined,
    });
  });
  if (job.reviewerFindings.length > 0) {
    const count = job.reviewerFindings.length;
    steps.push({
      key: "findings",
      title: `Review found ${count} issue${count === 1 ? "" : "s"}`,
      status: job.status === "changes_requested" ? "error" : undefined,
      detail: job.reviewerFindings
        .map((finding) => `${finding.severity} ${finding.path}: ${finding.message}`)
        .join("\n"),
    });
  }
  return steps;
}

/**
 * What a delegated worker reported, without the `[agent · model]` line its
 * answer opens with: the line already shows the agent and model.
 */
export function delegationReport(tool: ToolPart): string | undefined {
  const summary = tool.summary?.trim();
  if (!summary || /^Delegated to /.test(summary)) return undefined;
  return summary.replace(/^\[[^\]\n]+\]\s*/, "").trim() || undefined;
}
