import type { ProviderQuotaSnapshot } from "./types.ts";

export const PROVIDER_QUOTA_TTL_MS = 5 * 60 * 1000;

export type ProviderQuotaLoadResult =
  | { kind: "fresh"; snapshot: ProviderQuotaSnapshot }
  | { kind: "cached"; snapshot: ProviderQuotaSnapshot }
  | { kind: "error"; snapshot: ProviderQuotaSnapshot | null; error: unknown }
  | { kind: "stale"; snapshot: ProviderQuotaSnapshot | null };

export type ProviderQuotaLoader = {
  getSnapshot(): ProviderQuotaSnapshot | null;
  load(force?: boolean, nowMs?: number): Promise<ProviderQuotaLoadResult>;
};

export function shouldFetchProviderQuota(
  snapshot: ProviderQuotaSnapshot | null,
  nowMs = Date.now(),
  force = false
): boolean {
  return force || snapshot == null || !isProviderQuotaFresh(snapshot, nowMs);
}

export function isProviderQuotaFresh(
  snapshot: ProviderQuotaSnapshot,
  nowMs = Date.now(),
  ttlMs = PROVIDER_QUOTA_TTL_MS
): boolean {
  const checkedAtMs = snapshot.checkedAt * 1000;
  if (!Number.isFinite(checkedAtMs) || checkedAtMs <= 0 || ttlMs <= 0) return false;

  // Treat a small clock skew into the future as fresh instead of immediately
  // rechecking a snapshot that the backend just returned.
  return nowMs - checkedAtMs < ttlMs;
}

export function createProviderQuotaLoader(
  fetchQuota: () => Promise<ProviderQuotaSnapshot>
): ProviderQuotaLoader {
  let snapshot: ProviderQuotaSnapshot | null = null;
  let inFlight: Promise<ProviderQuotaLoadResult> | null = null;
  let generation = 0;

  return {
    getSnapshot: () => snapshot,
    load(force = false, nowMs = Date.now()) {
      if (!force) {
        if (inFlight) return inFlight;
        if (!shouldFetchProviderQuota(snapshot, nowMs)) {
          return Promise.resolve({ kind: "cached", snapshot: snapshot! });
        }
      }

      const requestGeneration = ++generation;
      const request = Promise.resolve()
        .then(fetchQuota)
        .then((next) => {
          if (requestGeneration !== generation) {
            return { kind: "stale", snapshot } satisfies ProviderQuotaLoadResult;
          }
          snapshot = next;
          return { kind: "fresh", snapshot: next } satisfies ProviderQuotaLoadResult;
        })
        .catch((error: unknown) => {
          if (requestGeneration !== generation) {
            return { kind: "stale", snapshot } satisfies ProviderQuotaLoadResult;
          }
          return { kind: "error", snapshot, error } satisfies ProviderQuotaLoadResult;
        })
        .finally(() => {
          if (requestGeneration === generation) inFlight = null;
        });

      inFlight = request;
      return request;
    },
  };
}
