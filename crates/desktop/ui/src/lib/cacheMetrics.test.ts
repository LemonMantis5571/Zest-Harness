import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { cacheMetrics, cacheVerdict } from "./cacheMetrics.ts";

describe("cache metrics", () => {
  it("uses the full prompt volume for the hit rate", () => {
    const metrics = cacheMetrics([
      { measured: { inputTokens: 1_000, cacheReadTokens: 9_000, cacheWriteTokens: 500 } },
    ]);

    assert.ok(metrics);
    assert.equal(metrics.promptTokens, 10_500);
    assert.equal(metrics.cachedInputTokens, 9_000);
    assert.equal(metrics.cacheWriteTokens, 500);
    assert.equal(metrics.hitPercent, (9_000 / 10_500) * 100);
  });

  it("splits the prompt into three shares that add up", () => {
    const metrics = cacheMetrics([
      { measured: { inputTokens: 1_000, cacheReadTokens: 9_000, cacheWriteTokens: 500 } },
    ]);

    assert.ok(metrics);
    const sum = metrics.hitPercent + metrics.writePercent + metrics.freshPercent;
    assert.ok(Math.abs(sum - 100) < 1e-9, `shares must partition the prompt, got ${sum}`);
  });

  it("reports reuse as reads per write", () => {
    const metrics = cacheMetrics([
      { measured: { inputTokens: 0, cacheReadTokens: 9_000, cacheWriteTokens: 500 } },
    ]);

    assert.ok(metrics);
    assert.equal(metrics.reuseRatio, 18);
    assert.match(cacheVerdict(metrics), /18\.0x/);
  });

  it("has no reuse figure when nothing was ever written", () => {
    // Not zero: a "0.0x" would read as caching having failed, when in fact
    // nothing was cached to reuse in the first place.
    const metrics = cacheMetrics([
      { measured: { inputTokens: 500, cacheReadTokens: 0, cacheWriteTokens: 0 } },
    ]);

    assert.ok(metrics);
    assert.equal(metrics.reuseRatio, null);
    assert.match(cacheVerdict(metrics), /Nothing cached/);
  });

  it("does not call a provider-side cache 'nothing cached'", () => {
    // OpenAI and Codex cache the prefix themselves and report reads only, so
    // writes stay at zero however well their cache is working. Reading that
    // as a cold cache would print "every prompt read fresh" underneath a
    // large hit rate.
    const metrics = cacheMetrics([
      { measured: { inputTokens: 2_000, cacheReadTokens: 8_000, cacheWriteTokens: 0 } },
    ]);

    assert.ok(metrics);
    assert.equal(metrics.reuseRatio, null);
    assert.equal(metrics.hitPercent, 80);
    assert.doesNotMatch(cacheVerdict(metrics), /Nothing cached|read fresh/);
    assert.match(cacheVerdict(metrics), /Cached by the provider/);
  });

  it("calls out caching that is not paying for itself", () => {
    // Below roughly 0.3 reads per write the 1.25x write premium is never
    // recovered, so the cache is a net cost.
    const metrics = cacheMetrics([
      { measured: { inputTokens: 0, cacheReadTokens: 100, cacheWriteTokens: 1_000 } },
    ]);

    assert.ok(metrics);
    assert.match(cacheVerdict(metrics), /costing more than it saves/);
  });

  it("says so plainly when there is nothing to report", () => {
    assert.match(cacheVerdict(null), /No cache data/);
  });

  it("combines cache usage across providers", () => {
    const metrics = cacheMetrics([
      { measured: { inputTokens: 100, cacheReadTokens: 300, cacheWriteTokens: 0 } },
      { measured: { inputTokens: 200, cacheReadTokens: 0, cacheWriteTokens: 100 } },
    ]);

    assert.ok(metrics);
    assert.equal(metrics.promptTokens, 700);
    assert.equal(metrics.hitPercent, (300 / 700) * 100);
  });

  it("does not show a rate before any prompt tokens are measured", () => {
    assert.equal(
      cacheMetrics([
        { measured: { inputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 } },
      ]),
      null
    );
  });
});
