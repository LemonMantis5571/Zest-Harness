import { ArrowDownIcon, ArrowUpIcon } from "lucide-react";

import type { WorktreeChatStatus } from "@/lib/types";
import { cn } from "@/lib/utils";

function count(value: number, noun: string) {
  return `${value} ${noun}${value === 1 ? "" : "s"}`;
}

/** Spoken and tooltip text for a status. Empty when there is nothing to report. */
export function worktreeStatusLabel(status: WorktreeChatStatus): string {
  const parts: string[] = [];
  if (status.ahead > 0) parts.push(`${count(status.ahead, "commit")} ahead`);
  if (status.behind > 0) parts.push(`${count(status.behind, "commit")} behind`);
  if (status.dirty > 0) parts.push(count(status.dirty, "uncommitted change"));
  return parts.join(", ");
}

/**
 * Compact Git counts for a worktree chat row: commits ahead of and behind its
 * source branch, and uncommitted changes. A clean, level checkout shows nothing.
 */
export function WorktreeStatusBadge({
  status,
  className,
}: {
  status: WorktreeChatStatus | undefined;
  className?: string;
}) {
  const label = status ? worktreeStatusLabel(status) : "";
  if (!status || !label) return null;
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={cn(
        "flex shrink-0 items-center gap-1.5 text-[10px] leading-none tabular-nums text-muted-foreground",
        className
      )}
    >
      {status.ahead > 0 ? (
        <span className="flex items-center" aria-hidden="true">
          <ArrowUpIcon className="size-2.5" />
          {status.ahead}
        </span>
      ) : null}
      {status.behind > 0 ? (
        <span className="flex items-center" aria-hidden="true">
          <ArrowDownIcon className="size-2.5" />
          {status.behind}
        </span>
      ) : null}
      {status.dirty > 0 ? (
        <span className="flex items-center gap-0.5 text-amber-400" aria-hidden="true">
          <span className="size-1.5 rounded-full bg-current" />
          {status.dirty}
        </span>
      ) : null}
    </span>
  );
}
