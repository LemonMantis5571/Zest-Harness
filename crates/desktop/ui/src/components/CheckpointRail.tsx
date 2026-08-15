import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { WaypointsIcon } from "lucide-react";

import { resolveCheckpointMarkerPositions } from "@/lib/checkpointPositions";
import type { ChatMessage, ThreadCheckpoint } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  checkpoints: ThreadCheckpoint[];
  messages: ChatMessage[];
  onJump: (messageId: string) => void;
};

function checkpointMessageId(checkpoint: ThreadCheckpoint, messages: ChatMessage[]) {
  if (checkpoint.anchorMessageId) return checkpoint.anchorMessageId;
  return messages[checkpoint.messageCount]?.id ?? messages[checkpoint.messageCount - 1]?.id;
}

function checkpointAge(createdAt: number): string {
  const elapsed = Math.max(0, Date.now() - createdAt * 1000);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export function CheckpointRail({ checkpoints, messages, onJump }: Props) {
  const railRef = useRef<HTMLDivElement>(null);
  const markerRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [positions, setPositions] = useState<number[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);

  const anchors = useMemo(
    () => checkpoints.map((checkpoint) => checkpointMessageId(checkpoint, messages)),
    [checkpoints, messages]
  );

  const recalculate = useCallback(() => {
    const rail = railRef.current;
    if (!rail) return;
    const railRect = rail.getBoundingClientRect();
    const height = rail.clientHeight;
    const desiredPositions = checkpoints.map((checkpoint, index) => {
      const anchor = anchors[index];
      const element = anchor ? document.getElementById(`message-${anchor}`) : null;
      if (element) {
        return Math.max(8, Math.min(height - 28, element.getBoundingClientRect().top - railRect.top));
      }
      const fallback = messages.length > 1
        ? (checkpoint.messageCount / Math.max(1, messages.length - 1)) * Math.max(0, height - 28)
        : 8;
      return Math.max(8, Math.min(height - 28, fallback));
    });
    setPositions(resolveCheckpointMarkerPositions(desiredPositions, height));
  }, [anchors, checkpoints, messages.length]);

  useLayoutEffect(() => {
    recalculate();
    const rail = railRef.current;
    if (!rail) return;
    const observer = typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(recalculate);
    observer?.observe(rail);
    window.addEventListener("resize", recalculate);
    window.addEventListener("scroll", recalculate, true);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", recalculate);
      window.removeEventListener("scroll", recalculate, true);
    };
  }, [recalculate]);

  useEffect(() => {
    if (!activeId) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setActiveId(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeId]);

  if (checkpoints.length === 0) return null;

  return (
    <div
      ref={railRef}
      className="pointer-events-none absolute inset-y-0 left-0 z-10 w-8"
      aria-label="Conversation checkpoints"
    >
      <div className="absolute bottom-4 left-3 top-4 w-px bg-border/45" aria-hidden="true" />
      {checkpoints.map((checkpoint, index) => {
        const active = activeId === checkpoint.id;
        const messageId = anchors[index];
        return (
          <div
            key={checkpoint.id}
            className="pointer-events-auto absolute left-0"
            style={{ top: positions[index] ?? 8 }}
            onMouseEnter={() => setActiveId(checkpoint.id)}
            onMouseLeave={() => {
              if (!active) return;
              window.setTimeout(() => {
                if (!document.activeElement?.closest(`[data-checkpoint="${checkpoint.id}"]`)) {
                  setActiveId(null);
                }
              }, 80);
            }}
          >
            <button
              ref={(element) => {
                markerRefs.current[index] = element;
              }}
              type="button"
              data-checkpoint={checkpoint.id}
              aria-label={`${checkpoint.label}: ${checkpoint.preview ?? "checkpoint"}${messageId ? ". Click to jump to its message." : ""}`}
              aria-expanded={active}
              className={cn(
                "flex size-6 items-center justify-center rounded-full border transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
                active
                  ? "border-primary/80 bg-primary/20 text-primary shadow-sm"
                  : "border-border/80 bg-[var(--chat-canvas)] text-muted-foreground hover:border-primary/60 hover:text-primary"
              )}
              onFocus={() => setActiveId(checkpoint.id)}
              onClick={() => {
                if (!messageId) {
                  setActiveId(checkpoint.id);
                  return;
                }
                setActiveId(null);
                onJump(messageId);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  if (messageId) {
                    setActiveId(null);
                    onJump(messageId);
                  } else {
                    setActiveId(checkpoint.id);
                  }
                } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                  event.preventDefault();
                  const next = event.key === "ArrowUp" ? index - 1 : index + 1;
                  markerRefs.current[next]?.focus();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  setActiveId(null);
                }
              }}
            >
              <WaypointsIcon className="size-3" aria-hidden="true" />
            </button>
            {active ? (
              <div className="absolute left-7 top-0 z-30 w-72 max-w-[calc(100vw-3rem)] overflow-hidden rounded-xl border border-border/80 bg-card/95 text-card-foreground shadow-2xl shadow-black/25 backdrop-blur-sm">
                <div className="flex items-start gap-2.5 border-b border-border/60 px-3 py-2.5">
                  <div className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
                    <WaypointsIcon className="size-4" aria-hidden="true" />
                  </div>
                  <div className="min-w-0 flex-1 pt-px">
                    <div className="truncate text-xs font-semibold tracking-tight">{checkpoint.label}</div>
                    <div className="mt-0.5 text-[10px] text-muted-foreground">
                      {checkpointAge(checkpoint.createdAt)} <span aria-hidden="true">·</span> {checkpoint.kind}
                    </div>
                  </div>
                </div>
                <div className="px-3 py-2.5">
                  <p className="m-0 max-h-16 overflow-hidden border-l-2 border-primary/30 pl-2.5 text-[11px] leading-relaxed text-muted-foreground">
                    {checkpoint.preview ?? "Conversation checkpoint"}
                  </p>
                </div>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
