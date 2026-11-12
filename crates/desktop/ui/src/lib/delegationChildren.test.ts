import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MAX_DELEGATION_CHILDREN,
  delegationChildStatus,
  delegationChildrenByThread,
  isSettledDelegation,
} from "./delegationChildren.ts";
import type { DelegationJob } from "./types.ts";

function job(
  jobId: string,
  parentThreadId: string,
  status: DelegationJob["status"],
  updatedAt: number
): DelegationJob {
  return { jobId, parentThreadId, status, updatedAt, title: jobId } as DelegationJob;
}

const ids = (jobs: DelegationJob[] | undefined) => (jobs ?? []).map((entry) => entry.jobId);

describe("delegationChildStatus", () => {
  it("says every state in words, with the ones waiting on you marked loudest", () => {
    const statuses: DelegationJob["status"][] = [
      "planned",
      "awaiting_approval",
      "queued",
      "worker_running",
      "review_running",
      "ready_to_apply",
      "accepted",
      "changes_requested",
      "blocked",
      "failed",
      "cancelled",
      "apply_conflict",
    ];
    for (const status of statuses) {
      assert.ok(delegationChildStatus(status).label.length > 0, status);
    }
    assert.deepEqual(delegationChildStatus("awaiting_approval"), {
      label: "Needs approval",
      tone: "attention",
    });
    assert.equal(delegationChildStatus("worker_running").tone, "running");
    assert.equal(delegationChildStatus("review_running").label, "Reviewing");
    assert.equal(delegationChildStatus("ready_to_apply").tone, "ready");
    assert.equal(delegationChildStatus("accepted").label, "Applied");
  });

  it("treats only applied and cancelled jobs as settled", () => {
    assert.ok(isSettledDelegation("accepted"));
    assert.ok(isSettledDelegation("cancelled"));
    assert.ok(!isSettledDelegation("failed"), "a failed job can still be retried");
    assert.ok(!isSettledDelegation("ready_to_apply"));
  });
});

describe("delegationChildrenByThread", () => {
  it("files each job under the chat that created it", () => {
    const children = delegationChildrenByThread([
      job("a", "chat-1", "queued", 1),
      job("b", "chat-2", "queued", 2),
      job("c", "chat-1", "queued", 3),
    ]);
    assert.deepEqual(ids(children.get("chat-1")?.shown), ["c", "a"]);
    assert.deepEqual(ids(children.get("chat-2")?.shown), ["b"]);
    assert.equal(children.get("chat-3"), undefined);
  });

  it("puts unsettled jobs ahead of settled ones, newest first in each", () => {
    const children = delegationChildrenByThread([
      job("applied-new", "chat", "accepted", 9),
      job("running-old", "chat", "worker_running", 1),
      job("ready-new", "chat", "ready_to_apply", 5),
      job("cancelled-old", "chat", "cancelled", 2),
    ], 10);
    assert.deepEqual(ids(children.get("chat")?.shown), [
      "ready-new",
      "running-old",
      "applied-new",
      "cancelled-old",
    ]);
  });

  it("folds jobs past the cap into the hidden list", () => {
    const jobs = Array.from({ length: MAX_DELEGATION_CHILDREN + 2 }, (_, index) =>
      job(`j${index}`, "chat", "queued", index)
    );
    const children = delegationChildrenByThread(jobs).get("chat");
    assert.equal(children?.shown.length, MAX_DELEGATION_CHILDREN);
    assert.deepEqual(ids(children?.hidden), ["j1", "j0"]);
  });

  it("skips a job with no parent chat", () => {
    assert.equal(delegationChildrenByThread([job("orphan", "", "queued", 1)]).size, 0);
  });
});
