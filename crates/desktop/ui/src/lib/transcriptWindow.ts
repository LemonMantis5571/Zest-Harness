/** Number of recent messages mounted when a long transcript opens. */
export const TRANSCRIPT_WINDOW_SIZE = 80;

/** Reveal history in useful chunks without mounting the entire transcript. */
export const TRANSCRIPT_REVEAL_STEP = 80;

/** Allow one extra chunk before trimming again after returning to the end. */
export const TRANSCRIPT_WINDOW_LIMIT =
  TRANSCRIPT_WINDOW_SIZE + TRANSCRIPT_REVEAL_STEP;

export function initialTranscriptStart(messageCount: number): number {
  return Math.max(0, messageCount - TRANSCRIPT_WINDOW_SIZE);
}

/**
 * Keep a requested start valid after reload, rewind, or new messages. At least
 * the normal tail window remains visible when the transcript shrinks.
 */
export function clampTranscriptStart(
  messageCount: number,
  requestedStart: number
): number {
  return Math.max(
    0,
    Math.min(Math.floor(requestedStart), initialTranscriptStart(messageCount))
  );
}

export function revealEarlierTranscriptStart(currentStart: number): number {
  return Math.max(0, currentStart - TRANSCRIPT_REVEAL_STEP);
}

/** Expand far enough to mount a hidden jump target with a little context. */
export function transcriptStartForTarget(
  currentStart: number,
  targetIndex: number,
  contextMessages = 4
): number {
  if (targetIndex < 0 || targetIndex >= currentStart) return currentStart;
  return Math.max(0, targetIndex - Math.max(0, contextMessages));
}

export function shouldTrimTranscript(
  messageCount: number,
  currentStart: number,
  atEnd: boolean
): boolean {
  return atEnd && messageCount - currentStart > TRANSCRIPT_WINDOW_LIMIT;
}
