import type { MeasuredUsage, RangeTotals } from "./types.ts";

type CacheProviderUsage = {
  measured: Pick<MeasuredUsage, "inputTokens" | "cacheReadTokens" | "cacheWriteTokens">;
};

export type CacheMetrics = {
  cachedInputTokens: number;
  cacheWriteTokens: number;
  promptTokens: number;
  /** Share of the prompt the provider served from its cache. */
  hitPercent: number;
  /** Share of the prompt written into the cache for later turns to read. */
  writePercent: number;
  /** Share of the prompt read at full price. */
  freshPercent: number;
  /**
   * Reads per write: how many times the average cached token was reused before
   * it expired. `null` when nothing was ever written, because a ratio against
   * zero is not "no reuse", it is no measurement.
   */
  reuseRatio: number | null;
};

/**
 * Summarise provider-reported prompt caching.
 *
 * The three shares partition the prompt exactly, which is the point: a hit rate
 * on its own counts cache *writes* as failures, so the first turn of a healthy
 * session — nearly all writes — is indistinguishable from a session whose cache
 * never worked. Splitting them apart tells "not caching" from "filling the
 * cache", and `reuseRatio` says whether the filling paid off.
 */
export function cacheMetrics(
  providers: ReadonlyArray<CacheProviderUsage>
): CacheMetrics | null {
  const totals = providers.reduce(
    (sum, provider) => ({
      inputTokens: sum.inputTokens + provider.measured.inputTokens,
      cachedInputTokens: sum.cachedInputTokens + provider.measured.cacheReadTokens,
      cacheWriteTokens: sum.cacheWriteTokens + provider.measured.cacheWriteTokens,
    }),
    { inputTokens: 0, cachedInputTokens: 0, cacheWriteTokens: 0 }
  );
  const promptTokens =
    totals.inputTokens + totals.cachedInputTokens + totals.cacheWriteTokens;

  if (promptTokens <= 0) return null;

  return {
    cachedInputTokens: totals.cachedInputTokens,
    cacheWriteTokens: totals.cacheWriteTokens,
    promptTokens,
    hitPercent: (totals.cachedInputTokens / promptTokens) * 100,
    writePercent: (totals.cacheWriteTokens / promptTokens) * 100,
    freshPercent: (totals.inputTokens / promptTokens) * 100,
    reuseRatio:
      totals.cacheWriteTokens > 0
        ? totals.cachedInputTokens / totals.cacheWriteTokens
        : null,
  };
}

/**
 * One plain sentence for what the cache numbers mean, so the tile does not
 * require the reader to already know the pricing model.
 *
 * The thresholds are the pricing, not taste: a cache write costs 1.25x a fresh
 * read (2x at the hour TTL) and a read costs 0.1x, so break-even sits near 0.3
 * reads per write and anything past a few is comfortably ahead.
 *
 * A missing `reuseRatio` is not the same as a cold cache. Only providers that
 * bill writes separately — Anthropic — report them at all; OpenAI and Codex
 * cache the prefix on their own and report reads only. Reading "no writes" as
 * "nothing cached" would print "every prompt read fresh" directly under a
 * large cache-hit figure on exactly those providers.
 */
export function cacheVerdict(metrics: CacheMetrics | null): string {
  if (!metrics) return "No cache data yet";
  if (metrics.reuseRatio == null) {
    return metrics.cachedInputTokens > 0
      ? "Cached by the provider, which does not report what it wrote"
      : "Nothing cached yet — every prompt read fresh";
  }
  if (metrics.reuseRatio < 0.3) {
    return "Caching is costing more than it saves — prompts are changing before they get reused";
  }
  return `Each cached token was reused ${metrics.reuseRatio.toFixed(1)}x before expiring`;
}

/**
 * One line under the usage-screen cache tile.
 *
 * The window total folds in Claude Code / Codex CLI transcripts. When that
 * mix would hide what Zest itself cached, say so in the same sentence.
 */
export function cacheWindowHint(totals: RangeTotals): string {
  const merged = `${totals.servedFromCachePercent.toFixed(1)}%`;
  const zest = totals.zest;
  if (
    zest &&
    Math.abs(zest.servedFromCachePercent - totals.servedFromCachePercent) >= 0.5
  ) {
    return `${zest.servedFromCachePercent.toFixed(1)}% of Zest prompts · ${merged} including CLI transcripts`;
  }
  return `${merged} of prompt, at a tenth of the price`;
}
