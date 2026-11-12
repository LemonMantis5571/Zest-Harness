/** Where a user turn started. Composer sends own the draft box. Answers do not. */
export type SendOrigin = "composer" | "answer";

export type SendTurnRequest =
  | { origin: "composer"; text?: string }
  | { origin: "answer"; text: string };

/**
 * Answers come from a questionnaire or a button. They are not the composer
 * draft, so they must not clear it, send its attachments, or write themselves
 * back into the box if the turn fails.
 *
 * Composer sends used to pass only the latest text. `onSend` treated "has a
 * text argument" as "this is an answer", so a normal Enter never cleared the
 * box and dropped attachments.
 */
export function sendOwnsComposer(origin: SendOrigin): boolean {
  return origin === "composer";
}
