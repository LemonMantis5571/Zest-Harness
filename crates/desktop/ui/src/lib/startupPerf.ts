const PREFIX = "zest-startup";

/**
 * Development-only startup marks. The packaged app does not write console
 * entries or retain PerformanceEntry objects; development builds can inspect
 * the real WebView timing without changing the launch path.
 */
export function markStartup(label: string) {
  if (!import.meta.env.DEV || typeof performance === "undefined") return;
  performance.mark(`${PREFIX}:${label}`);
}

export function measureStartup(label: string, from: string) {
  if (!import.meta.env.DEV || typeof performance === "undefined") return;
  const start = `${PREFIX}:${from}`;
  const end = `${PREFIX}:${label}`;
  try {
    const entry = performance.measure(`${PREFIX}:${label}`, start, end);
    console.debug(`[zest:startup] ${label}: ${Math.round(entry.duration)}ms`);
    performance.clearMarks(end);
    performance.clearMeasures(`${PREFIX}:${label}`);
  } catch {
    // Marks can be cleared by a devtools reload; timing must never affect boot.
  }
}
