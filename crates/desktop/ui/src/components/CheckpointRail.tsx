import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { resolveCheckpointMarkerPositions } from "@/lib/checkpointPositions";
import type { ConversationTurn } from "@/lib/conversationTurns";
import { cn } from "@/lib/utils";

type Props = {
  turns: ConversationTurn[];
  onJump: (messageId: string) => void;
};

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

function statusLabel(status: ConversationTurn["status"]): string {
  if (status === "working") return "working";
  if (status === "pending") return "waiting for a response";
  if (status === "error") return "ended with an error";
  return "complete";
}

export function CheckpointRail({ turns, onJump }: Props) {
  const railRef = useRef<HTMLDivElement>(null);
  const markerRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [positions, setPositions] = useState<number[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);

  const recalculate = useCallback(() => {
    const rail = railRef.current;
    if (!rail) return;
    const railRect = rail.getBoundingClientRect();
    const height = rail.clientHeight;
    const desiredPositions = turns.map((turn, index) => {
      const element = document.getElementById(`message-${turn.messageId}`);
      if (element) {
        return Math.max(8, Math.min(height - 28, element.getBoundingClientRect().top - railRect.top));
      }
      const fallback = turns.length > 1
        ? (index / Math.max(1, turns.length - 1)) * Math.max(0, height - 28)
        : 8;
      return Math.max(8, Math.min(height - 28, fallback));
    });
    setPositions(resolveCheckpointMarkerPositions(desiredPositions, height));
  }, [turns]);

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

  const activeTurn = useMemo(
    () => turns.find((turn) => turn.id === activeId) ?? null,
    [activeId, turns]
  );

  if (turns.length === 0) return null;

  return (
    <div
      ref={railRef}
      className="pointer-events-none absolute inset-y-0 left-0 z-10 w-8"
      aria-label="Conversation turns"
    >
      <div className="absolute bottom-4 left-3 top-4 w-px bg-border/35" aria-hidden="true" />
      {turns.map((turn, index) => {
        const active = activeId === turn.id;
        return (
          <div
            key={turn.id}
            className="pointer-events-auto absolute left-1"
            style={{ top: positions[index] ?? 8 }}
            onMouseEnter={() => setActiveId(turn.id)}
            onMouseLeave={() => {
              if (!active) return;
              window.setTimeout(() => {
                if (!document.activeElement?.closest(`[data-turn="${turn.id}"]`)) {
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
              data-turn={turn.id}
              aria-label={`Turn ${turn.number}: ${turn.preview}. Click to jump to this message.`}
              aria-expanded={active}
              className={cn(
                "block h-1 w-4 rounded-full transition-all focus-visible:h-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
                active
                  ? "w-6 bg-primary shadow-[0_0_10px_color-mix(in_srgb,var(--primary)_50%,transparent)]"
                  : turn.checkpoint
                    ? "bg-primary/70 hover:w-6 hover:bg-primary"
                    : "bg-muted-foreground/55 hover:w-6 hover:bg-muted-foreground"
              )}
              onFocus={() => setActiveId(turn.id)}
              onClick={() => {
                setActiveId(null);
                onJump(turn.messageId);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  setActiveId(null);
                  onJump(turn.messageId);
                } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                  event.preventDefault();
                  const next = event.key === "ArrowUp" ? index - 1 : index + 1;
                  markerRefs.current[next]?.focus();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  setActiveId(null);
                }
              }}
            />
            {active && activeTurn ? (
              <div className="absolute left-6 top-1/2 z-30 w-72 -translate-y-1/2 overflow-hidden rounded-xl border border-border/80 bg-card/95 text-card-foreground shadow-2xl shadow-black/25 backdrop-blur-sm">
                <div className="border-b border-border/60 px-3 py-2.5">
                  <div className="flex items-center justify-between gap-2">
                    <div className="text-xs font-semibold tracking-tight">Turn {activeTurn.number}</div>
                    <div className="text-[10px] text-muted-foreground">
                      {activeTurn.toolCount} tool call{activeTurn.toolCount === 1 ? "" : "s"}
                    </div>
                  </div>
                  <div className="mt-0.5 text-[10px] text-muted-foreground">
                    {statusLabel(activeTurn.status)}
                    {activeTurn.checkpoint ? (
                      <>
                        <span aria-hidden="true"> · </span>
                        {checkpointAge(activeTurn.checkpoint.createdAt)}
                      </>
                    ) : null}
                  </div>
                </div>
                <p className="m-0 max-h-20 overflow-hidden px-3 py-2.5 text-[11px] leading-relaxed text-muted-foreground">
                  {activeTurn.preview}
                </p>
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
