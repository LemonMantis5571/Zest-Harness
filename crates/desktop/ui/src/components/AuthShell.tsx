import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Shared enter motion for auth screens — transform/opacity only. */
export function AuthShell({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "w-full max-w-[400px] animate-in fade-in slide-in-from-bottom-2 duration-200 ease-out",
        className
      )}
    >
      {children}
    </div>
  );
}
