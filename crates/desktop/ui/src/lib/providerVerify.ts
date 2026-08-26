/**
 * What the last gateway probe said about a provider, remembered across launches.
 *
 * Persisted rather than session-scoped so a warm launch can skip verification
 * entirely. Probing costs a real turn against the account, and doing it on every
 * start is what used to make the app sit on "Opening your session…" waiting for
 * the model to answer.
 *
 * Storage failures are always swallowed: an unavailable or full `localStorage`
 * must degrade to "we do not know", never break launch.
 */

const STORAGE_KEY = "zest.providerVerify";

/** How long a verdict is worth trusting. */
export const VERIFY_TTL_MS = 30 * 60 * 1000;

export type VerifyMemory = {
  providerId: string;
  /** Unix ms when the probe finished. */
  at: number;
  ok: boolean;
};

type Stored = Record<string, { at: number; ok: boolean }>;

function readAll(): Stored {
  if (typeof localStorage === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    const out: Stored = {};
    for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
      // Validate rather than trust: a hand-edited or half-written entry should
      // read as "unknown", which re-probes, instead of throwing on every launch.
      if (!value || typeof value !== "object") continue;
      const { at, ok } = value as { at?: unknown; ok?: unknown };
      if (typeof at !== "number" || !Number.isFinite(at) || typeof ok !== "boolean") continue;
      out[id] = { at, ok };
    }
    return out;
  } catch {
    return {};
  }
}

function write(providerId: string, ok: boolean): void {
  if (typeof localStorage === "undefined") return;
  try {
    const all = readAll();
    all[providerId] = { at: Date.now(), ok };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
  } catch {
    /* Full or blocked storage just means the next launch re-probes. */
  }
}

export function markProviderVerified(providerId: string) {
  write(providerId, true);
}

export function markProviderVerifyFailed(providerId: string) {
  write(providerId, false);
}

export function getProviderVerify(providerId: string): VerifyMemory | null {
  const entry = readAll()[providerId];
  return entry ? { providerId, ...entry } : null;
}

/** Recent failed probe — the picker should not pretend Ready. */
export function recentVerifyFailed(providerId: string, maxAgeMs = VERIFY_TTL_MS): boolean {
  const entry = readAll()[providerId];
  if (!entry || entry.ok) return false;
  return Date.now() - entry.at < maxAgeMs;
}

/**
 * Recent successful probe — safe to skip verifying this provider again.
 *
 * The whole point of persisting: a launch that already knows the account works
 * should not spend a turn re-learning it.
 */
export function recentVerifySucceeded(providerId: string, maxAgeMs = VERIFY_TTL_MS): boolean {
  const entry = readAll()[providerId];
  if (!entry || !entry.ok) return false;
  return Date.now() - entry.at < maxAgeMs;
}

/** Drop a verdict, so the next start verifies from scratch. */
export function forgetProviderVerify(providerId: string): void {
  if (typeof localStorage === "undefined") return;
  try {
    const all = readAll();
    delete all[providerId];
    localStorage.setItem(STORAGE_KEY, JSON.stringify(all));
  } catch {
    /* Nothing to do — a stale verdict expires on its own. */
  }
}
