/**
 * Pacing for streamed text.
 *
 * A gateway does not stream token by token — it hands over whatever it had
 * buffered, so a turn arrives as a few large bursts and reads as text
 * *appearing* rather than being typed. Revealing a slice per frame turns each
 * burst back into motion.
 */

/** Never stall, even on a one-character tail. */
const MIN_PER_FRAME = 3;
/** Roughly how many frames any backlog should take to clear. */
const DRAIN_FRAMES = 6;

/**
 * Characters to reveal this frame, given how many are waiting.
 *
 * Proportional to the backlog, which makes the drain exponential rather than
 * linear — each frame clears a fraction of what is left, so time-to-empty grows
 * *logarithmically* with burst size:
 *
 * | pending | frames | ≈ at 60fps |
 * |---------|--------|------------|
 * | 40      | 10     | 170 ms     |
 * | 400     | 23     | 380 ms     |
 * | 4000    | 35     | 580 ms     |
 * | 40000   | 48     | 800 ms     |
 *
 * That is the property worth having: a fixed characters-per-frame rate would
 * put a long answer seconds behind the real stream, whereas this stays under a
 * second even for a whole response delivered in one event.
 */
export function revealCount(pending: number): number {
  return Math.max(MIN_PER_FRAME, Math.ceil(pending / DRAIN_FRAMES));
}
