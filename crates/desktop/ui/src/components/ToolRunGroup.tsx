import { useState } from "react";
import { ListChecksIcon } from "lucide-react";

import { ToolLine, toolTitleText } from "@/components/ToolCallRow";
import { TraceHoverTitle } from "@/components/TraceHoverTitle";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
} from "@/components/ui/trace";
import { countDiffLines, type ToolRunSummary } from "@/lib/toolRuns";
import type { ToolPart } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  tools: ToolPart[];
  summary: ToolRunSummary;
  onOpenDiff?: (path: string, diff: string) => void;
};

/**
 * A stretch of tool calls as one Chain of Thought line.
 *
 * The line says what the run did ("Ran 3 commands, edited 2 files") and opens
 * into the calls. It opens on its own while a call runs and folds when the run
 * settles; once the user opens or closes it, their choice wins.
 */
export function ToolRunGroup({ tools, summary, onOpenDiff }: Props) {
  const [open, setOpen] = useState<boolean>();
  const running = tools.find((tool) => tool.status === "running");
  const failed = tools.filter((tool) => tool.status === "error").length;
  const hasChanges = summary.added > 0 || summary.removed > 0;
  // While a call runs, the line names it; the run's tally is for afterwards.
  const title = running
    ? toolTitleText(running)
    : failed > 0
      ? `${summary.label}, ${failed} failed`
      : summary.label;

  return (
    <TraceCollapsible open={open ?? Boolean(running)} onOpenChange={setOpen}>
      <TraceCollapsibleTrigger>
        <TraceLine status={running ? "running" : failed > 0 ? "error" : undefined}>
          <TraceIcon>
            <ListChecksIcon />
          </TraceIcon>
          <TraceHoverTitle text={title} />
          {hasChanges ? (
            <TraceDescription className="font-mono text-[11px]">
              {summary.added > 0 ? <span className="text-primary">+{summary.added}</span> : null}
              {summary.added > 0 && summary.removed > 0 ? " " : null}
              {summary.removed > 0 ? (
                <span className="text-destructive">-{summary.removed}</span>
              ) : null}
            </TraceDescription>
          ) : null}
        </TraceLine>
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent keepMounted className="flex flex-col gap-2">
        <Trace>
          {tools.map((tool) => (
            <ToolLine key={tool.id} tool={tool} onOpenDiff={onOpenDiff} />
          ))}
        </Trace>
        <DiffChips tools={tools} onOpenDiff={onOpenDiff} />
      </TraceCollapsibleContent>
    </TraceCollapsible>
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
    <div className="flex max-w-full flex-wrap gap-1.5 border-t border-border/60 pt-2">
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
