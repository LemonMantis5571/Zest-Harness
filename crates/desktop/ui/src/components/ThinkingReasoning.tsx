/**
 * One block of the model's thinking, as the AI Elements Reasoning pattern
 * draws it: a line that reads "Thinking" and stays open while the block
 * streams, then reads "Thought for Ns" and folds. A turn that thinks again
 * after a tool call gets a second line in its place.
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { BrainIcon } from "lucide-react";

import {
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { thinkingTraceRows, thoughtForLabel } from "@/lib/thinkingSummary";

type Props = {
  text: string;
  /** This block is still streaming. */
  live: boolean;
};

/**
 * Seconds since `active` last became true, frozen when it goes false.
 *
 * Deliberately not derived from the message: thinking text carries no
 * timestamps, so the only honest duration is the one this window measured.
 * Null until something has actually been watched.
 */
function useElapsedSeconds(active: boolean): number | null {
  const [seconds, setSeconds] = useState<number | null>(null);

  useEffect(() => {
    if (!active) return;
    const start = Date.now();
    setSeconds(0);
    const timer = window.setInterval(() => {
      setSeconds(Math.round((Date.now() - start) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [active]);

  return seconds;
}

export function ThinkingReasoning({ text, live }: Props) {
  const rows = useMemo(() => thinkingTraceRows(text), [text]);
  const [open, setOpen] = useState<boolean>();
  const elapsed = useElapsedSeconds(live);
  const viewport = useRef<HTMLDivElement>(null);

  // A live block is read from its newest line, like a terminal.
  useLayoutEffect(() => {
    if (live && viewport.current) {
      viewport.current.scrollTop = viewport.current.scrollHeight;
    }
  }, [live, rows]);

  const line = (
    <TraceLine status={live ? "running" : undefined}>
      <TraceIcon>
        <BrainIcon />
      </TraceIcon>
      <TraceTitle>{live ? "Thinking" : thoughtForLabel(elapsed, text)}</TraceTitle>
    </TraceLine>
  );

  // Nothing to open before the model has written anything.
  if (rows.length === 0) return line;

  return (
    <TraceCollapsible open={open ?? live} onOpenChange={setOpen}>
      <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <div
          ref={viewport}
          className="scroll-fade-y flex max-h-60 flex-col gap-1.5 overflow-y-auto text-[13px]"
        >
          {rows.map((row, index) => (
            <p key={`${index}-${row.primary.slice(0, 24)}`}>
              {row.kind === "step" ? (
                <span className="font-medium text-foreground/80">{row.primary}</span>
              ) : (
                row.primary
              )}
              {row.secondary ? <> — {row.secondary}</> : null}
            </p>
          ))}
        </div>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}
