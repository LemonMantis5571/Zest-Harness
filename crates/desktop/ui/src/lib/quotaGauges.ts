import type { HeadroomView, ProviderQuotaView } from "./types.ts";

/** A single "how much is left" reading, derived only from provider numbers. */
export type QuotaGauge = {
  id: string;
  label: string;
  /** 0–100, already clamped. */
  remainingPercent: number;
  /** Epoch seconds, when the provider said the window resets. */
  resetsAt?: number | null;
};

export type GaugeTone = "critical" | "low" | "healthy";

export function gaugeTone(remainingPercent: number): GaugeTone {
  if (remainingPercent <= 10) return "critical";
  if (remainingPercent <= 25) return "low";
  return "healthy";
}

function clampPercent(value: number): number | null {
  if (!Number.isFinite(value)) return null;
  return Math.max(0, Math.min(100, value));
}

/**
 * Build the gauges a provider row can honestly draw.
 *
 * A bar needs a denominator. Rate-limit windows and percentage headroom carry
 * one; a raw balance such as `4.60 USD` does not, because the provider never
 * says what "full" was. Those rows deliberately get no bar rather than a
 * guessed one — the same rule the panel states in its own footer.
 */
export function quotaGauges(
  quota: ProviderQuotaView | undefined,
  headroom: HeadroomView | undefined
): QuotaGauge[] {
  const gauges: QuotaGauge[] = [];

  if (quota?.kind === "rate_limit") {
    quota.windows.forEach((quotaWindow, index) => {
      const remaining = clampPercent(100 - quotaWindow.usedPercent);
      if (remaining == null) return;
      gauges.push({
        id: `window-${index}-${quotaWindow.label}`,
        label: quotaWindow.label,
        remainingPercent: remaining,
        resetsAt: quotaWindow.resetsAt,
      });
    });

    if (quota.spendLimit) {
      const remaining = clampPercent(quota.spendLimit.remainingPercent);
      if (remaining != null) {
        gauges.push({
          id: "spend-limit",
          label: "Monthly spend",
          remainingPercent: remaining,
          resetsAt: quota.spendLimit.resetsAt,
        });
      }
    }
  }

  if (gauges.length === 0 && headroom?.kind === "provider_reported") {
    if (headroom.quotaUsedPercent != null) {
      const remaining = clampPercent(100 - headroom.quotaUsedPercent);
      if (remaining != null) {
        gauges.push({
          id: "quota-window",
          label: headroom.quotaWindow ?? "Quota",
          remainingPercent: remaining,
          resetsAt: headroom.quotaResetAt,
        });
      }
    }

    // A request budget is only a fraction when the provider sent both halves.
    if (
      headroom.requestsRemaining != null &&
      headroom.requestsLimit != null &&
      headroom.requestsLimit > 0
    ) {
      const remaining = clampPercent(
        (headroom.requestsRemaining / headroom.requestsLimit) * 100
      );
      if (remaining != null) {
        gauges.push({
          id: "requests",
          label: "Requests",
          remainingPercent: remaining,
        });
      }
    }
  }

  return gauges;
}
