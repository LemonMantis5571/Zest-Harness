import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { GaugeIcon, RefreshCwIcon } from "lucide-react";

import { TopbarPanel } from "@/components/TopbarPanel";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { getBackend } from "@/lib/backend";
import { createProviderQuotaLoader } from "@/lib/quotaCache";
import { gaugeTone, quotaGauges, type QuotaGauge } from "@/lib/quotaGauges";
import type { ProviderQuotaSnapshot, ProviderRow, UsageSnapshot } from "@/lib/types";
import { cn } from "@/lib/utils";

type Props = {
  providers: ProviderRow[];
  refreshKey: string | number;
};

export function AgentQuotaButton({ providers, refreshKey }: Props) {
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [liveQuota, setLiveQuota] = useState<ProviderQuotaSnapshot | null>(null);
  const [quotaLoading, setQuotaLoading] = useState(false);
  const [quotaError, setQuotaError] = useState<string | null>(null);
  const quotaRequestRef = useRef(0);
  const quotaLoader = useMemo(
    () => createProviderQuotaLoader(() => getBackend().providerQuota()),
    []
  );

  useEffect(() => {
    let live = true;
    const backend = getBackend();
    void backend
      .usageSnapshot()
      .then((next) => {
        if (live) setSnapshot(next);
      })
      .catch(() => {
        if (live) setSnapshot(null);
      });
    return () => {
      live = false;
    };
  }, [refreshKey]);

  const loadQuota = useCallback((force = false) => {
    const requestId = quotaRequestRef.current + 1;
    quotaRequestRef.current = requestId;
    setQuotaLoading(true);
    setQuotaError(null);

    return quotaLoader
      .load(force)
      .then((result) => {
        if (requestId !== quotaRequestRef.current || result.kind === "stale") return;
        if (result.kind === "error") {
          setQuotaError(
            result.snapshot
              ? "Could not refresh provider limits. Showing the last result."
              : "Could not check provider limits."
          );
          return;
        }
        setLiveQuota(result.snapshot);
      })
      .finally(() => {
        if (requestId !== quotaRequestRef.current) return;
        setQuotaLoading(false);
      });
  }, [quotaLoader]);

  const rows = useMemo(() => {
    const ids = providers.length
      ? providers.map((provider) => provider.id)
      : (snapshot?.providers ?? []).map((provider) => provider.providerId);
    return Array.from(new Set(ids)).map((id) => ({
      id,
      label: providers.find((provider) => provider.id === id)?.label ?? id,
      headroom: snapshot?.providers.find((provider) => provider.providerId === id)?.headroom,
      quota: liveQuota?.providers.find((provider) => provider.providerId === id),
    }));
  }, [liveQuota, providers, snapshot]);
  const waitingForQuota = quotaLoading && !liveQuota;

  return (
    <TopbarPanel
      icon={GaugeIcon}
      label="Agent quota"
      onOpenChange={(open) => {
        if (open) void loadQuota();
      }}
    >
      <div className="flex flex-col gap-2.5">
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 className="m-0 text-sm font-semibold">Agent quota</h2>
            <p className="m-0 mt-0.5 text-[11px] text-muted-foreground">
              Real balance or limits from the provider.
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            size="icon-xs"
            title="Refresh quota"
            aria-label="Refresh quota"
            disabled={quotaLoading}
            onClick={() => void loadQuota(true)}
          >
            <RefreshCwIcon
              className={quotaLoading ? "animate-spin" : undefined}
              aria-hidden="true"
            />
          </Button>
        </div>

        {quotaLoading ? (
          <p className="m-0 text-[11px] text-muted-foreground">
            {liveQuota ? "Updating provider limits…" : "Checking provider limits…"}
          </p>
        ) : null}
        {quotaError ? (
          <p role="status" className="m-0 text-[11px] text-amber-300">
            {quotaError}
          </p>
        ) : null}

        {waitingForQuota ? (
          <QuotaSkeleton count={rows.length} />
        ) : rows.length ? (
          <div className="flex flex-col gap-1.5">
            {rows.map((row) => (
              <QuotaRow
                key={row.id}
                label={row.label}
                headroom={row.headroom}
                quota={row.quota}
              />
            ))}
          </div>
        ) : (
          <p className="m-0 rounded-md border border-dashed border-border/70 px-2.5 py-2 text-[11px] text-muted-foreground">
            No providers are configured yet.
          </p>
        )}

        <p className="m-0 border-t border-border/60 pt-2 text-[10px] leading-relaxed text-muted-foreground">
          Zest shows only values returned by the provider. If a provider has no supported quota
          check, that is shown instead of a guessed number.
        </p>
      </div>
    </TopbarPanel>
  );
}

