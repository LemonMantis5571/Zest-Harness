import type { EffortId } from "./models.ts";
import { isEffortId } from "./models.ts";
import type { SessionInfo, SessionMeta } from "./types.ts";

export type SessionOptionsSnapshot = {
  model: string;
  effort: EffortId;
};

/** The two values sent by the Reset action as one coherent request. */
export function resetSessionOptions(
  defaults: SessionOptionsSnapshot
): SessionOptionsSnapshot {
  return { model: defaults.model, effort: defaults.effort };
}

/**
 * Apply authoritative model/effort from Rust onto the session already held.
 *
 * Takes metadata rather than a whole session because there is nothing else to
 * take: changing a model does not change the transcript, so Rust no longer
 * sends one. This function used to receive every message and immediately drop
 * them for the copy it already had, which is what made the cost visible.
 *
 * `null` when there is no session to update — a reply about options cannot
 * conjure a conversation that was not open.
 */
export function mergeSessionOptions(
  prev: SessionInfo | null,
  meta: SessionMeta
): SessionInfo | null {
  if (!prev) return null;
  return { ...prev, ...meta };
}

/** Roll back optimistic model/effort after a failed update. */
export function rollbackSessionOptions(
  session: SessionInfo | null,
  snapshot: SessionOptionsSnapshot
): SessionInfo | null {
  if (!session) return null;
  return {
    ...session,
    model: snapshot.model,
    effort: snapshot.effort,
  };
}

export function effortFromSession(effort: string, fallback: EffortId): EffortId {
  return isEffortId(effort) ? effort : fallback;
}
