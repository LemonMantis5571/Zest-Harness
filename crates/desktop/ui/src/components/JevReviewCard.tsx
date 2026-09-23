import { useState } from "react";
import type { JevQuickReview } from "@/lib/types";
import { Button } from "@/components/ui/button";

export function JevReviewCard({ review, onInvestigate, onRetry }: {
  review: JevQuickReview;
  onInvestigate?: () => void;
  onRetry?: () => Promise<void>;
}) {
  const [rerunning, setRerunning] = useState(false);
  const canRetry = !["clear", "skipped"].includes(review.status) ? onRetry : undefined;
  const flagged = review.status === "stale" ? [] : review.checks.filter((check) => check.outcome === "concern");
  const uncertain = review.status === "stale" ? [] : review.checks.filter((check) => check.outcome === "inconclusive");
  const title = review.status === "attention"
    ? `Jev suggests a closer look at ${flagged.map((check) => check.label.toLowerCase()).join(", ")}`
    : review.status === "inconclusive"
      ? "Jev could not settle every check"
      : review.status === "clear"
        ? "Jev quick checks found no concern"
        : review.status === "stale"
          ? "Jev check is stale for this snapshot"
        : review.status === "skipped"
          ? "Jev check skipped"
          : "Jev check unavailable";

  return (
    <div className="rounded-md border border-border/60 bg-secondary/30 px-2.5 py-2 text-[11px]">
      <div className="font-medium text-foreground">{title}</div>
      {flagged.length > 0 || uncertain.length > 0 ? (
        <div className="mt-1 text-muted-foreground">
          {[...flagged, ...uncertain].map((check) => (
            <div key={check.id}>{check.label}: {check.outcome === "concern" ? "possible issue" : "inconclusive"}</div>
          ))}
        </div>
      ) : null}
      {(flagged.length > 0 || uncertain.length > 0) && review.sourceExcerpt ? (
        <div className="mt-1 line-clamp-2 text-muted-foreground" title={review.sourceExcerpt}>
          Request: {review.sourceExcerpt}
        </div>
      ) : null}
      {review.detail ? <div className="mt-1 text-muted-foreground">{review.detail}</div> : null}
      {canRetry || ((review.status === "attention" || review.status === "inconclusive") && onInvestigate) ? (
        <div className="mt-2 flex gap-2">
          {(review.status === "attention" || review.status === "inconclusive") && onInvestigate ? (
            <Button type="button" size="sm" variant="outline" onClick={onInvestigate}>Ask Zest to investigate</Button>
          ) : null}
          {canRetry ? <Button type="button" size="sm" variant="ghost" disabled={rerunning} onClick={() => {
            setRerunning(true);
            void canRetry().finally(() => setRerunning(false));
          }}>{rerunning ? "Checking…" : "Run again"}</Button> : null}
        </div>
      ) : null}
    </div>
  );
}