function QuotaSkeleton({ count }: { count: number }) {
  const visibleRows = Math.max(1, Math.min(count, 3));

  return (
    <div
      className="flex flex-col gap-1.5"
      role="status"
      aria-label="Checking provider limits"
      aria-busy="true"
    >
      {Array.from({ length: visibleRows }, (_, index) => (
        <div
          key={index}
          className="rounded-md border border-border/70 bg-secondary/30 px-2.5 py-2"
        >
          <div className="flex items-center justify-between gap-2">
            <Skeleton className="h-3 w-20" />
            <Skeleton className="h-2.5 w-10" />
          </div>
          <Skeleton className="mt-1.5 h-1.5 w-full rounded-full" />
          <Skeleton className="mt-1.5 h-2.5 w-3/5" />
          <Skeleton className="mt-1 h-2.5 w-2/5" />
        </div>
      ))}
    </div>
  );
}

function QuotaRow({
  label,
  headroom,
  quota,
}: {
  label: string;
  headroom: UsageSnapshot["providers"][number]["headroom"] | undefined;
  quota: ProviderQuotaSnapshot["providers"][number] | undefined;
}) {
  const reported = headroom?.kind === "provider_reported" ? headroom : null;
  const balance = quota?.kind === "balance" ? quota : null;
  const rateLimit = quota?.kind === "rate_limit" ? quota : null;
  const requestLine = reported?.requestsRemaining != null
    ? `${reported.requestsRemaining.toLocaleString()} requests left${
        reported.requestsLimit != null ? ` of ${reported.requestsLimit.toLocaleString()}` : ""
      }`
    : null;
  const tokenLine = reported
    ? [
        reported.inputTokensRemaining != null
          ? `${reported.inputTokensRemaining.toLocaleString()} input`
          : null,
        reported.outputTokensRemaining != null
          ? `${reported.outputTokensRemaining.toLocaleString()} output`
          : reported.tokensRemaining != null
            ? `${reported.tokensRemaining.toLocaleString()} tokens`
            : null,
      ]
        .filter(Boolean)
        .join(" · ")
    : "";
  const reset = reported?.requestsReset ?? reported?.tokensReset;
  const gauges = quotaGauges(quota, headroom);

  return (
    <div className="rounded-md border border-border/70 bg-secondary/30 px-2.5 py-2">
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate text-xs font-medium" title={label}>
          {label}
        </span>
        {reported?.ageSecs != null ? (
          <span className="shrink-0 text-[10px] text-muted-foreground">
            {formatAge(reported.ageSecs)} ago
          </span>
        ) : null}
      </div>
      {gauges.length ? (
        <div className="mt-1.5 flex flex-col gap-1.5">
          {gauges.map((gauge) => (
            <QuotaGaugeBar key={gauge.id} gauge={gauge} />
          ))}
        </div>
      ) : null}
      {balance ? (
        <div className="mt-1 space-y-0.5 text-[10px] text-muted-foreground">
          {balance.balances.length ? (
            balance.balances.map((entry) => (
              <div key={entry.currency} className="text-foreground/85">
                {entry.totalBalance} {entry.currency}{" "}
                {balance.available === false ? "reported" : "available"}
              </div>
            ))
          ) : (
            <div className="text-foreground/85">No balance details returned.</div>
          )}
          <div>{balance.detail}</div>
        </div>
      ) : rateLimit ? (
        <div className="mt-1 space-y-0.5 text-[10px] text-muted-foreground">
          {rateLimit.plan ? (
            <div className="text-foreground/85">Plan: {formatPlan(rateLimit.plan)}</div>
          ) : null}
          {/* Windows and spend limit are drawn as bars above. */}
          <div>{rateLimit.detail}</div>
        </div>
      ) : reported ? (
        <div className="mt-1 space-y-0.5 text-[10px] text-muted-foreground">
          {/* A percentage window is drawn as a bar above; only the status-only
              form still needs a line of its own. */}
          {reported.quotaWindow && reported.quotaUsedPercent == null ? (
            <div className="text-foreground/85">
              {formatQuotaWindow(reported.quotaWindow)}
              {reported.quotaStatus ? ": " + formatQuotaStatus(reported.quotaStatus) : ""}
            </div>
          ) : null}
          {reported.quotaResetAt != null && reported.quotaUsedPercent == null ? (
            <div>Resets: {formatResetEpoch(reported.quotaResetAt)}</div>
          ) : null}
          {reported.quotaOverageStatus ? (
            <div>
              Extra use: {formatQuotaStatus(reported.quotaOverageStatus)}
              {reported.quotaOverageResetAt != null
                ? " · resets " + formatResetEpoch(reported.quotaOverageResetAt)
                : ""}
            </div>
          ) : null}
          {reported.quotaIsUsingOverage ? <div>Using extra capacity</div> : null}
          {requestLine ? (
            <div className="text-foreground/85">{requestLine}</div>
          ) : !reported.quotaWindow ? (
            <div className="text-foreground/85">Requests shared by provider</div>
          ) : null}
          {tokenLine ? <div>Tokens: {tokenLine}</div> : null}
          {reset ? <div>Reset: {formatReset(reset)}</div> : null}
          {reported.retryAfterSecs != null ? (
            <div className="text-amber-300">Try again in {formatRetry(reported.retryAfterSecs)}</div>
          ) : null}
        </div>
      ) : (
        <div className="mt-0.5 text-[10px] text-muted-foreground">
          {quota?.detail ?? headroom?.label ?? "No quota data returned."}
        </div>
      )}
    </div>
  );
}

