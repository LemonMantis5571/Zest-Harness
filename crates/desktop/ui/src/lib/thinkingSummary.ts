/**
 * Reducing a growing thinking stream to the one line worth showing.
 *
 * Summarized thinking arrives as a run of `**Title**` blocks, each with a
 * paragraph under it. Rendered in full while a turn runs, twenty of those stack
 * into a column taller than the viewport that pushes the actual answer off
 * screen — so the transcript shows the newest line and keeps the rest behind a
 * disclosure.
 */

/** A `**Bold title**` occupying a whole line — how each step announces itself. */
const TITLE_LINE = /^\s*\*\*(.+?)\*\*\s*$/;

export type ThinkingTraceRow = {
  primary: string;
  secondary?: string;
  kind: "step" | "detail";
};

function lines(thinking: string): string[] {
  return thinking.split("\n").map((line) => line.trim());
}

function normalizeTraceText(value: string): string {
  return value.replace(/\s+/g, " ").replace(/\*\*/g, "").trim();
}

/**
 * Turn the provider's summarized thinking into the small rows used by the
 * disclosure. A title is paired with the paragraph that follows it so the
 * expanded trace feels like a sequence of steps instead of a raw Markdown
 * dump.
 */
export function thinkingTraceRows(thinking: string): ThinkingTraceRow[] {
  const blocks = thinking
    .split(/\n\s*\n/)
    .map((block) => block.trim())
    .filter(Boolean);
  const rows: ThinkingTraceRow[] = [];

  for (let index = 0; index < blocks.length; index += 1) {
    const blockLines = blocks[index]
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
    const firstLine = blockLines[0] ?? "";
    const title = TITLE_LINE.exec(firstLine);

    if (title) {
      let secondary = normalizeTraceText(blockLines.slice(1).join(" "));
      const nextBlock = blocks[index + 1];
      const nextFirstLine =
        nextBlock
          ?.split("\n")
          .map((line) => line.trim())
          .find(Boolean) ?? "";

      if (!secondary && nextBlock && !TITLE_LINE.test(nextFirstLine)) {
        secondary = normalizeTraceText(nextBlock);
        index += 1;
      }

      rows.push({
        primary: normalizeTraceText(title[1]),
        secondary: secondary || undefined,
        kind: "step",
      });
      continue;
    }

    const primary = normalizeTraceText(blocks[index]);
    if (primary) rows.push({ primary, kind: "detail" });
  }

  return rows;
}

/**
 * The newest step title, or the newest non-empty line when there are none.
 *
 * Falls back rather than returning empty: unsummarized providers stream plain
 * prose with no titles at all, and those turns still need something to show.
 */
export function lastThinkingLine(thinking: string): string {
  const all = lines(thinking).filter(Boolean);
  if (all.length === 0) return "";

  for (let i = all.length - 1; i >= 0; i -= 1) {
    const match = TITLE_LINE.exec(all[i]);
    if (match) return match[1].trim();
  }

  // No titles: the tail of the prose is the best available "where we are now".
  // Stripped of emphasis so a half-streamed `**` does not render as literal
  // asterisks on the one line the user actually sees.
  return all[all.length - 1].replace(/\*\*/g, "").trim();
}

/** How many titled steps the stream has produced. Zero for untitled prose. */
export function countThinkingSteps(thinking: string): number {
  return lines(thinking).filter((line) => TITLE_LINE.test(line)).length;
}

/** Label for the collapsed disclosure once a turn has settled. */
export function thinkingSummaryLabel(thinking: string): string {
  const steps = countThinkingSteps(thinking);
  if (steps === 0) return "Thought about this";
  return `Thought through ${steps} step${steps === 1 ? "" : "s"}`;
}

/**
 * One line per step, for a trace that renders prose rather than a checklist.
 *
 * A step's detail is joined onto its title with an em dash instead of being a
 * second row: the line is clamped to two lines of the same height whatever it
 * holds, so a title with nothing under it and one with a paragraph under it
 * occupy the same space and the column keeps its rhythm.
 */
export function thinkingSentences(thinking: string): string[] {
  return thinkingTraceRows(thinking).map((row) =>
    row.secondary ? `${row.primary} — ${row.secondary}` : row.primary
  );
}

/**
 * What the settled disclosure says it did.
 *
 * The measured duration is preferred because it is the honest number, but it
 * only exists for a turn this window watched stream. Reopening a saved chat
 * has the reasoning and not the clock, and inventing a duration there would be
 * worse than describing the steps instead.
 */
export function thoughtForLabel(
  elapsedSeconds: number | null,
  thinking: string
): string {
  if (elapsedSeconds == null) return thinkingSummaryLabel(thinking);
  const seconds = Math.max(1, Math.round(elapsedSeconds));
  if (seconds < 60) return `Thought for ${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return rest === 0
    ? `Thought for ${minutes}m`
    : `Thought for ${minutes}m ${rest}s`;
}
