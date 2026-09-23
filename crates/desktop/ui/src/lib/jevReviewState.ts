import type { JevQuickReview } from "./types.ts";

export function shouldAutoCheckChange(lastTargetId: string, currentTargetId: string) {
  return currentTargetId.length > 0 && lastTargetId !== currentTargetId;
}

export function reviewForSnapshot(review: JevQuickReview | null, targetId: string, reviewedInput: string, currentInput: string): JevQuickReview | null {
  if (!review || review.targetId !== targetId) return null;
  return reviewedInput === currentInput ? review : { ...review, status: "stale" };
}
