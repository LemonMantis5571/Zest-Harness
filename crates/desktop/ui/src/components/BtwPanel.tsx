import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUpIcon, LoaderCircleIcon, SquareIcon, XIcon } from "lucide-react";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { useDialogFocusTrap } from "@/lib/useDialogFocusTrap";

type Entry = { id: string; role: "user" | "assistant"; text: string };
type Lifetime = { alive: boolean; id: string | null; stopped: boolean };

export function BtwPanel({
  sessionId,
  initialQuestion,
  mainRunning,
  onClose,
}: {
  sessionId: string;
  initialQuestion: string;
  mainRunning: boolean;
  onClose: () => void;
}) {
  const backend = getBackend();
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState(initialQuestion);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const followOutput = useRef(true);
  const lifetime = useRef<Lifetime | null>(null);
  const inFlight = useRef(false);
  useDialogFocusTrap(true, dialog);

  const send = useCallback(
    async (text: string, life: Lifetime) => {
      if (!text.trim() || !life.id || !life.alive || inFlight.current) return;
      inFlight.current = true;
      life.stopped = false;
      setBusy(true);
      setError(null);
      setDraft("");
      followOutput.current = true;
      const userId = crypto.randomUUID();
      const answerId = crypto.randomUUID();
      let settled = false;
      setEntries((previous) => [
        ...previous,
        { id: userId, role: "user", text },
        { id: answerId, role: "assistant", text: "" },
      ]);
      try {
        const answer = await backend.sendBtw(life.id, text, (delta) => {
          if (!life.alive || settled) return;
          setEntries((previous) =>
            previous.map((entry) =>
              entry.id === answerId
                ? { ...entry, text: entry.text + delta }
                : entry,
            ),
          );
        });
        if (life.alive)
          setEntries((previous) =>
            previous.map((entry) =>
              entry.id === answerId ? { ...entry, text: answer } : entry,
            ),
          );
      } catch (reason) {
        if (life.alive) {
          setEntries((previous) =>
            previous.filter(
              (entry) => entry.id !== userId && entry.id !== answerId,
            ),
          );
          setDraft(text);
          setError(
            life.stopped
              ? "Answer stopped. You can edit or send your question again."
              : String(reason),
          );
        }
      } finally {
        settled = true;
        if (life.alive) {
          inFlight.current = false;
          setBusy(false);
          input.current?.focus();
        }
      }
    },
    [backend],
  );

  useEffect(() => {
    const life: Lifetime = { alive: true, id: null, stopped: false };
    lifetime.current = life;
    void backend
      .startBtw(sessionId)
      .then((id) => {
        life.id = id;
        if (!life.alive) {
          void backend
            .closeBtw(id)
            .catch((reason) =>
              ignoreExpectedFailure(
                reason,
                "close unmounted side conversation",
              ),
            );
          return;
        }
        setReady(true);
        input.current?.focus();
        if (initialQuestion) void send(initialQuestion, life);
      })
      .catch((reason) => {
        if (life.alive) setError(String(reason));
      });
    return () => {
      life.alive = false;
      if (life.id)
        void backend
          .closeBtw(life.id)
          .catch((reason) =>
            ignoreExpectedFailure(reason, "close side conversation"),
          );
    };
  }, [backend, sessionId, initialQuestion, send]);

  useEffect(() => {
    if (ready && !busy) input.current?.focus();
  }, [ready, busy]);

  useEffect(() => {
    if (followOutput.current && viewport.current) {
      viewport.current.scrollTop = viewport.current.scrollHeight;
    }
  }, [entries]);

  async function stop() {
    const life = lifetime.current;
    if (!life?.id) return;
    life.stopped = true;
    try {
      await backend.cancelBtw(life.id);
    } catch (reason) {
      if (life.alive) setError(String(reason));
    }
  }

  return (
    <div
      className="absolute inset-0 z-50 flex justify-end bg-black/35 p-2 sm:p-4"
      onKeyDownCapture={(event) => {
        if (event.key === "Escape" && !event.nativeEvent.isComposing) {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="btw-title"
        aria-describedby="btw-description"
        tabIndex={-1}
        className="relative flex h-full w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border bg-background shadow-2xl"
      >
        <header className="flex shrink-0 items-start justify-between gap-3 border-b border-border px-5 py-4">
          <div>
            <div className="flex items-center gap-2.5">
              <span className="rounded-md bg-secondary px-2 py-1 font-mono text-xs text-muted-foreground">
                /btw
              </span>
              <h2 id="btw-title" className="text-sm font-semibold">
                Side conversation
              </h2>
            </div>
            <p
              id="btw-description"
              className="mt-2 max-w-sm text-xs leading-relaxed text-muted-foreground"
            >
              Uses this chat’s completed context. Messages stay here and leave
              your main chat unchanged.
            </p>
            {mainRunning ? (
              <p className="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground">
                <LoaderCircleIcon className="size-3 animate-spin" />
                Main task is still running
              </p>
            ) : null}
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Close side conversation"
            title="Close (Esc)"
            onClick={onClose}
          >
            <XIcon />
          </Button>
        </header>
        <div
          ref={viewport}
          className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-5"
          role="log"
          aria-label="Side conversation messages"
          aria-busy={busy}
          onScroll={() => {
            const el = viewport.current;
            if (el)
              followOutput.current =
                el.scrollHeight - el.scrollTop - el.clientHeight < 80;
          }}
        >
          {!entries.length ? (
            <div className="py-10 text-center">
              <p className="text-sm font-medium">Ask a side question</p>
              <p className="mx-auto mt-2 max-w-xs text-sm leading-relaxed text-muted-foreground">
                Clarify a decision or explore an idea, then return to your main
                task.
              </p>
            </div>
          ) : null}
          <div className="space-y-5">
            {entries.map((entry) => (
              <div
                key={entry.id}
                className={
                  entry.role === "user"
                    ? "ml-8 rounded-xl bg-secondary px-4 py-3 text-sm whitespace-pre-wrap break-words"
                    : "min-w-0 text-sm"
                }
              >
                {entry.role === "user" ? (
                  entry.text
                ) : entry.text ? (
                  <Markdown streaming={busy && entry.id === entries.at(-1)?.id}>
                    {entry.text}
                  </Markdown>
                ) : (
                  <p className="flex items-center gap-2 text-muted-foreground">
                    <LoaderCircleIcon className="size-4 animate-spin" />
                    Thinking…
                  </p>
                )}
              </div>
            ))}
          </div>
        </div>
        <form
          className="shrink-0 border-t border-border p-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (lifetime.current) void send(draft, lifetime.current);
          }}
        >
          {error ? (
            <p role="alert" className="mb-3 text-xs text-destructive">
              {error}
            </p>
          ) : null}
          <div className="rounded-xl border border-border bg-card p-3 focus-within:border-ring">
            <textarea
              ref={input}
              aria-label="Side question"
              placeholder={
                ready ? "Ask a side question…" : "Opening side conversation…"
              }
              value={draft}
              disabled={!ready || busy}
              onChange={(event) => setDraft(event.target.value)}
              rows={3}
              className="w-full resize-none bg-transparent text-sm outline-none placeholder:text-muted-foreground disabled:opacity-60"
              onKeyDown={(event) => {
                if (
                  event.key === "Enter" &&
                  !event.shiftKey &&
                  !event.nativeEvent.isComposing
                ) {
                  event.preventDefault();
                  if (lifetime.current) void send(draft, lifetime.current);
                }
              }}
            />
            <div className="mt-2 flex items-center justify-between gap-3">
              <span className="text-[11px] text-muted-foreground">
                Temporary · Esc to return
              </span>
              {busy ? (
                <Button
                  type="button"
                  size="icon-sm"
                  variant="secondary"
                  aria-label="Stop side answer"
                  onClick={() => void stop()}
                >
                  <SquareIcon />
                </Button>
              ) : (
                <Button
                  type="submit"
                  size="icon-sm"
                  aria-label="Send side question"
                  disabled={!ready || !draft.trim()}
                >
                  <ArrowUpIcon />
                </Button>
              )}
            </div>
          </div>
        </form>
      </div>
    </div>
  );
}
