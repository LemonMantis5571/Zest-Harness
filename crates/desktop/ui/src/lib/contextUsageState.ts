import { isBusyError } from "./invokeErrors.ts";
import type { ContextUsage } from "./types.ts";

/** The meter's last reading, tied to the chat it was measured for. */
export type ContextReading = { scope: string; usage: ContextUsage } | null;

export type ContextFetch = { ok: true; usage: ContextUsage } | { ok: false; error: unknown };

/**
 * The desktop cannot measure a chat while a turn holds its session, so every
 * fetch during a turn fails with `busy`. Keep the last reading for the same
 * chat instead of blanking the meter; it refreshes when the turn ends. Any
 * other failure, or a reading from a different chat, clears it.
 */
export function settleContextReading(
  previous: ContextReading,
  scope: string,
  result: ContextFetch,
): ContextReading {
  if (result.ok) return { scope, usage: result.usage };
  if (previous?.scope === scope && isBusyError(result.error)) return previous;
  return null;
}
