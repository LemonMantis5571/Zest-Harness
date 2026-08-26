/**
 * Whether an answer has taken the shape of a document.
 *
 * The command card frames an answer as a document — titled, savable as `.md`,
 * collapsible. That framing is right for a plan and wrong for "which framework
 * do you want?", and since Plan mode tags every turn it produces, something has
 * to tell those apart. Rust cannot: it tags at `assistant_start`, before a
 * single token of the answer exists.
 *
 * **Every rule here is monotonic**, and that is the whole design constraint.
 * Streamed text only ever grows, and each test is a "contains" — so once this
 * returns true it keeps returning true. A predicate that could flip back would
 * wrap and unwrap the card while text streamed, which is exactly the kind of
 * reflow the block splitter exists to avoid.
 */

/** A markdown heading — the clearest signal a reply is structured. */
const HEADING = /^#{1,6}\s/m;

/**
 * An ordered list item. The optional `**` is not pedantry: models routinely
 * write `**1. Fix the lint setup**`, and that is a plan step by any reading.
 */
const ORDERED_ITEM = /^[ \t]{0,3}(\*\*)?\d{1,9}[.)]\s/m;

/**
 * Past this, prose is a document whether or not it is marked up. Set well above
 * a clarifying question and well below a plan; the gateway's first burst is
 * usually larger than this, so a real plan is framed from the first frame
 * rather than popping in a beat later.
 */
const LONG_ENOUGH = 400;

export function looksLikeDocument(text: string): boolean {
  return (
    text.length >= LONG_ENOUGH || HEADING.test(text) || ORDERED_ITEM.test(text)
  );
}
