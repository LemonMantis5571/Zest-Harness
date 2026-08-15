const MARKER_MIN_TOP = 8;
const MARKER_BOTTOM_INSET = 28;
const MARKER_SIZE = 24;
const MARKER_GAP = 8;
const MARKER_SEPARATION = MARKER_SIZE + MARKER_GAP;

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

/**
 * Keeps checkpoint hit targets apart when several anchors are outside the
 * visible message viewport and would otherwise be clamped to the same edge.
 */
export function resolveCheckpointMarkerPositions(
  desiredPositions: number[],
  height: number,
): number[] {
  if (desiredPositions.length === 0) return [];

  const maxTop = Math.max(MARKER_MIN_TOP, height - MARKER_BOTTOM_INSET);
  const availableHeight = maxTop - MARKER_MIN_TOP;
  const ordered = desiredPositions
    .map((position, index) => ({
      index,
      position: Number.isFinite(position) ? position : MARKER_MIN_TOP,
    }))
    .sort((left, right) => left.position - right.position || left.index - right.index);
  const separation =
    ordered.length > 1
      ? Math.min(
          MARKER_SEPARATION,
          Math.max(MARKER_SIZE, availableHeight / (ordered.length - 1)),
        )
      : 0;
  const resolved = ordered.map(({ position }) => clamp(position, MARKER_MIN_TOP, maxTop));

  // Push later markers down first, then pull earlier markers back up if the
  // stack reached the bottom edge. This keeps visible markers near their
  // messages while separating markers pinned to either edge.
  for (let index = 1; index < resolved.length; index += 1) {
    const previous = resolved[index - 1];
    const current = resolved[index];
    if (previous !== undefined && current !== undefined) {
      resolved[index] = Math.max(current, previous + separation);
    }
  }
  for (let index = resolved.length - 2; index >= 0; index -= 1) {
    const current = resolved[index];
    const next = resolved[index + 1];
    if (current !== undefined && next !== undefined) {
      resolved[index] = Math.min(current, next - separation);
    }
  }

  const first = resolved[0] ?? MARKER_MIN_TOP;
  const last = resolved[resolved.length - 1] ?? MARKER_MIN_TOP;
  const offset =
    first < MARKER_MIN_TOP
      ? MARKER_MIN_TOP - first
      : last > maxTop
        ? maxTop - last
        : 0;
  const positions = new Array<number>(desiredPositions.length);
  for (const [index, { index: originalIndex }] of ordered.entries()) {
    const position = resolved[index] ?? MARKER_MIN_TOP;
    positions[originalIndex] = clamp(position + offset, MARKER_MIN_TOP, maxTop);
  }
  return positions;
}
