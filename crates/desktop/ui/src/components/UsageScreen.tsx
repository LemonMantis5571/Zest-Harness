import { ArrowLeftIcon, PencilIcon, RefreshCwIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { getBackend } from "@/lib/backend";
import { ignoreExpectedFailure } from "@/lib/backgroundFailure";
import { modelLabel } from "@/lib/models";
import { cn } from "@/lib/utils";
import type {
  DayCostPoint,
  ModelCostRow,
  RatesStatus,
  UsageReport,
} from "@/lib/types";

type Props = {
  onBack: () => void;
};

/** Windows the range toggle offers, in days. */
const RANGES = [7, 30, 90] as const;
type Range = (typeof RANGES)[number];

/** Which number the chart and the breakdown are showing. */
type Metric = "cost" | "tokens";
type Breakdown = "model" | "day";

/**
 * Chart bands, in the order providers are stacked.
 *
 * Assigned by rank rather than by provider name so the busiest provider always
 * gets the strongest colour, and a provider you have never used cannot claim the
 * one that reads as "primary".
 */
const BAND_COLORS = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)"];

export function UsageScreen({ onBack }: Props) {
  const [range, setRange] = useState<Range>(30);
  const [metric, setMetric] = useState<Metric>("cost");
  const [breakdown, setBreakdown] = useState<Breakdown>("model");
  const [report, setReport] = useState<UsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  /** Bumped by Refresh. One effect owns fetching, whatever triggered it. */
  const [reloadToken, setReloadToken] = useState(0);
  const forceRefreshRef = useRef(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    const backend = getBackend();
    const forceRefresh = forceRefreshRef.current;
    forceRefreshRef.current = false;
    setLoading(true);

    // The report first and on its own: it is a local file read, so the screen
    // fills immediately whatever the network is doing. The rate refresh runs
    // behind it and only costs a second read when the rates actually moved —
    // which, given a 24h cache, is at most once a day.
    void backend
      .usageReport(range)
      .then((next) => {
        if (!live) return;
        setReport(next);
        setError(null);

        return backend
          .refreshRates(forceRefresh)
          .then((rates) => {
            if (!live || rates.fetchedAt === next.rates.fetchedAt) return;
            return backend.usageReport(range).then((repriced) => {
              if (live) setReport(repriced);
            });
          })
          // A failed refresh is not a failed screen. The cached rates are
          // already on it, and their age is already shown.
          .catch((error) => ignoreExpectedFailure(error, "refresh usage rates"));
      })
      .catch(() => {
        if (live) setError("Could not read the usage ledger. Try again.");
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [range, reloadToken]);

  // Escape returns to chat, matching every other dismissable surface.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onBack();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onBack]);

  // Move focus onto the screen so the tab order and the screen reader follow
  // the navigation instead of staying in the chat behind it.
  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  const providerOrder = useMemo(
    () => (report?.providers ?? []).map((p) => p.providerId),
    [report]
  );

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      role="region"
      aria-label="Usage"
      className="mx-auto flex w-full max-w-[1100px] flex-col gap-7 px-6 py-8 outline-none animate-in fade-in slide-in-from-bottom-2 duration-200"
    >
      <div className="flex items-center">
        <Button type="button" variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeftIcon className="size-3.5" />
          Back to chat
          <kbd
            aria-hidden
            className="ml-1.5 rounded border border-border/70 px-1 py-px font-mono text-[10px] leading-none text-muted-foreground"
          >
            Esc
          </kbd>
        </Button>
      </div>

      <header className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="m-0 text-[26px] font-semibold leading-tight tracking-[-0.5px]">
            Usage
          </h1>
          <p className="m-0 mt-1 text-[13px] text-muted-foreground">
            {report ? `${formatDate(report.startDate)} to ${formatDate(report.endDate)}` : " "}
          </p>
        </div>

        <div className="flex items-center gap-2">
          <div
            className="flex items-center gap-px rounded-lg border border-border/70 p-0.5"
            role="group"
            aria-label="Time range"
          >
            {RANGES.map((days) => (
              <Tab
                key={days}
                active={range === days}
                onClick={() => setRange(days)}
                label={`${days} days`}
              />
            ))}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            title="Refresh"
            aria-label="Refresh usage"
            disabled={loading}
            onClick={() => {
              forceRefreshRef.current = true;
              setReloadToken((token) => token + 1);
            }}
          >
            <RefreshCwIcon className={cn("size-3.5", loading && "animate-spin")} />
          </Button>
        </div>
      </header>

      {error ? (
        <p className="rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
          {error}
        </p>
      ) : null}

      {report ? (
        <>
          <div className="grid gap-8 lg:grid-cols-[minmax(0,320px)_minmax(0,1fr)]">
            <Headline report={report} />
            <DailyChart
              series={report.series}
              providerOrder={providerOrder}
              metric={metric}
              onMetricChange={setMetric}
            />
          </div>

          <StatStrip report={report} />

          <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_minmax(0,300px)]">
            <section className="flex min-w-0 flex-col gap-3">
              <div className="flex items-baseline justify-between gap-3">
                <h2 className="m-0 text-[15px] font-semibold tracking-[-0.2px]">Breakdown</h2>
                <div
                  className="flex items-center gap-px rounded-lg border border-border/70 p-0.5"
                  role="group"
                  aria-label="Breakdown grouping"
                >
                  <Tab
                    active={breakdown === "model"}
                    onClick={() => setBreakdown("model")}
                    label="Model"
                  />
                  <Tab
                    active={breakdown === "day"}
                    onClick={() => setBreakdown("day")}
                    label="Day"
                  />
                </div>
              </div>
              {breakdown === "model" ? (
                <ModelTable rows={report.models} providerOrder={providerOrder} />
              ) : (
                <DayTable series={report.series} />
              )}
            </section>

            <div className="flex flex-col gap-8">
              <CostQualityCard report={report} />
              <SourcesCard report={report} />
              {report.externalWorkers.length ? (
                <ExternalWorkers report={report} />
              ) : null}
            </div>
          </div>
        </>
      ) : loading ? (
        <p className="text-[13px] text-muted-foreground">Reading the ledger…</p>
      ) : null}
    </div>
  );
}

