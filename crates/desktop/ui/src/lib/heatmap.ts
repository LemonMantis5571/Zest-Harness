export type HeatmapTone = 0 | 1 | 2 | 3 | 4 | 5;

/**
 * Empty days (no record) stay at 0. A recorded zero is 1, so the graph can
 * show a quiet day without looking like a missing cell. 2–5 are quartiles of
 * the busiest day in the window.
 *
 * Class names are written out in full so Tailwind can see them.
 */
export const HEATMAP_TONE_CLASS: Record<HeatmapTone, string> = {
  0: "bg-heatmap-0",
  1: "bg-heatmap-1",
  2: "bg-heatmap-2",
  3: "bg-heatmap-3",
  4: "bg-heatmap-4",
  5: "bg-heatmap-5",
};

export function heatmapTone(value: number | null, peak: number): HeatmapTone {
  if (value === null) return 0;
  if (value <= 0 || peak <= 0) return 1;
  const ratio = value / peak;
  if (ratio > 0.75) return 5;
  if (ratio > 0.5) return 4;
  if (ratio > 0.25) return 3;
  return 2;
}
