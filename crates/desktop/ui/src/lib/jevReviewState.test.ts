import test from "node:test";
import assert from "node:assert/strict";
import { reviewForSnapshot, shouldAutoCheckChange } from "./jevReviewState.ts";
import type { JevQuickReview } from "./types.ts";

test("auto checks run once per diff target", () => {
  assert.equal(shouldAutoCheckChange("", "thread:diff-a"), true);
  assert.equal(shouldAutoCheckChange("thread:diff-a", "thread:diff-a"), false);
  assert.equal(shouldAutoCheckChange("thread:diff-a", "thread:diff-b"), true);
});

test("a result from changed evidence is stale and cannot be applied to another target", () => {
  const review: JevQuickReview = {
    targetId: "thread:diff-a", contentHash: "hash", kind: "changes", status: "attention",
    checks: [], model: "jev", usage: null, sourceExcerpt: "request", detail: null, reviewedAtMs: 0,
  };
  assert.equal(reviewForSnapshot(review, "thread:diff-a", "old", "new")?.status, "stale");
  assert.equal(reviewForSnapshot(review, "thread:diff-b", "old", "old"), null);
  assert.equal(reviewForSnapshot(review, "thread:diff-a", "old", "old")?.status, "attention");
});
