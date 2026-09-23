import type { JevQuickReview } from "./types.ts";

function checkNames(review: JevQuickReview) {
  return review.checks
    .filter((check) => check.outcome !== "clear")
    .map((check) => check.label)
    .join(", ");
}

export function planInvestigationPrompt(review: JevQuickReview, request: string, plan: string) {
  return `Review this finished plan. Jev marked these checks for investigation: ${checkNames(review)}. Check the request against the exact plan below. Explain concrete issues with evidence and suggest a correction if needed.\n\nRequest:\n${request}\n\nPlan:\n${plan}`;
}

export function changeInvestigationPrompt(review: JevQuickReview, objective: string, claimedSummary: string, diff: string) {
  return `Review this exact workspace change. Jev marked these checks for investigation: ${checkNames(review)}. Inspect the diff against the task, explain concrete issues with evidence, and suggest a correction if needed.\n\nTask:\n${objective}\n\nReported result:\n${claimedSummary}\n\nDiff:\n${diff}`;
}
