import { GitBranchIcon, GitPullRequestIcon, XIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { GitContext, WorkspaceChange } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  /** Folder name of the open project, shown as the left-hand label. */
  projectLabel: string;
  branch: string | null;
  workspaceChange: WorkspaceChange | null;
  gitContext: GitContext | null;
  /** Opens the branch diff. The whole strip is the affordance. */
  onOpen: () => void;
  onDismiss: () => void;
};

/**
 * Shows the current branch change count above the composer.
 *
 * The bar shares the composer's overlay so it does not push the transcript down.
 * It appears only when the workspace has changes.
 */
export function BranchChangesBar({
  projectLabel,
  branch,
  workspaceChange,
  gitContext,
  onOpen,
  onDismiss,
}: Props) {
  const additions = workspaceChange?.additions ?? gitContext?.additions ?? 0;
  const deletions = workspaceChange?.deletions ?? gitContext?.deletions ?? 0;
  const fileCount =
    workspaceChange?.changedFiles.length ?? gitContext?.changedFiles ?? 0;
  const pullRequest = gitContext?.pullRequest;
  const hasLineChanges = additions > 0 || deletions > 0;

  return (
    <div className="mb-2">
      <div
        className={cn(
          "group/branchbar flex items-center gap-2 rounded-xl border border-border/80 py-1.5 pl-2.5 pr-1.5",
          // Matches the queued-messages block directly below it: both float in
          // the composer's overlay, so both need the same translucent card.
          "bg-[color-mix(in_srgb,var(--card)_92%,transparent)] shadow-lg backdrop-blur-xl",
          "animate-in fade-in slide-in-from-bottom-1 duration-200"
        )}
      >
        <button
          type="button"
          onClick={onOpen}
          title={`Review ${fileCount} changed ${fileCount === 1 ? "file" : "files"} on ${
            branch ?? "this branch"
          }`}
          aria-label={
            hasLineChanges
              ? `Show branch diff: ${fileCount} ${
                  fileCount === 1 ? "file" : "files"
                } changed, ${additions} added, ${deletions} removed`
              : `Show branch diff: ${fileCount} ${
                  fileCount === 1 ? "file" : "files"
                } changed`
          }
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md px-1 py-0.5 text-left outline-none hover:bg-secondary/60 focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <span className="truncate text-[12px] font-medium">{projectLabel}</span>
          {branch ? (
            <span className="inline-flex min-w-0 items-center gap-1 text-[11px] text-muted-foreground">
              <GitBranchIcon className="size-3 shrink-0 opacity-70" aria-hidden="true" />
              <span className="truncate">{branch}</span>
            </span>
          ) : null}
        </button>

        {hasLineChanges ? (
          <span
            className="shrink-0 rounded-md bg-secondary/70 px-1.5 py-0.5 text-[11px] tabular-nums"
            title={`${additions} added, ${deletions} removed across ${fileCount} ${
              fileCount === 1 ? "file" : "files"
            }`}
          >
            <span className="text-primary">+{additions}</span>{" "}
            <span className="text-destructive">−{deletions}</span>
          </span>
        ) : null}

        {/* Zest can read an existing pull request but cannot open one, so this
            slot links to the PR when there is one and stays empty otherwise
            rather than offering a button that would do nothing. */}
        {pullRequest ? (
          <a
            href={pullRequest.url}
            target="_blank"
            rel="noreferrer"
            title={`${pullRequest.title} (${pullRequest.state.toLowerCase()})`}
            className="inline-flex shrink-0 items-center gap-1 rounded-md border border-border/70 px-2 py-0.5 text-[11px] text-muted-foreground hover:bg-secondary hover:text-foreground"
          >
            <GitPullRequestIcon className="size-3 opacity-80" aria-hidden="true" />
            <span>#{pullRequest.number}</span>
          </a>
        ) : null}

        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          title="Hide branch changes"
          aria-label="Hide branch changes"
          onClick={onDismiss}
        >
          <XIcon />
        </Button>
      </div>
    </div>
  );
}
