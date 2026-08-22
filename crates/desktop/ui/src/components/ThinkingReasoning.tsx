/**
 * The thinking disclosure: a shimmering label while a turn reasons, folding
 * into "Thought for Ns" with the trace behind it.
 *
 * Adapted from the `thinking-reasoning` block on aicss.dev. The published block
 * is a showcase — it ships a hardcoded array of sentences about a JWT fix and a
 * fixed timeline of delays, and takes no props. What is kept is its behaviour:
 * the shimmer, the height-capped viewport with fade masks, the stream that
 * translates so the newest line stays in view, and the collapse into a summary.
 * What is replaced is every source of truth — the sentences are the provider's
 * real reasoning, and the duration is measured rather than declared.
 */
import { useEffect, useMemo, useRef, useState } from "react";

import { thinkingSentences, thoughtForLabel } from "@/lib/thinkingSummary";

import styles from "./ThinkingReasoning.module.css";

/** Height of one clamped two-line sentence, and the gap between them. */
const SENTENCE_HEIGHT = 40;
const SENTENCE_GAP = 4;
/** Tallest the viewport grows before it starts scrolling under a fade. */
const MAX_HEIGHT = 180;
/** Depth of the fade at a masked edge. */
const FADE = 16;

type Props = {
  thinking: string;
  streaming: boolean;
  /** Shown while a turn is working but has produced no reasoning yet. */
  emptyLabel?: string;
};

/**
 * Seconds since `active` last became true, frozen when it goes false.
 *
 * Deliberately not derived from the message: thinking text carries no
 * timestamps, so the only honest duration is the one this window measured.
 * Null until something has actually been watched.
 */
function useElapsedSeconds(active: boolean): number | null {
  const [seconds, setSeconds] = useState<number | null>(null);
  const startedAt = useRef<number | null>(null);

  useEffect(() => {
    if (!active) {
      startedAt.current = null;
      return;
    }
    const start = Date.now();
    startedAt.current = start;
    setSeconds(0);
    const timer = window.setInterval(() => {
      setSeconds(Math.round((Date.now() - start) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [active]);

  return seconds;
}

export function ThinkingReasoning({
  thinking,
  streaming,
  emptyLabel = "Working...",
}: Props) {
  const sentences = useMemo(() => thinkingSentences(thinking), [thinking]);
  const [open, setOpen] = useState(false);
  const [fade, setFade] = useState({ top: false, bottom: true });
  const viewportRef = useRef<HTMLDivElement>(null);
  const elapsed = useElapsedSeconds(streaming);

  // A turn that produced nothing to show and is not producing anything now has
  // no disclosure to offer, and an empty one would still occupy a row.
  if (!streaming && sentences.length === 0) return null;

  // While streaming the trace is always open: it is the only sign of progress,
  // and a user who has to click to see whether anything is happening is being
  // told nothing. Once settled, their own choice takes over.
  const expanded = streaming || open;
  const lines = sentences.length > 0 ? sentences : [emptyLabel];
  const pending = sentences.length === 0;

  const contentHeight =
    lines.length * SENTENCE_HEIGHT + (lines.length - 1) * SENTENCE_GAP;
  const capped = contentHeight > MAX_HEIGHT;
  const viewportHeight = capped ? MAX_HEIGHT : contentHeight;
  /** Scrolling is the settled reading mode; a live trace is driven, not browsed. */
  const scrollable = !streaming && open;
  // Pull the stream up so its newest line sits at the bottom edge while it
  // grows, leaving one fade's worth of the previous line visible above it.
  const translate = scrollable
    ? 0
    : capped
      ? MAX_HEIGHT - FADE - contentHeight
      : 0;

  const showTop = scrollable ? fade.top : capped;
  const showBottom = scrollable ? fade.bottom : capped;
  const mask = capped
    ? `linear-gradient(to bottom, transparent 0, #000 ${showTop ? FADE : 0}px, #000 calc(100% - ${
        showBottom ? FADE : 0
      }px), transparent 100%)`
    : "none";

  const onScroll = () => {
    const element = viewportRef.current;
    if (!element) return;
    setFade({
      top: element.scrollTop > 1,
      bottom:
        element.scrollTop + element.clientHeight < element.scrollHeight - 1,
    });
  };

  const toggle = () => {
    const next = !open;
    if (next) {
      setFade({ top: false, bottom: true });
      if (viewportRef.current) viewportRef.current.scrollTop = 0;
    }
    setOpen(next);
  };

  const summary = thoughtForLabel(elapsed, thinking);

  return (
    <div className={styles.tr}>
      <button
        type="button"
        className={
          streaming ? styles.trHeader : `${styles.trHeader} ${styles.isClickable}`
        }
        aria-expanded={expanded}
        aria-label={
          streaming
            ? "Thinking"
            : expanded
              ? "Hide the reasoning"
              : "Show the reasoning"
        }
        onClick={streaming ? undefined : toggle}
      >
        {streaming ? (
          <span
            aria-live="polite"
            className={`${styles.trLabel} ${styles.trShimmer}`}
          >
            Thinking…
          </span>
        ) : (
          <span className={styles.trLabel}>
            <span className={styles.trVerb}>{summary}</span>
          </span>
        )}
        {streaming ? null : (
          <svg
            className={styles.trChevron}
            viewBox="0 0 24 24"
            width="12"
            height="12"
            aria-hidden="true"
          >
            <path
              d="m4.5 15.75 7.5-7.5 7.5 7.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </button>

      <div
        className={
          expanded
            ? styles.trCollapsible
            : `${styles.trCollapsible} ${styles.isCollapsed}`
        }
      >
        <div className={styles.trInner}>
          <div
            ref={viewportRef}
            className={
              scrollable
                ? `${styles.trViewport} ${styles.isScroll}`
                : styles.trViewport
            }
            style={{
              height: `${viewportHeight}px`,
              WebkitMaskImage: mask,
              maskImage: mask,
            }}
            onScroll={scrollable ? onScroll : undefined}
          >
            <div
              className={styles.trStream}
              style={{ transform: `translateY(${translate}px)` }}
            >
              {lines.map((line, index) => (
                <p
                  key={`${index}-${line.slice(0, 24)}`}
                  className={pending ? styles.trPending : styles.trSentence}
                >
                  {line}
                </p>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
