/** A turn must occupy the user's attention long enough to merit a completion notice. */
export const LONG_TURN_NOTIFICATION_MS = 10_000;

export function isLongTurn(durationMs: number) {
  return durationMs >= LONG_TURN_NOTIFICATION_MS;
}

/**
 * Group key for stacked toasts.
 *
 * Errors keep the description so distinct failures stay separate. Success and
 * info group on title: extra copy such as "type to start a new chat" must not
 * fork a second "Chat deleted" card behind the count badge.
 */
export function notificationFingerprint(
  type: string | undefined,
  title: string,
  description: string
) {
  if (type === "success" || type === "info") {
    return JSON.stringify([type, title]);
  }
  return JSON.stringify([type ?? "", title, description]);
}

/** Prefer a non-empty hint when collapsing two success toasts into one card. */
export function mergeToastDescription(existing: string, incoming: string) {
  return incoming || existing;
}

