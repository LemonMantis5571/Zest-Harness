import { useEffect, useRef, useState } from "react";

import { Marker, MarkerContent } from "@/components/ui/marker";
import { elapsedLabel } from "@/lib/threadActivity";
import { cn } from "@/lib/utils";

function WorkingDots({ className }: { className?: string }) {
  return (
    <span
      className={cn("inline-flex items-center gap-0.5", className)}
      aria-hidden="true"
    >
      <span className="size-1.5 rounded-full bg-foreground/70 animate-bounce [animation-delay:-0.32s]" />
      <span className="size-1.5 rounded-full bg-foreground/70 animate-bounce [animation-delay:-0.16s]" />
      <span className="size-1.5 rounded-full bg-foreground/70 animate-bounce" />
    </span>
  );
}

/**
 * In-transcript sign that a turn is still moving.
 *
 * Lives in the message list rather than a hover card: the clock and the motion
 * have to be visible without pointing at the sidebar.
 */
export function WorkingIndicator({
  startedAt,
  action,
}: {
  startedAt?: number;
  action?: string;
}) {
  const fallbackStart = useRef(startedAt ?? Date.now());
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (startedAt) fallbackStart.current = startedAt;
  }, [startedAt]);

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const elapsed = elapsedLabel(startedAt ?? fallbackStart.current, now);
  const label = ["Working", elapsed, action].filter(Boolean).join(" · ");

  return (
    <Marker role="status" aria-live="polite" aria-label={label} className="min-h-6">
      <MarkerContent className="flex min-w-0 items-center gap-2 text-xs">
        <WorkingDots />
        <span className="shimmer-text font-medium">Working</span>
        {elapsed ? (
          <span className="tabular-nums text-muted-foreground">{elapsed}</span>
        ) : null}
        {action ? (
          <span className="truncate text-muted-foreground">{action}</span>
        ) : null}
      </MarkerContent>
    </Marker>
  );
}
