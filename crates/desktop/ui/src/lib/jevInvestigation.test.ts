import test from "node:test";
import assert from "node:assert/strict";
import { changeInvestigationPrompt, planInvestigationPrompt } from "./jevInvestigation.ts";
import type { JevQuickReview } from "./types.ts";

const review: JevQuickReview = {
  targetId: "thread:change",
  contentHash: "hash",
  kind: "changes",
  status: "attention",
  checks: [
    { id: "alignment", label: "Task alignment", outcome: "concern", probability: 0.94 },
    { id: "scope", label: "Change scope", outcome: "clear", probability: 0.95 },
  ],
  model: "~typesafe/jev-latest",
  usage: null,
  sourceExcerpt: "request",
  detail: null,
  reviewedAtMs: 0,
};

test("investigation hands the exact plan and request to the normal reviewer", () => {
  const prompt = planInvestigationPrompt(review, "Keep data local", "Build a local cache");
  assert.match(prompt, /Request:\nKeep data local\n\nPlan:\nBuild a local cache/);
  assert.match(prompt, /Task alignment/);
  assert.doesNotMatch(prompt, /Change scope/);
});

test("investigation hands the exact diff and test claim to the normal reviewer", () => {
  const prompt = changeInvestigationPrompt(review, "Fix crash", "Tests passed", "+guardNull()");
  assert.match(prompt, /Task:\nFix crash\n\nReported result:\nTests passed\n\nDiff:\n\+guardNull\(\)/);
  assert.match(prompt, /explain concrete issues with evidence/);
});
