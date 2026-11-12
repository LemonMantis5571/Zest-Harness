import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  agentLabel,
  delegationReport,
  formatDuration,
  jobProgress,
  jobSteps,
  stepDuration,
} from "./subagents.ts";
import type { DelegationJob, ToolPart } from "./types.ts";

function tool(over: Partial<ToolPart> & { id: string }): ToolPart {
  return { name: "delegate_external", status: "done", ...over };
}

describe("agentLabel", () => {
  it("names known workers and title-cases the rest", () => {
    assert.equal(agentLabel("claude"), "Claude Code");
    assert.equal(agentLabel("my_worker"), "My Worker");
  });
});

describe("durations", () => {
  it("formats seconds, minutes and hours", () => {
    assert.equal(formatDuration(48_000), "48s");
    assert.equal(formatDuration(192_000), "3m 12s");
    assert.equal(formatDuration(3_840_000), "1h 4m");
  });

  it("counts to now while running and is absent when never timed", () => {
    assert.equal(stepDuration(1_000, undefined, 6_000), "5s");
    assert.equal(stepDuration(undefined, undefined, 6_000), undefined);
  });
});

function job(over: Partial<DelegationJob> = {}): DelegationJob {
  return {
    jobId: "job-1",
    parentThreadId: "thread-1",
    projectRoot: ".",
    cardId: "card-1",
    title: "Add dark mode",
    objective: "Add a dark theme toggle.",
    lane: "product",
    scope: ["src"],
    context: [],
    dependsOn: [],
    agent: "codex",
    reviewerAgent: "codex",
    workerTarget: { kind: "externalAgent", agentId: "codex" },
    reviewerTarget: { kind: "sameAsWorker" },
    resolvedWorkerTarget: null,
    resolvedReviewerTarget: null,
    approved: true,
    origin: null,
    attempts: [],
    attempt: 1,
    status: "worker_running",
    changedFiles: [],
    changedFileCount: 0,
    acceptanceChecks: [],
    reviewerFindings: [],
    createdAt: 0,
    updatedAt: 0,
    ...over,
  } as DelegationJob;
}

describe("feature card jobs", () => {
  it("names where the job is", () => {
    assert.deepEqual(jobProgress(job()), { status: "running", label: "Worker running" });
    assert.deepEqual(jobProgress(job({ status: "ready_to_apply" })), {
      status: undefined,
      label: "Ready to apply",
    });
    assert.equal(jobProgress(job({ status: "failed" })).status, "error");
  });

  it("lists the worker, the review and the checks as steps", () => {
    const steps = jobSteps(
      job({
        status: "review_running",
        attempts: [
          {
            attemptId: "w1",
            role: "worker",
            agent: "codex",
            target: null,
            usage: null,
            resolvedTarget: null,
            startedAt: 1_000n,
            finishedAt: 61_000n,
          },
          {
            attemptId: "r1",
            role: "reviewer",
            agent: "claude",
            target: null,
            usage: null,
            resolvedTarget: null,
            startedAt: 61_000n,
            finishedAt: null,
          },
        ],
        acceptanceChecks: [
          { command: "npm test", status: "passed", output: "" },
          { command: "npm run lint", status: "failed", output: "1 error" },
        ],
      }),
      91_000
    );
    assert.deepEqual(
      steps.map((step) => [step.title, step.status, step.description]),
      [
        ["Approved the card", undefined, undefined],
        ["Ran Codex", undefined, "1m"],
        ["Reviewing with Claude Code", "running", "30s"],
        ["Passed npm test", undefined, undefined],
        ["Failed npm run lint", "error", undefined],
      ]
    );
    assert.equal(steps[4].detail, "1 error");
  });
});

describe("delegationReport", () => {
  it("drops the provenance line the badge already shows", () => {
    assert.equal(
      delegationReport(tool({ id: "w", summary: "[codex · gpt-5] Found the bug in the cache key." })),
      "Found the bug in the cache key."
    );
  });

  it("has nothing to report for the old provenance-only summary", () => {
    assert.equal(
      delegationReport(tool({ id: "w", summary: "Delegated to codex · gpt-5" })),
      undefined
    );
  });
});