/**
 * The headline figure, and immediately underneath it the sentence that says what
 * it is not.
 *
 * The asterisk is load-bearing. Zest has no billing relationship with any
 * provider: this combines local token counts, provider-reported costs, and list
 * rates, and anyone on a subscription is not being charged it.
 */
function Headline({ report }: { report: UsageReport }) {
  const { totals, providers } = report;
  const maxCost = providers.reduce((peak, p) => Math.max(peak, p.costUsd), 0);

  return (
    <section className="flex flex-col gap-5">
      <div>
        <h2 className="m-0 text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
          Known token cost
        </h2>
        <div className="mt-1 text-[34px] font-semibold leading-none tracking-[-1px] tabular-nums">
          {money(totals.costUsd)}
          <span className="text-muted-foreground">*</span>
        </div>
        <p className="m-0 mt-2 text-[11px] leading-relaxed text-muted-foreground">
          * combines provider-reported charges with list-rate estimates where needed. Not a bill
          — Zest does not see your account, and a subscription does not charge this.
        </p>
      </div>

      <div className="flex flex-col gap-3">
        {providers.length ? (
          providers.map((provider, index) => (
            <div key={provider.providerId} className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-3">
                <span className="flex items-center gap-2 text-[13px] font-medium">
                  <span
                    aria-hidden
                    className="size-2 rounded-full"
                    style={{ background: bandColor(index) }}
                  />
                  {provider.providerId}
                </span>
                <span className="text-[13px] font-medium tabular-nums">
                  {money(provider.costUsd)}
                </span>
              </div>
              <div className="h-[3px] overflow-hidden rounded-full bg-muted/40">
                <div
                  className="h-full rounded-full"
                  style={{
                    width: `${maxCost > 0 ? (provider.costUsd / maxCost) * 100 : 0}%`,
                    background: bandColor(index),
                  }}
                />
              </div>
              <p className="m-0 text-[11px] text-muted-foreground">
                {provider.sharePercent.toFixed(1)}% of cost · {compact(provider.tokens)} tokens
              </p>
            </div>
          ))
        ) : (
          <p className="m-0 text-[12px] text-muted-foreground">
            Nothing recorded in this window.
          </p>
        )}
      </div>
    </section>
  );
}

/**
 * Daily spend, stacked by provider.
 *
 * Drawn as straight segments between real days rather than a smoothed curve.
 * Smoothing invents values between the points it joins, and on a chart whose
 * whole purpose is to be trustworthy about spend that is the wrong trade.
 */
