import { useState } from "react";
import { ChevronRightIcon, TriangleAlertIcon, XIcon } from "lucide-react";

import { ToolCallRow } from "@/components/ToolCallRow";
import { countDiffLines, type ToolRunSummary } from "@/lib/toolRuns";
import type { ApprovalChoice, ToolPart } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  tools: ToolPart[];
  summary: ToolRunSummary;
  onResolveApproval: (
    approvalId: string,
    decision: ApprovalChoice
  ) => Promise<void>;
  onOpenDiff?: (path: string, diff: string) => void;
};

/**
 * One line standing in for a stretch of finished tool calls.
 *
 * Inspection-only runs collapse by default, but edit-containing runs stay open
 * so their diffs remain immediately reviewable. Failures are always stated on
 * the summary line because a fold that hides an error is worse than the rows.
 */
export function ToolRunGroup({
  tools,
  summary,
  onResolveApproval,
  onOpenDiff,
}: Props) {
  const hasChanges = summary.added > 0 || summary.removed > 0;
  const totalFailure = summary.errors > 0 && summary.errors === tools.length;
  const partialFailure = summary.errors > 0 && !totalFailure;
  // Completed edits contain the user's most important review surface. Keep
  // those cards visible; inspection-only runs can still collapse to one line.
  const [open, setOpen] = useState(hasChanges);

  if (open) {
    return (
      <div className="flex w-full max-w-full flex-col gap-0.5">
        <button
          type="button"
          aria-expanded
          onClick={() => setOpen(false)}
          className="group/run-header flex w-fit max-w-full items-center gap-1.5 rounded-md px-1 py-1 text-left text-xs text-muted-foreground outline-none transition-colors hover:bg-accent/70 focus-visible:ring-2 focus-visible:ring-ring/40"
        >
          <ChevronRightIcon className="size-3 shrink-0 rotate-90 text-muted-foreground/60" />
          <span className="min-w-0 truncate font-medium">{summary.label}</span>
        </button>
        <div className="flex flex-col gap-0.5">
          {tools.map((tool) => (
            <ToolCallRow
              key={tool.id}
              tool={tool}
              onResolveApproval={onResolveApproval}
              onOpenDiff={onOpenDiff}
            />
          ))}
        </div>
        <DiffChips tools={tools} onOpenDiff={onOpenDiff} />
      </div>
    );
  }

  return (
    <button
      type="button"
      aria-expanded={false}
      onClick={() => setOpen(true)}
      className={cn(
        "group/run flex w-fit max-w-full items-center gap-1.5 rounded-md px-1 py-1 text-left outline-none transition-colors",
        "hover:bg-accent/70 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40"
      )}
    >
      <span className="grid size-4 shrink-0 place-items-center">
        {totalFailure ? (
          <XIcon className="size-3 text-destructive" />
        ) : partialFailure ? (
          <TriangleAlertIcon className="size-3 text-amber-400" />
        ) : (
          <ChevronRightIcon className="size-3 text-muted-foreground/60" />
        )}
      </span>
      <span className="min-w-0 truncate text-xs font-medium text-muted-foreground">
        {summary.label}
      </span>
      {hasChanges ? (
        <span className="shrink-0 font-mono text-[11px]">
          {summary.added > 0 ? (
            <span className="text-primary">+{summary.added}</span>
          ) : null}
          {summary.added > 0 && summary.removed > 0 ? " " : null}
          {summary.removed > 0 ? (
            <span className="text-destructive">-{summary.removed}</span>
          ) : null}
        </span>
      ) : null}
      {summary.errors > 0 ? (
        <span
          className={cn(
            "shrink-0 text-[11px]",
            totalFailure ? "text-destructive" : "text-amber-400"
          )}
        >
          {summary.errors} failed
        </span>
      ) : null}
      <span className="min-w-0 flex-1" />
      <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground/50" />
    </button>
  );
}

function DiffChips({
  tools,
  onOpenDiff,
}: {
  tools: ToolPart[];
  onOpenDiff?: (path: string, diff: string) => void;
}) {
  const diffs = tools.flatMap((tool) => {
    const diff = tool.diff?.trim();
    if (!diff) return [];
    const counts = countDiffLines(tool.diff);
    return [
      {
        id: tool.id,
        path: tool.path || tool.name,
        diff: tool.diff as string,
        ...counts,
      },
    ];
  });

  if (diffs.length === 0) return null;

  return (
    <div className="mt-1.5 flex max-w-full flex-wrap gap-1.5 border-t border-border/60 px-1 pt-2">
      {diffs.map((item) => (
        <button
          key={item.id}
          type="button"
          disabled={!onOpenDiff}
          onClick={() => onOpenDiff?.(item.path, item.diff)}
          className={cn(
            "inline-flex h-7 max-w-full items-center gap-1.5 rounded-sm border border-border/60 bg-card/70 px-2 font-mono text-[11px] text-muted-foreground outline-none transition-colors",
            onOpenDiff
              ? "hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
              : "cursor-default opacity-70"
          )}
        >
          <span className="min-w-0 truncate">{item.path}</span>
          {item.added > 0 ? (
            <span className="shrink-0 text-primary tabular-nums">+{item.added}</span>
          ) : null}
          {item.removed > 0 ? (
            <span className="shrink-0 text-destructive tabular-nums">
              -{item.removed}
            </span>
          ) : null}
        </button>
      ))}
    </div>
  );
}
