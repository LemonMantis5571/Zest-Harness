import { LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";

import { paneStripLabel, type PaneStatus } from "@/lib/splitPaneStatus";
import { cn } from "@/lib/utils";

/**
 * A split pane put away: its chat title and what the chat is doing, as one
 * button that brings the pane back.
 *
 * It is a thin column beside an expanded pane and a short row above or below
 * one; at 700 px and under every strip is a row (see `.zest-split-strip` in
 * `index.css`). The transcript, draft and running turn stay mounted behind it.
 */
export function SplitPaneStrip({
  paneId,
  title,
  status,
  orientation,
  controlsId,
  onRestore,
}: {
  paneId: string;
  title: string;
  status: PaneStatus;
  orientation: "vertical" | "horizontal";
  /** The pane body this strip stands in for. */
  controlsId: string;
  onRestore: () => void;
}) {
  return (
    <button
      type="button"
      data-split-strip={paneId}
      data-status={status.kind}
      data-orientation={orientation}
      aria-label={paneStripLabel(title, status)}
      aria-expanded={false}
      aria-controls={controlsId}
      title={`${title} · ${status.label}`}
      onClick={onRestore}
      className="zest-split-strip text-xs outline-none hover:bg-secondary focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <StatusMark kind={status.kind} />
      <span
        className={cn(
          "zest-split-strip-title",
          status.kind === "unread" ? "font-semibold text-foreground" : "font-medium"
        )}
      >
        {title}
      </span>
      {status.kind === "idle" || status.kind === "empty" ? null : (
        <span
          className={cn(
            "zest-split-strip-status text-[10px]",
            status.kind === "needs_input" ? "text-amber-400" : "text-muted-foreground"
          )}
        >
          {status.label}
        </span>
      )}
    </button>
  );
}

function StatusMark({ kind }: { kind: PaneStatus["kind"] }) {
  return (
    <span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden="true">
      {kind === "needs_input" ? (
        <TriangleAlertIcon className="size-3.5 text-amber-400" />
      ) : kind === "working" ? (
        <LoaderCircleIcon className="size-3.5 animate-spin text-[var(--sidebar-primary)] motion-reduce:animate-none" />
      ) : kind === "unread" ? (
        <span className="size-2 rounded-full bg-[var(--sidebar-primary)]" />
      ) : (
        <span className="size-2 rounded-full border border-muted-foreground/50" />
      )}
    </span>
  );
}
