/**
 * Split markdown into top-level blocks so a streaming message can re-parse
 * only its tail.
 *
 * Rendering one growing string means re-parsing the whole document on every
 * frame — O(n) per frame, O(n²) per message, which is what makes a long answer
 * stutter more the longer it gets. Splitting into blocks that are individually
 * memoized makes the total O(n): a settled block is parsed once and never
 * again.
 *
 * The splitter is deliberately **conservative**. Under-splitting only costs a
 * little performance; over-splitting changes what the markdown *means* — a
 * list broken in half renders as two lists, and an indented paragraph orphaned
 * from its list item renders as a code block. Every rule here exists to avoid
 * that, so when in doubt it keeps text together.
 */

export type MarkdownBlock = {
  /** Stable across appends — see `splitBlocks` for why an index is safe. */
  key: number;
  text: string;
};

const FENCE = /^[ \t]{0,3}(`{3,}|~{3,})/;
const LIST_ITEM = /^[ \t]{0,3}([-*+]|\d{1,9}[.)])[ \t]/;

/** A line that continues the block above rather than starting a new one. */
function isContinuation(line: string, inList: boolean): boolean {
  // Indented under a list item: its second paragraph, or nested content.
  // Orphaning this would turn it into an indented code block.
  if (inList && /^[ \t]{2,}\S/.test(line)) return true;
  // Another item of the same (loose) list.
  if (inList && LIST_ITEM.test(line)) return true;
  // Lazy blockquote continuation.
  if (line.startsWith(">")) return false;
  return false;
}

/**
 * Split `text` into renderable blocks.
 *
 * Guarantees the property the memoization depends on: for append-only growth,
 * every block except the last is byte-identical to the block at the same index
 * in the shorter text. So block `key` can be the index — earlier blocks keep
 * both their identity and their content, and React skips them entirely.
 *
 * The one exception is deliberate: while a code fence is open, everything from
 * the fence onward is a single block that grows until the fence closes. It has
 * to re-render, because an unterminated fence is not the same document as a
 * terminated one.
 */
export function splitBlocks(text: string): MarkdownBlock[] {
  if (!text) return [];

  const lines = text.split("\n");
  const blocks: MarkdownBlock[] = [];
  let current: string[] = [];
  let fence: string | null = null;
  let inList = false;

  const flush = () => {
    if (current.length === 0) return;
    const body = current.join("\n");
    if (body.trim()) blocks.push({ key: blocks.length, text: body });
    current = [];
    inList = false;
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];

    if (fence) {
      current.push(line);
      // A closing fence must use the same character and be at least as long.
      const close = FENCE.exec(line);
      if (close && close[1][0] === fence[0] && close[1].length >= fence.length) {
        fence = null;
      }
      continue;
    }

    const open = FENCE.exec(line);
    if (open) {
      // A fence starts its own block, so the settled text before it can be
      // memoized while the code inside is still arriving.
      flush();
      fence = open[1];
      current.push(line);
      continue;
    }

    if (line.trim() === "") {
      // Look ahead: a blank line only ends a block if what follows is
      // genuinely a new one.
      let next = i + 1;
      while (next < lines.length && lines[next].trim() === "") next += 1;
      if (next >= lines.length) {
        // Trailing blanks. Keep them with the current block rather than
        // emitting an empty one — more text may still be coming.
        current.push(line);
        continue;
      }
      if (isContinuation(lines[next], inList)) {
        current.push(line);
        continue;
      }
      flush();
      continue;
    }

    if (LIST_ITEM.test(line)) inList = true;
    current.push(line);
  }

  flush();
  return blocks;
}
