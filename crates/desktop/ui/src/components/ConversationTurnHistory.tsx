import { useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2Icon,
  CircleAlertIcon,
  LoaderCircleIcon,
  MessageSquareTextIcon,
  SearchIcon,
  XIcon,
} from "lucide-react";

import type { ConversationTurn } from "@/lib/conversationTurns";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";

type Props = {
  turns: ConversationTurn[];
  messageCount: number;
  onJump: (messageId: string) => void;
};

function summaryLabel(turns: ConversationTurn[], messageCount: number): string {
  const toolCalls = turns.reduce((total, turn) => total + turn.toolCount, 0);
  return `${toolCalls} tool call${toolCalls === 1 ? "" : "s"} · ${messageCount} message${messageCount === 1 ? "" : "s"}`;
}

function statusIcon(status: ConversationTurn["status"]) {
  if (status === "working") return <LoaderCircleIcon className="size-3.5 animate-spin text-primary" aria-hidden="true" />;
  if (status === "error") return <CircleAlertIcon className="size-3.5 text-amber-300" aria-hidden="true" />;
  if (status === "done") return <CheckCircle2Icon className="size-3.5 text-muted-foreground" aria-hidden="true" />;
  return <MessageSquareTextIcon className="size-3.5 text-muted-foreground" aria-hidden="true" />;
}

export function ConversationTurnHistory({ turns, messageCount, onJump }: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);

  const visibleTurns = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return turns;
    return turns.filter(
      (turn) =>
        turn.preview.toLowerCase().includes(normalized) ||
        String(turn.number).includes(normalized)
    );
  }, [query, turns]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => searchRef.current?.focus());
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  if (turns.length === 0) return null;

  function jump(turn: ConversationTurn) {
    setOpen(false);
    setQuery("");
    onJump(turn.messageId);
  }

  return (
    <div ref={rootRef} className="relative">
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        className={cn(
          "size-7 shrink-0 px-0 text-muted-foreground",
          open && "bg-secondary/70 text-foreground"
        )}
        aria-label={`Open chat turn history: ${summaryLabel(turns, messageCount)}`}
        title="Open chat history"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="flex flex-col items-start gap-1" aria-hidden="true">
          {turns.slice(0, 3).map((turn) => (
            <span key={turn.id} className="block h-px w-3 rounded-full bg-muted-foreground/65" />
          ))}
        </span>
        <span className="sr-only">{summaryLabel(turns, messageCount)}</span>
      </Button>

      {open ? (
        <div
          role="dialog"
          aria-label="Chat turn history"
          className="absolute right-0 top-9 z-40 w-[min(360px,calc(100vw-2rem))] overflow-hidden rounded-xl border border-border/80 bg-card/98 text-card-foreground shadow-2xl shadow-black/30 backdrop-blur-sm"
        >
          <div className="border-b border-border/60 p-2">
            <div className="mb-1.5 flex items-center justify-between gap-2 px-1">
              <div className="text-xs font-semibold tracking-tight">Chat history</div>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label="Close chat history"
                title="Close"
                onClick={() => setOpen(false)}
              >
                <XIcon aria-hidden="true" />
              </Button>
            </div>
            <div className="flex h-8 items-center gap-2 rounded-md border border-border/70 bg-background/60 px-2 focus-within:border-ring/70 focus-within:ring-2 focus-within:ring-ring/25">
              <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
              <input
                ref={searchRef}
                value={query}
                aria-label="Search chat turns"
                placeholder="Search turns..."
                className="min-w-0 flex-1 bg-transparent text-[11px] outline-none placeholder:text-muted-foreground/60"
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
          </div>
          <div className="max-h-[min(420px,60vh)] overflow-y-auto p-1.5" role="list">
            {visibleTurns.length === 0 ? (
              <div className="px-2.5 py-5 text-center text-[11px] text-muted-foreground">
                No matching turns.
              </div>
            ) : (
              visibleTurns.map((turn, index) => (
                <button
                  key={turn.id}
                  ref={(element) => {
                    itemRefs.current[index] = element;
                  }}
                  type="button"
                  role="listitem"
                  className="flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-secondary/60 focus-visible:bg-secondary/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                  onClick={() => jump(turn)}
                  onKeyDown={(event) => {
                    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                    event.preventDefault();
                    const next = event.key === "ArrowUp" ? index - 1 : index + 1;
                    itemRefs.current[next]?.focus();
                  }}
                >
                  <span className="mt-0.5 shrink-0">{statusIcon(turn.status)}</span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center justify-between gap-2">
                      <span className="text-[10px] font-semibold uppercase tracking-[0.08em] text-muted-foreground">
                        Turn {turn.number}
                      </span>
                      <span className="shrink-0 text-[10px] text-muted-foreground">
                        {turn.toolCount} tool{turn.toolCount === 1 ? "" : "s"}
                      </span>
                    </span>
                    <span className="mt-1 block line-clamp-2 text-[11px] leading-relaxed text-foreground/85">
                      {turn.preview}
                    </span>
                  </span>
                </button>
              ))
            )}
          </div>
          <div className="border-t border-border/60 px-3 py-2 text-[10px] text-muted-foreground">
            Click a turn to jump there. Conversation history is unchanged.
          </div>
        </div>
      ) : null}
    </div>
  );
}
