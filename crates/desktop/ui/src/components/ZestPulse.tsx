import { BrandMark } from "@/components/BrandMark";
import { cn } from "@/lib/utils";

/** Compact Zest activity mark while the agent is thinking, typing, or working. */
export function ZestPulse({
  size = 14,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <span
      role="status"
      aria-label="Working"
      className={cn("inline-flex shrink-0 animate-pulse", className)}
    >
      <BrandMark size={size} />
    </span>
  );
}
