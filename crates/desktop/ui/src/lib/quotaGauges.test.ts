import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { gaugeTone, quotaGauges } from "./quotaGauges.ts";
import type { HeadroomView, ProviderQuotaView } from "./types.ts";

function rateLimit(overrides: Partial<ProviderQuotaView> = {}): ProviderQuotaView {
  return {
    providerId: "claude",
    kind: "rate_limit",
    detail: "Shared with Claude Desktop and Claude Code.",
    balances: [],
    windows: [],
    ...overrides,
  };
}

describe("quota gauges", () => {
  it("turns used percent into remaining percent per window", () => {
    const gauges = quotaGauges(
      rateLimit({
        windows: [
          { label: "5-hour", usedPercent: 4 },
          { label: "7-day", usedPercent: 0 },
        ],
      }),
      undefined
    );

    assert.deepEqual(
      gauges.map((gauge) => [gauge.label, gauge.remainingPercent]),
      [
        ["5-hour", 96],
        ["7-day", 100],
      ]
    );
  });

  it("includes a monthly spend gauge when the provider sends one", () => {
    const gauges = quotaGauges(
      rateLimit({
        windows: [{ label: "5-hour", usedPercent: 50 }],
        spendLimit: { used: "10", limit: "100", remainingPercent: 90, resetsAt: 42 },
      }),
      undefined
    );

    assert.equal(gauges.length, 2);
    assert.deepEqual(
      [gauges[1]?.label, gauges[1]?.remainingPercent, gauges[1]?.resetsAt],
      ["Monthly spend", 90, 42]
    );
  });

  it("clamps provider values that fall outside 0-100", () => {
    const gauges = quotaGauges(
      rateLimit({
        windows: [
          { label: "over", usedPercent: 140 },
          { label: "under", usedPercent: -20 },
        ],
      }),
      undefined
    );

    assert.deepEqual(
      gauges.map((gauge) => gauge.remainingPercent),
      [0, 100]
    );
  });

  it("skips windows whose percent is not a real number", () => {
    const gauges = quotaGauges(
      rateLimit({ windows: [{ label: "broken", usedPercent: Number.NaN }] }),
      undefined
    );

    assert.deepEqual(gauges, []);
  });

  /**
   * The panel promises it never guesses. A balance has no denominator, so
   * there is nothing honest to draw.
   */
  it("draws no bar for a raw balance", () => {
    const gauges = quotaGauges(
      {
        providerId: "deepseek",
        kind: "balance",
        detail: "Balance reported by DeepSeek.",
        available: true,
        balances: [
          {
            currency: "USD",
            totalBalance: "4.60",
            grantedBalance: "0",
            toppedUpBalance: "4.60",
          },
        ],
        windows: [],
      },
      undefined
    );

    assert.deepEqual(gauges, []);
  });

  it("falls back to reported headroom when no live quota exists", () => {
    const headroom: HeadroomView = {
      kind: "provider_reported",
      label: "provider",
      quotaWindow: "weekly",
      quotaUsedPercent: 30,
      quotaResetAt: 99,
      requestsRemaining: 250,
      requestsLimit: 1000,
    };

    const gauges = quotaGauges(undefined, headroom);

    assert.deepEqual(
      gauges.map((gauge) => [gauge.label, gauge.remainingPercent]),
      [
        ["weekly", 70],
        ["Requests", 25],
      ]
    );
  });

  it("ignores a request count with no limit to divide by", () => {
    const gauges = quotaGauges(undefined, {
      kind: "provider_reported",
      label: "provider",
      requestsRemaining: 250,
      requestsLimit: 0,
    });

    assert.deepEqual(gauges, []);
  });

  it("prefers live quota over stale headroom", () => {
    const gauges = quotaGauges(
      rateLimit({ windows: [{ label: "5-hour", usedPercent: 10 }] }),
      { kind: "provider_reported", label: "provider", quotaUsedPercent: 80 }
    );

    assert.deepEqual(
      gauges.map((gauge) => gauge.remainingPercent),
      [90]
    );
  });

  it("reports nothing for a provider with no quota support", () => {
    assert.deepEqual(quotaGauges(undefined, { kind: "not_reported", label: "n/a" }), []);
    assert.deepEqual(quotaGauges(undefined, undefined), []);
  });

  it("tones the bar by how little is left", () => {
    assert.equal(gaugeTone(100), "healthy");
    assert.equal(gaugeTone(26), "healthy");
    assert.equal(gaugeTone(25), "low");
    assert.equal(gaugeTone(11), "low");
    assert.equal(gaugeTone(10), "critical");
    assert.equal(gaugeTone(0), "critical");
  });
});