function DailyChart({
  series,
  providerOrder,
  metric,
  onMetricChange,
}: {
  series: DayCostPoint[];
  providerOrder: string[];
  metric: Metric;
  onMetricChange: (metric: Metric) => void;
}) {
  const WIDTH = 720;
  const HEIGHT = 190;

  const valueOfDay = (point: DayCostPoint) =>
    metric === "cost" ? point.costUsd : point.tokens;
  const valueOfBand = (point: DayCostPoint, providerId: string) => {
    const band = point.byProvider.find((b) => b.providerId === providerId);
    if (!band) return 0;
    return metric === "cost" ? band.costUsd : band.tokens;
  };

  const peak = Math.max(...series.map(valueOfDay), 0);
  const total = series.reduce((sum, point) => sum + valueOfDay(point), 0);

  const x = (index: number) =>
    series.length <= 1 ? WIDTH / 2 : (index / (series.length - 1)) * WIDTH;
  const y = (value: number) => (peak <= 0 ? HEIGHT : HEIGHT - (value / peak) * HEIGHT);

  // Bottom-up cumulative sums, so each band sits on the one below it.
  const bands = providerOrder.map((providerId, bandIndex) => {
    const lower = series.map((point) =>
      providerOrder
        .slice(0, bandIndex)
        .reduce((sum, id) => sum + valueOfBand(point, id), 0)
    );
    const upper = series.map((point, i) => lower[i] + valueOfBand(point, providerId));
    // Left to right along the top of the band...
    const top = upper.map((value, i) => `${i === 0 ? "M" : "L"}${x(i)},${y(value)}`).join(" ");
    // ...then right to left along the bottom, which closes the ribbon.
    const bottom = lower
      .map((value, i) => `L${x(i)},${y(value)}`)
      .reverse()
      .join(" ");
    return {
      providerId,
      color: bandColor(bandIndex),
      area: `${top} ${bottom} Z`,
      line: top,
    };
  });

  const summary = total
    ? `${metric === "cost" ? money(total) : `${compact(total)} tokens`} over ${series.length} days, peaking at ${
        metric === "cost" ? money(peak) : `${compact(peak)} tokens`
      }.`
    : "Nothing recorded in this window.";

  return (
    <section className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="m-0 text-[15px] font-semibold tracking-[-0.2px]">Daily {metric}</h2>
        <div className="flex items-center gap-3">
          <div
            className="flex items-center gap-px rounded-lg border border-border/70 p-0.5"
            role="group"
            aria-label="Chart metric"
          >
            <Tab
              active={metric === "cost"}
              onClick={() => onMetricChange("cost")}
              label="Cost"
            />
            <Tab
              active={metric === "tokens"}
              onClick={() => onMetricChange("tokens")}
              label="Tokens"
            />
          </div>
          <ul className="m-0 flex list-none items-center gap-3 p-0">
            {bands.map((band) => (
              <li
                key={band.providerId}
                className="flex items-center gap-1.5 text-[11px] text-muted-foreground"
              >
                <span
                  aria-hidden
                  className="size-2 rounded-full"
                  style={{ background: band.color }}
                />
                {band.providerId}
              </li>
            ))}
          </ul>
        </div>
      </div>

      {/* The scale labels are laid out beside the plot rather than inside the
          SVG: the plot stretches to the column width, and text inside it would
          stretch with it. Their own column, so they never sit over the data. */}
      <div className="flex items-stretch gap-2">
        <div
          aria-hidden
          className="flex shrink-0 flex-col justify-between py-px text-right text-[10px] tabular-nums text-muted-foreground"
        >
          {[peak, peak / 2, 0].map((value, index) => (
            <span key={index}>
              {peak <= 0 ? "" : metric === "cost" ? money(value) : compact(value)}
            </span>
          ))}
        </div>

        <div className="relative min-w-0 flex-1">
          <div
            aria-hidden
            className="pointer-events-none absolute inset-0 flex flex-col justify-between"
          >
            {[0, 1, 2].map((index) => (
              <span key={index} className="h-px w-full bg-border/50" />
            ))}
          </div>

          <svg
            viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
            preserveAspectRatio="none"
            className="h-[190px] w-full"
            role="img"
            aria-label={summary}
          >
            {bands.map((band) => (
              <g key={band.providerId}>
                <path d={band.area} fill={band.color} opacity={0.22} />
                <path
                  d={band.line}
                  fill="none"
                  stroke={band.color}
                  strokeWidth={1.5}
                  // Without this the non-uniform scale to the column width would
                  // squash the stroke horizontally into a hairline.
                  vectorEffect="non-scaling-stroke"
                />
              </g>
            ))}
            {/* Invisible per-day columns carry the hover tooltip. A stacked area
                has no shape you can point at for a given day. */}
            {series.map((point, index) => (
              <rect
                key={point.date}
                x={index === 0 ? 0 : x(index) - WIDTH / series.length / 2}
                y={0}
                width={WIDTH / series.length}
                height={HEIGHT}
                fill="transparent"
              >
                <title>{describeDay(point, metric)}</title>
              </rect>
            ))}
          </svg>

          <div className="mt-1 flex justify-between text-[10px] uppercase tracking-wide text-muted-foreground">
            <span>{series.length ? formatDate(series[0].date) : ""}</span>
            <span>{series.length ? formatDate(series[series.length - 1].date) : ""}</span>
          </div>
        </div>
      </div>
    </section>
  );
}

