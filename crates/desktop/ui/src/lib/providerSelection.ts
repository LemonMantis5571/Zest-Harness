import type { ProviderRow } from "./types";

export type ProviderFailureMemory = (providerId: string) => boolean;

/**
 * A provider is launch-ready only when Rust says both halves are present:
 * authentication and a provider entry that can build a parent runtime.
 */
export function isProviderReady(
  row: ProviderRow,
  failed: ProviderFailureMemory = () => false
): boolean {
  return row.selectable && row.statusKind === "ready" && !failed(row.id);
}

export function pickReadyProvider(
  rows: ProviderRow[],
  prefer: string | null,
  failed: ProviderFailureMemory = () => false
): ProviderRow | null {
  const ready = rows.filter((row) => isProviderReady(row, failed));
  if (prefer) {
    // A remembered provider is an explicit user choice. If it is no longer
    // ready, stop at the picker so the user can repair it or choose another;
    // silently starting a different provider can send messages to the wrong
    // account/project.
    return ready.find((row) => row.id === prefer) ?? null;
  }
  return ready[0] ?? null;
}

/** Keep a useful row selected when no provider can be opened automatically. */
export function pickProviderFallback(
  rows: ProviderRow[],
  prefer: string | null
): ProviderRow | null {
  return (
    (prefer && rows.find((row) => row.id === prefer)) ||
    rows.find((row) => row.selectable && row.statusKind === "unknown") ||
    rows.find((row) => row.selectable) ||
    rows[0] ||
    null
  );
}
