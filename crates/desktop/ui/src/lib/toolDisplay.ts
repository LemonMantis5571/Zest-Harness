/**
 * Notes written when a turn dies with a tool still open.
 *
 * The row already says the tool failed; repeating "interrupted" as the path
 * chip *and* the expanded body is the same word three times.
 */
export function displayToolSummary(summary: string | undefined): string | undefined {
  if (!summary?.trim()) return undefined;
  const value = summary.trim();
  if (
    value === "interrupted" ||
    value === "approval interrupted" ||
    value === "tool interrupted"
  ) {
    return undefined;
  }
  return value
    .replace(/ \(approval interrupted\)$/, "")
    .replace(/ \(interrupted\)$/, "");
}

export function wasInterrupted(summary: string | undefined): boolean {
  if (!summary?.trim()) return false;
  const value = summary.trim();
  return (
    value === "interrupted" ||
    value === "approval interrupted" ||
    value === "tool interrupted" ||
    value.endsWith(" (interrupted)") ||
    value.endsWith(" (approval interrupted)")
  );
}