/**
 * The token strip.
 *
 * The prompt is shown as three tiles that add up rather than as one input
 * figure with a hit rate beside it. In an agent loop the three differ by an
 * order of magnitude in both volume and price, and — more importantly — a lone
 * hit rate counts cache writes as failures, so a session busy filling its cache
 * is indistinguishable from one whose cache never worked.
 */
function StatStrip({ report }: { report: UsageReport }) {
  const { totals, quality } = report;
  const items = [
    {
      value: compact(totals.processedTokens),
      label: "Processed tokens",
      hint: `${compact(totals.tokensPerActiveDay)} per active day`,
    },
    {
      value: compact(totals.cachedInputTokens),
      label: "Prompt served from cache",
      hint: `${totals.servedFromCachePercent.toFixed(1)}% of prompt, at a tenth of the price`,
    },
    {
      value: compact(totals.cacheWriteTokens),
      label: "Prompt written to cache",
      hint:
        totals.cacheReuseRatio != null
          ? `reused ${totals.cacheReuseRatio.toFixed(1)}x before expiring`
          : // Only providers that bill writes separately report them; the
            // rest cache the prefix themselves and report reads only.
            totals.cachedInputTokens > 0
            ? "provider caches without reporting writes"
            : "nothing cached yet",
    },
    {
      value: compact(totals.uncachedInputTokens),
      label: "Prompt read fresh",
      hint: `${totals.readFreshPercent.toFixed(1)}% of prompt, at full price`,
    },
    {
      value: compact(totals.outputTokens),
      label: "Output",
      hint: `${totals.requests.toLocaleString()} requests`,
    },
    {
      value: money(totals.cacheSavingsUsd),
      label: "Cache savings",
      hint:
        quality.savingsMultiple != null
          ? `${quality.savingsMultiple.toFixed(1)}x the estimated cost`
          : "no priced traffic yet",
    },
  ];

  return (
    <ul className="m-0 grid list-none grid-cols-2 gap-px overflow-hidden rounded-lg border border-border/70 bg-border/60 p-0 sm:grid-cols-3 lg:grid-cols-6">
      {items.map((item) => (
        <li key={item.label} className="bg-card/60 px-4 py-3">
          <div className="text-[11px] text-muted-foreground">{item.label}</div>
          <div className="mt-1 text-[19px] font-semibold tabular-nums tracking-[-0.4px]">
            {item.value}
          </div>
          <div className="mt-0.5 text-[11px] text-muted-foreground">{item.hint}</div>
        </li>
      ))}
    </ul>
  );
}

