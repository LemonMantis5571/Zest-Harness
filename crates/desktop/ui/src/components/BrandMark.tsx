import zestMark from "@/assets/zest-mark.png";

import { cn } from "@/lib/utils";

export function BrandMark({
  size = 28,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <img
      src={zestMark}
      alt=""
      width={size}
      height={size}
      draggable={false}
      className={cn("select-none", className)}
      aria-hidden="true"
    />
  );
}
