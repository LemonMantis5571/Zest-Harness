import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type Props = {
  icon: LucideIcon;
  label: string;
  children: ReactNode;
  trigger?: ReactNode;
  triggerClassName?: string;
  /** Footer chrome opens upward so the panel stays on screen. */
  placement?: "below" | "above";
  onOpenChange?: (open: boolean) => void;
};

/**
 * A small anchored panel for the desktop topbar.
 *
 * This intentionally stays local to the header instead of using a portal. The
 * Tauri webview has had fragile behaviour with portalled menu surfaces, and
 * these panels do not need to escape the chat header's stacking context.
 */
export function TopbarPanel({
  icon: Icon,
  label,
  children,
  trigger,
  triggerClassName,
  placement = "below",
  onOpenChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) return;

    const closeIfOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) {
        setOpen(false);
        onOpenChange?.(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        onOpenChange?.(false);
      }
    };

    document.addEventListener("pointerdown", closeIfOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeIfOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [onOpenChange, open]);

  useLayoutEffect(() => {
    if (!open) return;

    const positionPanel = () => {
      const root = rootRef.current;
      const panel = panelRef.current;
      if (!root || !panel) return;

      const rootRect = root.getBoundingClientRect();
      const viewportPadding = 8;
      const panelWidth = Math.min(
        panel.getBoundingClientRect().width,
        Math.max(0, window.innerWidth - viewportPadding * 2)
      );
      const anchoredLeft = rootRect.right - panelWidth;
      const maxLeft = window.innerWidth - viewportPadding - panelWidth;
      const pageLeft = Math.max(viewportPadding, Math.min(anchoredLeft, maxLeft));

      panel.style.left = `${pageLeft - rootRect.left}px`;
      panel.style.right = "auto";
    };

    const frame = window.requestAnimationFrame(positionPanel);
    window.addEventListener("resize", positionPanel);
    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", positionPanel);
    };
  }, [open]);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    onOpenChange?.(next);
  };

  return (
    <div ref={rootRef} className="relative min-w-0">
      <Button
        type="button"
        variant="ghost"
        size={trigger ? "sm" : "icon-sm"}
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        className={cn(
          "min-w-0 max-w-full overflow-hidden",
          trigger && "max-w-[230px]",
          triggerClassName
        )}
        onClick={toggle}
      >
        {trigger ?? <Icon aria-hidden="true" />}
      </Button>
      {open ? (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          aria-label={label}
          className={cn(
            "absolute right-0 z-50 max-h-[min(70vh,480px)] w-[330px] max-w-[calc(100vw-1rem)] overflow-y-auto rounded-lg border border-border bg-popover p-3 text-popover-foreground shadow-2xl",
            placement === "above"
              ? "bottom-[calc(100%+0.5rem)]"
              : "top-[calc(100%+0.5rem)]"
          )}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}