function ModelTable({
  rows,
  providerOrder,
}: {
  rows: ModelCostRow[];
  providerOrder: string[];
}) {
  if (!rows.length) {
    return <Empty>No model has been metered in this window.</Empty>;
  }

  return (
    <Table head={["Model", "Cost", "Share", "Tokens"]}>
      {rows.map((row) => (
        <tr key={`${row.providerId}${row.modelId}`} className="border-b border-border/40 last:border-b-0">
          <td className="py-2 pr-3">
            <span className="flex items-center gap-2">
              <span
                aria-hidden
                className="size-2 shrink-0 rounded-full"
                style={{ background: bandColor(providerOrder.indexOf(row.providerId)) }}
              />
              <span className="truncate text-[12px] font-medium">
                {modelLabel(row.modelId)}
              </span>
              <span className="shrink-0 text-[11px] text-muted-foreground">
                {row.providerId}
              </span>
            </span>
          </td>
          <td className="py-2 pr-3 text-right text-[12px] tabular-nums">
            {row.costUsd == null ? (
              // Not "$0.00". The tokens were real and the rate is missing.
              <span
                className="text-muted-foreground"
                title="No published rate for this model. Add one to price it."
              >
                No rate
              </span>
            ) : (
              <span
                // The marker distinguishes provider-reported dollars from
                // estimates, including rows that mix sources or include gaps.
                title={costSourceTitle(row.costSource)}
              >
                {money(row.costUsd)}
                {row.costSource === "providerReported" ? null : (
                  <span className="text-muted-foreground">*</span>
                )}
              </span>
            )}
          </td>
          <td className="py-2 pr-3 text-right text-[12px] tabular-nums text-muted-foreground">
            {row.costUsd == null ? "—" : `${row.sharePercent.toFixed(1)}%`}
          </td>
          <td className="py-2 text-right text-[12px] tabular-nums text-muted-foreground">
            {compact(row.tokens)}
          </td>
        </tr>
      ))}
    </Table>
  );
}

function DayTable({ series }: { series: DayCostPoint[] }) {
  // Newest first, and quiet days dropped: a table of zeroes is scrolling, not
  // information. The chart is where a gap should be visible.
  const active = series.filter((point) => point.requests > 0).reverse();
  if (!active.length) {
    return <Empty>No activity in this window.</Empty>;
  }

  return (
    <Table head={["Day", "Cost", "Requests", "Tokens"]}>
      {active.map((point) => (
        <tr key={point.date} className="border-b border-border/40 last:border-b-0">
          <td className="py-2 pr-3 text-[12px] font-medium">{formatDate(point.date)}</td>
          <td className="py-2 pr-3 text-right text-[12px] tabular-nums">
            {money(point.costUsd)}
          </td>
          <td className="py-2 pr-3 text-right text-[12px] tabular-nums text-muted-foreground">
            {point.requests.toLocaleString()}
          </td>
          <td className="py-2 text-right text-[12px] tabular-nums text-muted-foreground">
            {compact(point.tokens)}
          </td>
        </tr>
      ))}
    </Table>
  );
}

/**
 * How much of the window the headline figure actually covers.
 *
 * This card is the reason the headline is allowed to exist. A total derived from
 * 40% of the tokens and one derived from 99% look identical on their own, and
 * only this says which you are reading.
 */
function CostQualityCard({ report }: { report: UsageReport }) {
  const { quality, pricesPath, rates } = report;
  const [opening, setOpening] = useState(false);

  const rows = [
    { label: "Provider reported", value: quality.providerReportedPercent },
    { label: "Model priced", value: quality.pricedPercent },
    { label: "Unpriced", value: quality.unpricedPercent },
    { label: "Before per-model metering", value: quality.unattributedPercent },
  ].filter((row) => row.value > 0);

  return (
    <section className="flex flex-col gap-3">
      <h2 className="m-0 text-[15px] font-semibold tracking-[-0.2px]">Cost quality</h2>

      <div className="flex flex-col">
        {rows.length ? (
          rows.map((row) => (
            <Row key={row.label} label={row.label} value={`${row.value.toFixed(1)}%`} />
          ))
        ) : (
          <Row label="Model priced" value="—" />
        )}
        <Row label="Cache savings" value={money(quality.cacheSavingsUsd)} />
      </div>

      {quality.unpricedModels.length ? (
        <div className="rounded-lg border border-border/70 bg-card/40 px-3 py-2.5">
          <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
            No rate for{" "}
            <span className="font-medium text-foreground">
              {quality.unpricedModels.join(", ")}
            </span>
            . Their tokens are counted but not costed.
          </p>
        </div>
      ) : null}

      {/* Where the rates came from and how old they are. A cost figure without
          this reads as current even when the machine has been offline a week. */}
      <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
        {rates.catalogModels === 0
          ? "Published rates have not been fetched yet, so only your own overrides can price anything."
          : `Priced against ${rates.catalogModels.toLocaleString()} published rates${
              rates.overrides > 0
                ? `, plus ${rates.overrides} of your own`
                : ""
            }, ${describeRateAge(rates)}.`}
      </p>

      {pricesPath ? (
        <div className="flex flex-col gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={opening}
            onClick={() => {
              setOpening(true);
              void getBackend()
                .openPricesFile()
                .finally(() => setOpening(false));
            }}
          >
            <PencilIcon className="size-3.5" />
            Override a rate
          </Button>
          <p className="m-0 break-all text-[10px] text-muted-foreground">{pricesPath}</p>
        </div>
      ) : null}
    </section>
  );
}

