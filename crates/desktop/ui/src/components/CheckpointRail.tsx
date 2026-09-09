import { memo, useRef, useState } from "react";

import type { ConversationTurn } from "@/lib/conversationTurns";
import { cn } from "@/lib/utils";

type Props = {
  turns: ConversationTurn[];
  onJump: (messageId: string) => void;
};

type HoverCard = {
  turnId: string;
  preview: string;
  top: number;
  left: number;
};

function placeCard(el: HTMLElement, turn: ConversationTurn): HoverCard {
  const box = el.getBoundingClientRect();
  return {
    turnId: turn.id,
    preview: turn.preview,
    top: box.top + box.height / 2,
    left: box.right + 8,
  };
}

function previewUrl(preview: string): string | undefined {
  return preview.match(/https?:\/\/[^\s<>]+/)?.[0];
}

/**
 * Compact, dash-only turn navigation. Hover lengthens the mark and shows the
 * user-turn preview beside it. The rail stays a gutter, not a transcript map.
 */
export const CheckpointRail = memo(function CheckpointRail({ turns, onJump }: Props) {
  const markerRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [hover, setHover] = useState<HoverCard | null>(null);

  if (turns.length === 0) return null;

  const hoverUrl = hover ? previewUrl(hover.preview) : undefined;

  return (
    <div
      className="pointer-events-none absolute left-0 top-1/2 z-10 w-10 -translate-y-1/2"
      aria-label="Conversation history"
    >
      <div className="pointer-events-auto no-scrollbar flex max-h-[min(360px,60vh)] flex-col items-start gap-1 overflow-y-auto px-1.5 py-1">
        {turns.map((turn, index) => {
          const active = hover?.turnId === turn.id;
          return (
            <button
              key={turn.id}
              ref={(element) => {
                markerRefs.current[index] = element;
              }}
              type="button"
              data-turn={turn.id}
              aria-label={`Turn ${turn.number}: ${turn.preview}. Click to jump to this message.`}
              aria-describedby={active ? "checkpoint-rail-preview" : undefined}
              className="group/rail-marker relative flex h-2.5 w-8 shrink-0 items-center justify-start rounded-sm p-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-offset-2 focus-visible:ring-offset-background"
              onClick={() => {
                onJump(turn.messageId);
              }}
              onMouseEnter={(event) => {
                setHover(placeCard(event.currentTarget, turn));
              }}
              onMouseLeave={() => {
                setHover(null);
              }}
              onFocus={(event) => {
                setHover(placeCard(event.currentTarget, turn));
              }}
              onBlur={() => {
                setHover(null);
              }}
              onKeyDown={(event) => {
                if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                  event.preventDefault();
                  const next = event.key === "ArrowUp" ? index - 1 : index + 1;
                  markerRefs.current[next]?.focus();
                }
              }}
            >
              <span
                className={cn(
                  "pointer-events-none block h-px origin-left rounded-full transition-[width,background-color] duration-150",
                  active ? "w-8" : "w-3",
                  turn.checkpoint
                    ? active
                      ? "bg-primary"
                      : "bg-primary/70"
                    : active
                      ? "bg-foreground"
                      : "bg-muted-foreground/55"
                )}
                aria-hidden="true"
              />
            </button>
          );
        })}
      </div>
      {hover ? (
        <div
          id="checkpoint-rail-preview"
          role="tooltip"
          className="pointer-events-none fixed z-50 w-[min(20rem,calc(100vw-3rem))] -translate-y-1/2 rounded-lg border border-border/80 bg-popover px-3 py-2 text-left shadow-xl"
          style={{ top: hover.top, left: hover.left }}
        >
          {hoverUrl ? (
            <div className="mb-1 truncate text-[11px] text-primary">{hoverUrl}</div>
          ) : null}
          <p className="text-[12px] leading-snug text-muted-foreground">{hover.preview}</p>
        </div>
      ) : null}
    </div>
  );
});
