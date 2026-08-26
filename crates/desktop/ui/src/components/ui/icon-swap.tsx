import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * Cross-fade between two icon states in place.
 *
 * A hard icon swap gives no sign the click registered — the new glyph is simply
 * there, which reads as a repaint rather than a response. Fading and scaling
 * between them is the acknowledgement.
 *
 * Both children stay mounted and stacked, so this is pure CSS: no exit-animation
 * runtime, and the app-wide `prefers-reduced-motion` rule flattens it for free.
 * That is the whole reason not to reach for an animation library here.
 *
 * Idea from Amicro's `IconSwap` (MIT); the implementation is ours, and CSS.
 */
export function IconSwap({
  active,
  initial,
  swapped,
  className,
}: {
  /** Show `swapped` when true, `initial` when false. */
  active: boolean;
  initial: ReactNode;
  swapped: ReactNode;
  className?: string;
}) {
  return (
    // `grid` with both children in cell 1/1 keeps the box the size of the larger
    // icon, so a swap never nudges the surrounding layout.
    <span className={cn("relative grid place-items-center", className)} aria-hidden>
      <IconLayer visible={!active}>{initial}</IconLayer>
      <IconLayer visible={active}>{swapped}</IconLayer>
    </span>
  );
}

function IconLayer({ visible, children }: { visible: boolean; children: ReactNode }) {
  return (
    <span
      className={cn(
        "col-start-1 row-start-1 flex items-center justify-center",
        "transition-[opacity,transform,filter] duration-200 ease-out",
        !visible && "pointer-events-none"
      )}
      // Written as inline style rather than Tailwind's `scale-*`, which routes
      // through `--tw-scale-*` custom properties. A transition against a value
      // that is only reached through a variable is a rough edge worth avoiding
      // when a plain `transform` says the same thing unambiguously.
      style={{
        opacity: visible ? 1 : 0,
        // Scaled down and blurred rather than merely hidden, so the outgoing
        // icon reads as leaving instead of blinking out.
        transform: visible ? "scale(1)" : "scale(0.5)",
        filter: visible ? "blur(0px)" : "blur(2px)",
      }}
    >
      {children}
    </span>
  );
}