/** "updated 3 hours ago", or why there is no figure to give. */
function describeRateAge(rates: RatesStatus): string {
  if (rates.fetchedAt == null) return "never updated";
  const ageSecs = Math.max(0, Math.floor(Date.now() / 1000) - rates.fetchedAt);
  const stale = rates.stale ? ", refresh due" : "";
  if (ageSecs < 3600) return `updated ${Math.max(1, Math.round(ageSecs / 60))} min ago${stale}`;
  if (ageSecs < 86_400) return `updated ${Math.round(ageSecs / 3600)}h ago${stale}`;
  return `updated ${Math.round(ageSecs / 86_400)}d ago${stale}`;
}

/**
 * Where the numbers came from.
 *
 * Two sources are being added together, and they are not the same kind of fact.
 * Zest's own ledger is exact for turns Zest sent; the transcript scan is a
 * read-back of what the CLIs recorded for turns Zest never saw. Naming both, and
 * saying which directories were read, is what stops the total reading as one
 * seamless measurement it is not.
 */
function SourcesCard({ report }: { report: UsageReport }) {
  const { scan } = report;
  const files = scan.filesScanned + scan.filesCached;

  return (
    <section className="flex flex-col gap-3">
      <h2 className="m-0 text-[15px] font-semibold tracking-[-0.2px]">Sources</h2>
      <div className="flex flex-col">
        <Row label="Metered by Zest" value="this app's own turns" />
        <Row
          label="Read from CLI transcripts"
          value={files ? `${files.toLocaleString()} files` : "none found"}
        />
        {scan.records > 0 ? (
          <Row label="Turns read" value={scan.records.toLocaleString()} />
        ) : null}
        {scan.duplicatesDropped > 0 ? (
          <Row
            label="Repeats dropped"
            value={scan.duplicatesDropped.toLocaleString()}
          />
        ) : null}
        {scan.filesFailed > 0 ? (
          <Row label="Unreadable files" value={scan.filesFailed.toLocaleString()} />
        ) : null}
      </div>
      {scan.roots.length ? (
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {scan.roots.map((root) => (
            <li key={root.path} className="text-[10px] text-muted-foreground">
              <span className="font-medium">{root.providerId}</span>{" "}
              <span className="break-all">{root.path}</span>
              {root.exists ? null : " — not found"}
            </li>
          ))}
        </ul>
      ) : null}
      <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
        Transcripts are read from disk and never leave the machine. Turns you ran in the CLIs
        directly are counted here even though Zest did not send them.
      </p>
    </section>
  );
}

/**
 * Worker figures sit beside the totals and are never added to them: these
 * processes authenticate against their own accounts and report their own
 * numbers, so folding them in would bill someone else's spend to you.
 *
 * Two things make this panel different from everything above it. Its figures are
 * lifetime rather than windowed, because worker usage has no daily buckets to
 * slice — so it says so rather than inheriting the range in the header. And when
 * a worker reports a cost, that cost is *reported*, not estimated from a local
 * price book: it is the only real money figure on this screen, and it is labelled
 * to say which run it belongs to, because workers bill per session and summing
 * them would invent a total nobody quoted.
 */
function ExternalWorkers({ report }: { report: UsageReport }) {
  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-baseline justify-between gap-3">
        <h2 className="m-0 text-[15px] font-semibold tracking-[-0.2px]">External workers</h2>
        <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
          All time
        </span>
      </div>
      <div className="flex flex-col gap-3">
        {report.externalWorkers.map((worker) => (
          <div key={worker.workerId} className="flex flex-col">
            <Row
              label={`${worker.workerId} (${worker.invocations} ${
                worker.invocations === 1 ? "run" : "runs"
              })`}
              value={
                worker.reportedTokenTotal == null
                  ? "Not reported"
                  : `${compact(worker.reportedTokenTotal)} tokens${
                      worker.tokenReports < worker.invocations
                        ? ` (${worker.tokenReports}/${worker.invocations} runs reported)`
                        : ""
                    }`
              }
            />
            {worker.lastCost ? (
              <Row label="Last run, worker-reported" value={reportedCost(worker.lastCost)} />
            ) : null}
          </div>
        ))}
      </div>
      <p className="m-0 text-[11px] leading-relaxed text-muted-foreground">
        Reported by the worker's own CLI, against its own account and its own billing. Never
        added to the totals above — and unlike them, a worker cost is measured rather than
        estimated.
      </p>
    </section>
  );
}

