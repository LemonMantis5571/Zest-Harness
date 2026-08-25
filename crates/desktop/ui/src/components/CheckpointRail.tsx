import { useRef } from "react";

import type { ConversationTurn } from "@/lib/conversationTurns";
import { cn } from "@/lib/utils";

type Props = {
  turns: ConversationTurn[];
  onJump: (messageId: string) => void;
};

/**
 * Compact, dash-only turn navigation. The rail stays small and centered
 * instead of stretching across the whole transcript and mirroring message
 * positions one-for-one.
 */
export function CheckpointRail({ turns, onJump }: Props) {
  const markerRefs = useRef<Array<HTMLButtonElement | null>>([]);

  if (turns.length === 0) return null;

  return (
    <div
      className="pointer-events-none absolute left-0 top-1/2 z-10 w-7 -translate-y-1/2"
      aria-label="Conversation history"
    >
      <div className="pointer-events-auto no-scrollbar flex max-h-[min(360px,60vh)] flex-col items-center gap-1 overflow-y-auto px-1.5 py-1">
        {turns.map((turn, index) => (
          <button
            key={turn.id}
            ref={(element) => {
              markerRefs.current[index] = element;
            }}
            type="button"
            data-turn={turn.id}
            aria-label={`Turn ${turn.number}: ${turn.preview}. Click to jump to this message.`}
            className="group/rail-marker relative flex h-2.5 w-4 shrink-0 items-center justify-center rounded-full p-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-offset-2 focus-visible:ring-offset-background"
            onClick={() => {
              onJump(turn.messageId);
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
                "pointer-events-none block h-px w-3 rounded-full transition-[width,background-color,box-shadow] group-hover/rail-marker:w-4 group-focus-visible/rail-marker:w-4",
                turn.checkpoint
                  ? "bg-primary/70 group-hover/rail-marker:bg-primary"
                  : "bg-muted-foreground/55 group-hover/rail-marker:bg-muted-foreground"
              )}
              aria-hidden="true"
            />
          </button>
        ))}
      </div>
    </div>
  );
}
