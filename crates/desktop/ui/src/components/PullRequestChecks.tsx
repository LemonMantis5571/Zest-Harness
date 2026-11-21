import { useEffect, useState } from "react";
import { getBackend } from "@/lib/backend";
import { safeHttpUrl } from "@/lib/externalLinks";
import type { PullRequestStatus } from "@/lib/generated/PullRequestStatus";
import { visibleInterval } from "@/lib/visibleInterval";

const labels: Record<string, string> = {
  SUCCESS: "Passed", FAILURE: "Failed", ERROR: "Failed", TIMED_OUT: "Timed out",
  IN_PROGRESS: "Running", PENDING: "Pending", QUEUED: "Queued", CANCELLED: "Cancelled",
  SKIPPED: "Skipped", ACTION_REQUIRED: "Action required", OPEN: "Open", MERGED: "Merged", CLOSED: "Closed",
};
function label(state: string) {
  return labels[state] ?? state.toLowerCase().replaceAll("_", " ");
}

export function PullRequestChecks({ link }: { link: { number: number; url: string } }) {
  const [status, setStatus] = useState<PullRequestStatus | null>(null);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let disposed = false;
    let fetching = false;
    const refresh = async () => {
      if (fetching) return;
      fetching = true;
      try {
        const next = await getBackend().pullRequestStatus(link.number);
        if (next.url !== link.url) throw new Error("The active repository returned a different pull request.");
        if (!disposed) { setStatus(next); setError(""); }
      } catch (cause) {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        fetching = false;
      }
    };
    void refresh();
    const stop = visibleInterval(document, () => void refresh(), 30_000);
    return () => { disposed = true; stop(); };
  }, [link.number, link.url, revision]);

  return (
    <section aria-label="Pull request checks" className="border-t border-border/50 px-3 py-2 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span>{status ? label(status.state) : "Pull request"} · Checks</span>
        <div className="flex gap-3">
          <button type="button" onClick={() => setRevision((value) => value + 1)}>Refresh checks</button>
          <a href={safeHttpUrl(link.url) ?? undefined} target="_blank" rel="noreferrer" className="underline">Open on GitHub</a>
        </div>
      </div>
      {error ? <p role="status" className="mt-1 text-muted-foreground">{status ? "Last update failed. " : ""}{error}</p> : null}
      {!error && !status ? <p role="status">Loading checks…</p> : null}
      {status?.checks.length === 0 ? <p>No checks reported.</p> : null}
      {status && status.checks.length > 0 ? (
        <ul className="mt-1 max-h-32 overflow-auto">
          {status.checks.map((check, index) => {
            const url = safeHttpUrl(check.url);
            return <li key={index} className="flex items-baseline justify-between gap-2 py-0.5">
              {url ? <a href={url} target="_blank" rel="noreferrer" className="truncate underline">{check.name}</a> : <span className="truncate">{check.name}</span>}
              <span className="shrink-0">{label(check.status)}</span>
            </li>;
          })}
        </ul>
      ) : null}
    </section>
  );
}