function Table({ head, children }: { head: string[]; children: React.ReactNode }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-left">
        <thead>
          <tr className="border-b border-border/70">
            {head.map((label, index) => (
              <th
                key={label}
                scope="col"
                className={cn(
                  "pb-2 text-[11px] font-medium text-muted-foreground",
                  index === 0 ? "text-left" : "text-right",
                  index < head.length - 1 && "pr-3"
                )}
              >
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}

function Tab({
  active,
  onClick,
  label,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      // `aria-pressed` is what makes these announce as a toggle rather than as
      // several unrelated buttons.
      aria-pressed={active}
      className={cn(
        "cursor-pointer rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors",
        active ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground"
      )}
    >
      {label}
    </button>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border/40 py-1.5 last:border-b-0">
      <span className="text-[12px] text-muted-foreground">{label}</span>
      <span className="text-[12px] font-medium tabular-nums">{value}</span>
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return (
    <p className="m-0 rounded-lg border border-dashed border-border/70 px-3 py-6 text-center text-[12px] text-muted-foreground">
      {children}
    </p>
  );
}

function bandColor(index: number): string {
  if (index < 0) return BAND_COLORS[BAND_COLORS.length - 1];
  return BAND_COLORS[index % BAND_COLORS.length];
}

function costSourceTitle(source: ModelCostRow["costSource"]): string {
  switch (source) {
    case "providerReported":
      return "Reported by the CLI — measured, not estimated";
    case "modelPriced":
      return "Estimated from published rates";
    case "mixed":
      return "Combines provider-reported and estimated or unpriced traffic";
    case "unpriced":
      return "No published rate for this model";
  }
}

function describeDay(point: DayCostPoint, metric: Metric): string {
  const when = formatDate(point.date);
  if (!point.requests) return `${when} — no activity`;
  const headline =
    metric === "cost" ? money(point.costUsd) : `${compact(point.tokens)} tokens`;
  const split = point.byProvider
    .map((band) =>
      metric === "cost"
        ? `${band.providerId} ${money(band.costUsd)}`
        : `${band.providerId} ${compact(band.tokens)}`
    )
    .join(", ");
  return split ? `${when} — ${headline} (${split})` : `${when} — ${headline}`;
}

/**
 * Fixed to USD because the price book is written in it. The locale is the
 * user's, so grouping and separators still follow their machine.
 */
const MONEY = new Intl.NumberFormat(undefined, {
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 2,
});

function money(n: number): string {
  if (!Number.isFinite(n)) return "—";
  // Sub-cent spend rounds to $0.00, which reads as free. Say "under a cent".
  if (n > 0 && n < 0.005) return "<$0.01";
  return MONEY.format(n);
}

/**
 * A cost a worker reported, in the currency it reported.
 *
 * Core keeps the amount as text so the ledger never rounds or assumes a
 * currency. Formatting is a display concern, so it happens here — and falls back
 * to the raw string rather than dropping a figure whose currency code we cannot
 * format. Four decimals because a single delegated run is routinely under a
 * cent, and two would round real spend to nothing.
 */
function reportedCost(cost: { amount: string; currency: string }): string {
  const amount = Number(cost.amount);
  if (!Number.isFinite(amount)) return `${cost.amount} ${cost.currency}`;
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: cost.currency,
      maximumFractionDigits: 4,
    }).format(amount);
  } catch {
    // An unrecognised currency code throws rather than degrading. Show both
    // parts plainly instead of losing the number.
    return `${amount} ${cost.currency}`;
  }
}

function compact(n: number): string {
  if (!Number.isFinite(n)) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}

function formatDate(iso: string): string {
  // Parsed as local parts, not `new Date(iso)` — that reads a bare ISO date as
  // UTC midnight and shows the previous day west of Greenwich.
  const [y, m, d] = iso.split("-").map(Number);
  if (!y || !m || !d) return iso;
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
