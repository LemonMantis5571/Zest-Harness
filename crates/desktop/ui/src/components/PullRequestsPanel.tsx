import { useEffect, useMemo, useState } from "react";
import { ExternalLinkIcon, GitPullRequestIcon, RefreshCwIcon, SearchIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { handlePullRequestClick, pullRequestAnchorProps } from "@/lib/pullRequestLink";
import type { ProjectChats, ThreadSummary } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = { onOpen: (project: ProjectChats, thread: ThreadSummary) => void };

export function PullRequestsPanel({ onOpen }: Props) {
  const [projects, setProjects] = useState<ProjectChats[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getBackend().listChatProjects().then((next) => {
      if (!cancelled) { setProjects(next); setError(null); }
    }).catch((cause: unknown) => {
      if (!cancelled) setError(cause instanceof Error ? cause.message : "Could not load pull requests.");
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [revision]);

  const requests = useMemo(() => {
    const seen = new Set<string>();
    return projects.flatMap((project) => project.threads.map((thread) => ({ project, thread })))
      .sort((a, b) => b.thread.updatedAt - a.thread.updatedAt)
      .flatMap(({ project, thread }) => {
        const pr = thread.gitContext?.pullRequest;
        if (!pr || !project.path || seen.has(pr.url)) return [];
        seen.add(pr.url);
        return [{ project, thread, pr }];
      });
  }, [projects]);
  const visible = requests.filter(({ project, thread, pr }) => {
    const state = pr.state.toLowerCase();
    return (filter === "all" || (filter === "draft" ? pr.isDraft && state === "open" : state === filter))
      && `${project.name} ${pr.title} ${pr.number} ${thread.gitContext?.branch ?? ""}`.toLowerCase().includes(query.trim().toLowerCase());
  });

  return (
    <section aria-label="Pull requests" className="min-h-0 flex-1 overflow-y-auto bg-[var(--canvas-solid)] p-5 sm:p-8">
      <div className="mx-auto flex max-w-4xl flex-col gap-5">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h1 className="m-0 text-xl font-semibold">Pull requests</h1>
            <p className="mt-1.5 text-sm text-muted-foreground">Review pull requests linked to your chats.</p>
          </div>
          <Button variant="outline" size="sm" disabled={loading} onClick={() => { setLoading(true); setRevision((value) => value + 1); }}>
            <RefreshCwIcon aria-hidden="true" />{loading ? "Loading…" : "Refresh"}
          </Button>
        </div>
        <div className="flex flex-col gap-3">
          <div className="relative">
            <SearchIcon className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <input aria-label="Search pull requests" placeholder="Search title, project, branch, or number" value={query} onChange={(event) => setQuery(event.target.value)} className="h-10 w-full min-w-0 rounded-lg border border-border bg-background py-2 pl-9 pr-3 text-sm outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/25" />
          </div>
          <div role="group" aria-label="Pull request status" className="flex flex-wrap items-center gap-1 border-b border-border/60 pb-3">
            {([['all', 'All'], ['open', 'Open'], ['draft', 'Draft'], ['merged', 'Merged'], ['closed', 'Closed']] as const).map(([value, label]) => (
              <button key={value} type="button" aria-pressed={filter === value} onClick={() => setFilter(value)} className={cn("rounded-md px-3 py-1.5 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50", filter === value ? "bg-secondary text-foreground" : "text-muted-foreground hover:bg-secondary/50 hover:text-foreground")}>
                {label}
              </button>
            ))}
            <span className="ml-auto px-2 text-xs tabular-nums text-muted-foreground">{visible.length} {visible.length === 1 ? "pull request" : "pull requests"}</span>
          </div>
        </div>
        {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
        {loading && !requests.length ? <p role="status" className="text-sm text-muted-foreground">Loading pull requests…</p> : !visible.length ? (
          <div className="flex min-h-64 flex-col items-center justify-center rounded-xl border border-border/60 bg-card/40 px-6 py-12 text-center">
            <div className="mb-4 rounded-xl border border-border/60 bg-secondary/50 p-3"><GitPullRequestIcon className="size-5 text-muted-foreground" aria-hidden="true" /></div>
            <p className="text-sm font-medium">{requests.length ? "No matching pull requests" : "No linked pull requests yet"}</p>
            <p className="mt-2 max-w-sm text-sm leading-relaxed text-muted-foreground">{requests.length ? "Try another search or status." : "Open a project chat on a branch with a pull request. It will appear here once Zest detects it."}</p>
          </div>
        ) : (
          <ul className="divide-y divide-border overflow-hidden rounded-xl border border-border bg-card">
            {visible.map(({ project, thread, pr }) => (
              <li key={pr.url} className="flex items-center gap-4 p-4">
                <GitPullRequestIcon className="size-5 shrink-0 text-primary" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <a {...pullRequestAnchorProps(pr.url)} onClick={(event) => handlePullRequestClick(event, () => onOpen(project, thread))} className="block truncate text-sm font-medium hover:underline">{pr.title || `Pull request #${pr.number}`}</a>
                  <p className="mt-1 truncate text-xs text-muted-foreground">{project.name} · #{pr.number} · {pr.isDraft && pr.state.toLowerCase() === "open" ? "Draft" : pr.state.toLowerCase()} · {thread.gitContext?.branch}</p>
                  <p className="mt-1 text-xs text-muted-foreground"><span className="text-green-500">+{pr.additions}</span> <span className="text-red-400">−{pr.deletions}</span> · {pr.changedFiles} files</p>
                </div>
                <a href={pr.url} target="_blank" rel="noreferrer" aria-label={`Open pull request #${pr.number} on GitHub`} title="Open on GitHub" className="rounded-md p-2 text-muted-foreground hover:bg-secondary hover:text-foreground"><ExternalLinkIcon className="size-4" aria-hidden="true" /></a>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
