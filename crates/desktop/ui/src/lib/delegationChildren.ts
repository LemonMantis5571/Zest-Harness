import type { DelegationJob } from "./types.ts";

/**
 * Feature-card jobs nested under the chat that started them.
 *
 * A job records its creating chat as `parentThreadId`, so the sidebar can show
 * at a glance that a worker was dispatched from a chat, and how it is doing,
 * without opening the Workbench.
 */

export type DelegationChildTone =
  | "attention"
  | "running"
  | "ready"
  | "waiting"
  | "done"
  | "cancelled";

export type DelegationChildStatus = { label: string; tone: DelegationChildTone };

/** What each job state says on its sidebar row, and how loudly. */
export function delegationChildStatus(status: DelegationJob["status"]): DelegationChildStatus {
  switch (status) {
    case "awaiting_approval":
      return { label: "Needs approval", tone: "attention" };
    case "changes_requested":
      return { label: "Changes requested", tone: "attention" };
    case "blocked":
      return { label: "Blocked", tone: "attention" };
    case "failed":
      return { label: "Failed", tone: "attention" };
    case "apply_conflict":
      return { label: "Apply conflict", tone: "attention" };
    case "worker_running":
      return { label: "Working", tone: "running" };
    case "review_running":
      return { label: "Reviewing", tone: "running" };
    case "ready_to_apply":
      return { label: "Ready to apply", tone: "ready" };
    case "planned":
      return { label: "Planned", tone: "waiting" };
    case "queued":
      return { label: "Queued", tone: "waiting" };
    case "accepted":
      return { label: "Applied", tone: "done" };
    case "cancelled":
      return { label: "Cancelled", tone: "cancelled" };
  }
}

/** Nothing left to do: the job was applied or called off. */
export function isSettledDelegation(status: DelegationJob["status"]): boolean {
  return status === "accepted" || status === "cancelled";
}

/** Rows under one chat before the rest fold into "N more". */
export const MAX_DELEGATION_CHILDREN = 3;

export type DelegationChildren = {
  shown: DelegationJob[];
  hidden: DelegationJob[];
};

/**
 * Jobs grouped by the chat that created them: unsettled first, newest first
 * within each, capped so a chat with a long history stays one glance tall.
 */
export function delegationChildrenByThread(
  jobs: ReadonlyArray<DelegationJob>,
  max = MAX_DELEGATION_CHILDREN
): ReadonlyMap<string, DelegationChildren> {
  const byThread = new Map<string, DelegationJob[]>();
  for (const job of jobs) {
    if (!job.parentThreadId) continue;
    const list = byThread.get(job.parentThreadId) ?? [];
    list.push(job);
    byThread.set(job.parentThreadId, list);
  }
  const result = new Map<string, DelegationChildren>();
  for (const [threadId, list] of byThread) {
    list.sort(
      (left, right) =>
        Number(isSettledDelegation(left.status)) - Number(isSettledDelegation(right.status)) ||
        right.updatedAt - left.updatedAt
    );
    result.set(threadId, { shown: list.slice(0, max), hidden: list.slice(max) });
  }
  return result;
}
