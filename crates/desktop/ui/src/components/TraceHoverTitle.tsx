import { useEffect, useRef, useState, type ReactNode } from "react";

import { TraceTitle } from "@/components/ui/trace";
import { cn } from "@/lib/utils";

type Tip = { left: number; top: number; below: boolean };

/**
 * A `TraceTitle` that shows its full text on hover when it is truncated.
 *
 * The card stays in the tree (no portal: the desktop WebView drops portalled
 * popovers), but it is `position: fixed` at the title's measured rectangle.
 * An absolutely placed card would be cut off by the `overflow: hidden` every
 * collapsible row needs for its height animation.
 */
export function TraceHoverTitle({
  text,
  className,
  children,
}: {
  /** The full title, shown in the card and read by assistive tech. */
  text: string;
  className?: string;
  /** Styled title content; defaults to `text`. */
  children?: ReactNode;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const [tip, setTip] = useState<Tip | null>(null);

  useEffect(() => {
    if (!tip) return;
    // A fixed card does not follow the row when the transcript scrolls.
    const hide = () => setTip(null);
    window.addEventListener("scroll", hide, true);
    return () => window.removeEventListener("scroll", hide, true);
  }, [tip]);

  const show = () => {
    const element = ref.current;
    if (!element || element.scrollWidth <= element.clientWidth + 1) return;
    const rect = element.getBoundingClientRect();
    const below = rect.top < 80;
    setTip({ left: rect.left, top: below ? rect.bottom + 6 : rect.top - 6, below });
  };

  return (
    <>
      <TraceTitle
        ref={ref}
        className={className}
        onMouseEnter={show}
        onMouseLeave={() => setTip(null)}
      >
        {children ?? text}
      </TraceTitle>
      {tip ? (
        <span
          role="tooltip"
          style={{ left: tip.left, top: tip.top }}
          className={cn(
            "pointer-events-none fixed z-50 w-max max-w-[min(28rem,80vw)] rounded-md border border-border/80 bg-popover px-2.5 py-1.5 text-left text-[11px] leading-snug whitespace-pre-wrap break-words text-popover-foreground shadow-lg",
            !tip.below && "-translate-y-full"
          )}
        >
          {text}
        </span>
      ) : null}
    </>
  );
}

/** The current time, ticking each second while `active`. */
export function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}