const GAUGE_FILL: Record<ReturnType<typeof gaugeTone>, string> = {
  healthy: "bg-primary",
  low: "bg-amber-500",
  critical: "bg-destructive",
};

const GAUGE_TEXT: Record<ReturnType<typeof gaugeTone>, string> = {
  healthy: "text-foreground/85",
  low: "text-amber-500",
  critical: "text-destructive",
};

/**
 * How much of a limit is still available. Width is a plain percentage of the
 * track, so this renders identically on every platform Zest ships to.
 */
function QuotaGaugeBar({ gauge }: { gauge: QuotaGauge }) {
  const tone = gaugeTone(gauge.remainingPercent);
  const rounded = Math.round(gauge.remainingPercent);

  return (
    <div>
      <div className="flex items-baseline justify-between gap-2 text-[10px]">
        <span className="min-w-0 truncate text-foreground/85" title={gauge.label}>
          {gauge.label}
        </span>
        <span className={cn("shrink-0 font-medium tabular-nums", GAUGE_TEXT[tone])}>
          {rounded}% left
        </span>
      </div>
      <div
        role="progressbar"
        aria-label={`${gauge.label} remaining`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={rounded}
        aria-valuetext={`${rounded}% left`}
        className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-muted"
      >
        <div
          className={cn(
            "h-full rounded-full transition-[width] duration-500 motion-reduce:transition-none",
            GAUGE_FILL[tone]
          )}
          style={{ width: `${gauge.remainingPercent}%` }}
        />
      </div>
      {gauge.resetsAt != null ? (
        <div className="mt-0.5 text-[10px] text-muted-foreground">
          Resets {formatResetEpoch(gauge.resetsAt)}
        </div>
      ) : null}
    </div>
  );
}

function formatAge(secs: number): string {
  if (secs < 60) return `${Math.max(1, secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86_400)}d`;
}

function formatRetry(secs: number): string {
  if (secs < 60) return `${Math.max(1, secs)}s`;
  if (secs < 3600) return `${Math.ceil(secs / 60)}m`;
  return `${Math.ceil(secs / 3600)}h`;
}

function formatReset(value: string): string {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function formatResetEpoch(value: number): string {
  return formatReset(new Date(value * 1000).toISOString());
}

function formatPlan(value: string): string {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function formatQuotaWindow(value: string): string {
  return formatPlan(value.replace(/_/g, " "));
}

function formatQuotaStatus(value: string): string {
  return formatPlan(value.replace(/_/g, " "));
}
