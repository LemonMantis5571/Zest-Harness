/**
 * Highlights a growing document by resuming the grammar after its last
 * complete line. Each call tokenizes only the lines completed since the
 * previous call plus the line still being written, not the whole block, so
 * colouring a streamed code fence costs time proportional to the new text
 * rather than to everything already shown. Adapted from t3code's incremental
 * highlighting.
 *
 * `tokenize(text, state)` highlights `text` (lines joined by "\n") starting
 * from grammar `state`, or from the initial state when it is undefined, and
 * returns one entry per line plus the state after the last one. Returning
 * `null` means it cannot resume here; the caller then takes a full pass.
 */
export function createResumableHighlight<Line, State>(
  tokenize: (
    text: string,
    state: State | undefined
  ) => { lines: Line[]; state: State } | null
): (code: string) => Line[] | null {
  let settled: { prefix: string; state: State; lines: Line[] } | undefined

  return (code) => {
    // A rewrite rather than an append (a regenerated answer, a reset stream)
    // invalidates everything tokenized so far.
    if (settled && !code.startsWith(settled.prefix)) settled = undefined

    const end = code.lastIndexOf("\n") + 1
    const from = settled?.prefix.length ?? 0
    if (end > from) {
      // Stop before the final newline: tokenizing it would add an empty line
      // and advance the grammar once more than the next line expects.
      const next = tokenize(code.slice(from, end - 1), settled?.state)
      if (!next) {
        settled = undefined
        return null
      }
      settled = {
        prefix: code.slice(0, end),
        state: next.state,
        lines: settled ? settled.lines.concat(next.lines) : next.lines,
      }
    }

    // The line still being written is always tokenized again, never kept.
    const tail = tokenize(code.slice(end), settled?.state)
    if (!tail) return null
    return settled ? settled.lines.concat(tail.lines) : tail.lines
  }
}
