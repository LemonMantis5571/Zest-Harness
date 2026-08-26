import { useEffect, useRef, useState } from "react";
import { CheckIcon, ShieldIcon } from "lucide-react";

import { APPROVAL_MODES, type ApprovalMode } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  mode: ApprovalMode;
  disabled?: boolean;
  onModeChange: (mode: ApprovalMode) => void;
};

/**
 * Plain positioned panel. Portal-based menus have been crashing the desktop
 * webview on open.
 *
 * Rust owns the mode. This only reports what the user picked; App puts the
 * chip back if the backend refuses.
 */
export function ApprovalModePicker({ mode, disabled, onModeChange }: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const current = APPROVAL_MODES.find((m) => m.id === mode) ?? APPROVAL_MODES[3];

  useEffect(() => {
    if (!open) return;

    const onPointerDown = (event: PointerEvent) => {
      const root = rootRef.current;
      if (!root) return;
      if (event.target instanceof Node && !root.contains(event.target)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  // Number keys mirror the order shown, so the list is drivable without aiming.
  useEffect(() => {
    if (!open) return;
    const onDigit = (event: KeyboardEvent) => {
      const index = Number(event.key) - 1;
      if (!Number.isInteger(index)) return;
      const target = APPROVAL_MODES[index];
      if (!target) return;
      event.preventDefault();
      if (target.id !== mode) onModeChange(target.id);
      setOpen(false);
    };
    document.addEventListener("keydown", onDigit);
    return () => document.removeEventListener("keydown", onDigit);
  }, [open, mode, onModeChange]);

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        type="button"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "flex items-center gap-1 whitespace-nowrap rounded-md px-1.5 py-0.5 text-[11px] transition-colors",
          "hover:bg-white/[0.04] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
          disabled && "cursor-not-allowed opacity-50",
          // Bypass is the one mode worth noticing from across the room.
          mode === "bypass"
            ? "text-[var(--destructive,#e5484d)]"
            : mode === "plan"
              ? "text-[#6b86d4]"
              : "text-muted-foreground"
        )}
        title={current.hint}
      >
        <ShieldIcon className="size-3" />
        <span>{current.label}</span>
      </button>

      {open ? (
        <div
          role="menu"
          className={cn(
            "absolute right-0 bottom-[calc(100%+6px)] z-40 w-64 max-w-[calc(100vw-1rem)] max-h-[min(24rem,calc(100vh-1rem))] overflow-y-auto",
            "rounded-lg border border-border/80 bg-popover p-1 shadow-xl"
          )}
        >
          <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70">
            Mode
          </div>
          {APPROVAL_MODES.map((option, index) => {
            const active = option.id === mode;
            return (
              <button
                key={option.id}
                type="button"
                role="menuitemradio"
                aria-checked={active}
                onClick={() => {
                  if (!active) onModeChange(option.id);
                  setOpen(false);
                }}
                className={cn(
                  "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
                  "hover:bg-white/[0.05] focus-visible:outline-none focus-visible:bg-white/[0.05]"
                )}
              >
                <span className="mt-0.5 grid size-3.5 shrink-0 place-items-center">
                  {active ? (
                    <CheckIcon className="size-3 text-foreground" />
                  ) : null}
                </span>
                <span className="min-w-0 flex-1">
                  <span
                    className={cn(
                      "block text-[12px]",
                      active ? "text-foreground" : "text-foreground/85",
                      option.id === "bypass" &&
                        "text-[var(--destructive,#e5484d)]"
                    )}
                  >
                    {option.label}
                  </span>
                  <span className="block text-[10px] leading-snug text-muted-foreground/80">
                    {option.hint}
                  </span>
                </span>
                <span className="mt-0.5 shrink-0 text-[10px] text-muted-foreground/50">
                  {index + 1}
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
