import { ArrowLeftIcon, ArrowRightIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { cacheMetrics, cacheVerdict, type CacheMetrics } from "@/lib/cacheMetrics";
import { UserAvatarButton } from "@/components/UserAvatarButton";
import { getBackend } from "@/lib/backend";
import { cn } from "@/lib/utils";
import type { DayPoint, ProfileStats, UsageSnapshot, UserProfile } from "@/lib/types";

type Props = {
  profile: UserProfile;
  providerLabel?: string | null;
  onBack: () => void;
  onEditProfile: () => void;
  onOpenUsage: () => void;
};

/** Which number the heatmap is colouring. */
type Metric = "activity" | "tokens";

const WEEKS = 27;
const DAY_MS = 86_400_000;

export function ProfileScreen({
  profile,
  providerLabel,
  onBack,
  onEditProfile,
  onOpenUsage,
}: Props) {
  const [stats, setStats] = useState<ProfileStats | null>(null);
  const [usage, setUsage] = useState<UsageSnapshot | null>(null);
  const [skillCount, setSkillCount] = useState<number | null>(null);
  const [metric, setMetric] = useState<Metric>("activity");
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  // Escape returns to chat, matching every other dismissable surface in the app.
  // Safe as a plain document listener because the profile is a whole screen
  // rather than an overlay: nothing is layered above it to swallow the key
  // first, and editing the profile navigates away before opening Settings.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onBack();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onBack]);

  // Move focus onto the screen when it opens. Without this the tab order and
  // the screen reader stay wherever they were in the chat behind it, and the
  // new page is announced to nobody.
  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  useEffect(() => {
    let live = true;
    const backend = getBackend();
    // Settled, not all: a profile is a summary, and one unavailable figure
    // should leave the rest of the page readable.
    void Promise.allSettled([
      backend.profileStats(),
      backend.usageSnapshot(),
      backend.listSkills(),
    ]).then(([statsResult, usageResult, skillsResult]) => {
      if (!live) return;
      if (statsResult.status === "fulfilled") setStats(statsResult.value);
      else setError("Could not load profile activity. Try again.");
      if (usageResult.status === "fulfilled") setUsage(usageResult.value);
      if (skillsResult.status === "fulfilled") setSkillCount(skillsResult.value.length);
    });
    return () => {
      live = false;
    };
  }, []);

  const cells = useMemo(() => buildGrid(stats?.days ?? []), [stats]);
  const hasTokenData = (stats?.peakDayTokens ?? 0) > 0;
  const cache = useMemo(() => cacheMetrics(usage?.providers ?? []), [usage]);

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      role="region"
      aria-label="Your profile"
      className="mx-auto flex w-full max-w-[880px] flex-col gap-7 px-6 py-8 outline-none animate-in fade-in slide-in-from-bottom-2 duration-200"
    >
      <div className="flex items-center">
        <Button type="button" variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeftIcon className="size-3.5" />
          Back to chat
          {/* The shortcut is only accessible if it is discoverable. Hidden from
              screen readers, which get it from the button's own label. */}
          <kbd
            aria-hidden
            className="ml-1.5 rounded border border-border/70 px-1 py-px font-mono text-[10px] leading-none text-muted-foreground"
          >
            Esc
          </kbd>
        </Button>
      </div>

      <header className="flex flex-col items-center gap-3 text-center">
        <UserAvatarButton
          avatarDataUrl={profile.avatarDataUrl}
          displayName={profile.displayName}
          title="Edit profile"
          onClick={onEditProfile}
          className="size-16 rounded-full"
        />
        <div>
          <h1 className="m-0 text-[22px] font-semibold leading-tight tracking-[-0.4px]">
            {profile.displayName.trim() || "Zest"}
          </h1>
          {providerLabel ? (
            <p className="m-0 mt-1 text-[13px] text-muted-foreground">{providerLabel}</p>
          ) : null}
        </div>
      </header>

      {error ? (
        <p className="rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
          {error}
        </p>
      ) : null}

      <StatStrip stats={stats} cache={cache} />

      <section className="flex flex-col gap-3">
        <div className="flex items-baseline justify-between gap-3">
          <h2 className="m-0 text-[13px] font-semibold tracking-[-0.1px]">Activity</h2>
          <div className="flex items-center gap-1" role="group" aria-label="Heatmap metric">
            <MetricTab
              active={metric === "activity"}
              onClick={() => setMetric("activity")}
              label="Chats"
            />
            <MetricTab
              active={metric === "tokens"}
              onClick={() => setMetric("tokens")}
              label="Tokens"
              disabled={!hasTokenData}
              hint={hasTokenData ? undefined : "No metered days yet"}
            />
          </div>
        </div>

        <Heatmap cells={cells} metric={metric} />

        {metric === "tokens" && stats?.meteringSince ? (
          <p className="m-0 text-[11px] text-muted-foreground">
            Token metering began {formatDate(stats.meteringSince)}. Earlier days show chat
            activity only — there is no spend history to backfill.
          </p>
        ) : null}
      </section>

      <div className="grid gap-7 sm:grid-cols-2">
        <Panel title="Activity">
          <Row label="Total chats" value={stats ? stats.totalChats.toLocaleString() : "—"} />
          <Row label="Messages" value={stats ? stats.totalMessages.toLocaleString() : "—"} />
          <Row label="Requests" value={stats ? stats.totalRequests.toLocaleString() : "—"} />
          <Row
            label="Skills available"
            value={skillCount === null ? "—" : String(skillCount)}
          />
          <Row
            label="First used"
            value={stats?.firstActivity ? formatStamp(stats.firstActivity) : "—"}
          />
        </Panel>

        <Panel
          title="Zest tokens by provider"
          action={
            <Button type="button" variant="ghost" size="sm" onClick={onOpenUsage}>
              Full report
              <ArrowRightIcon className="size-3.5" />
            </Button>
          }
        >
          {usage?.providers.length ? (
            usage.providers.map((p) => (
              <Row
                key={p.providerId}
                label={p.providerId}
                value={compact(p.measured.totalTokens)}
              />
            ))
          ) : !usage?.externalWorkers.length ? (
            <p className="m-0 text-[12px] text-muted-foreground">Nothing recorded yet.</p>
          ) : null}
          {usage?.externalWorkers.length ? (
            <>
              <div className="my-2 border-t border-border/60 pt-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                External workers
              </div>
              {usage.externalWorkers.map((worker) => (
                <Row
                  key={worker.workerId}
                  label={`${worker.workerId} (${worker.invocations} ${worker.invocations === 1 ? "run" : "runs"})`}
                  value={
                    worker.reportedTokenTotal == null
                      ? "Not reported"
                      : `${compact(worker.reportedTokenTotal)} reported`
                  }
                />
              ))}
              <p className="m-0 mt-2 text-[11px] leading-relaxed text-muted-foreground">
                Worker figures are reported by the external CLI/ACP process and are not included
                in Zest totals.
              </p>
            </>
          ) : null}
        </Panel>
      </div>
    </div>
  );
}

function StatStrip({
  stats,
  cache,
}: {
  stats: ProfileStats | null;
  cache: CacheMetrics | null;
}) {
  const items = [
    { value: stats ? compact(stats.totalTokens) : "—", label: "Zest tokens" },
    {
      value: cache ? `${cache.hitPercent.toFixed(1)}%` : "—",
      label: "Prompt from cache",
      hint: cacheVerdict(cache),
    },
    {
      value: stats?.peakDayTokens ? compact(stats.peakDayTokens) : "—",
      label: "Busiest day",
    },
    { value: stats ? formatDuration(stats.longestChatSecs) : "—", label: "Longest chat" },
    { value: stats ? `${stats.currentStreakDays}d` : "—", label: "Current streak" },
    { value: stats ? `${stats.longestStreakDays}d` : "—", label: "Longest streak" },
  ];

  return (
    <ul className="m-0 grid list-none grid-cols-2 gap-px overflow-hidden rounded-lg border border-border/70 bg-border/60 p-0 sm:grid-cols-3 lg:grid-cols-6">
      {items.map((item) => (
        <li
          key={item.label}
          title={item.hint ?? undefined}
          tabIndex={item.hint ? 0 : undefined}
          aria-label={item.hint ? `${item.label}: ${item.value}. ${item.hint}` : undefined}
          className={cn(
            "bg-card/60 px-3 py-3 text-center focus-visible:outline-2 focus-visible:outline-primary",
            item.hint ? "cursor-help" : undefined
          )}
        >
          <div className="text-[15px] font-semibold tabular-nums tracking-[-0.2px]">
            {item.value}
          </div>
          <div className="mt-0.5 text-[11px] text-muted-foreground">{item.label}</div>
        </li>
      ))}
    </ul>
  );
}

function MetricTab({
  active,
  onClick,
  label,
  disabled,
  hint,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  disabled?: boolean;
  hint?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={hint}
      // `aria-pressed` is what makes these announce as a toggle rather than two
      // unrelated buttons. A disabled button's `title` is not reliably read, so
      // the reason goes in the accessible name instead.
      aria-pressed={active}
      aria-label={hint ? `${label} — ${hint}` : undefined}
      className={cn(
        "cursor-pointer rounded-md px-2 py-1 text-[11px] font-medium transition-colors",
        active ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
        disabled && "cursor-not-allowed opacity-40 hover:text-muted-foreground"
      )}
    >
      {label}
    </button>
  );
}

type Cell = { date: string; point: DayPoint | null };

function Heatmap({ cells, metric }: { cells: Cell[]; metric: Metric }) {
  const values = useMemo(
    () =>
      cells.map((c) => valueOf(c.point, metric)).filter((v): v is number => v !== null && v > 0),
    [cells, metric]
  );
  const peak = values.length ? Math.max(...values) : 0;

  // A label saying only "a heatmap" tells a screen reader user nothing they can
  // act on. Summarise the figures the sighted reading conveys: how much, spread
  // over how many days, and the best one.
  const noun = metric === "tokens" ? "tokens" : "chats";
  const total = values.reduce((sum, v) => sum + v, 0);
  const summary = values.length
    ? `${compact(total)} ${noun} across ${values.length} active ${
        values.length === 1 ? "day" : "days"
      } in the last ${WEEKS} weeks. Busiest day ${compact(peak)} ${noun}.`
    : `No ${noun} recorded in the last ${WEEKS} weeks.`;

  return (
    <div className="overflow-x-auto">
      <div
        className="grid w-max grid-flow-col gap-[3px]"
        style={{ gridTemplateRows: "repeat(7, minmax(0, 1fr))" }}
        role="img"
        aria-label={summary}
      >
        {cells.map((cell) => (
          <div
            key={cell.date}
            title={describe(cell, metric)}
            className={cn(
              "size-[11px] rounded-[2px] transition-colors",
              levelClass(valueOf(cell.point, metric), peak)
            )}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * Fixed-width grid ending today, laid out column-per-week with weekdays down
 * each column — the shape the reference uses and the one a `grid-flow-col`
 * with seven rows produces naturally.
 *
 * Days are bucketed by the browser's local date, matching how core buckets
 * tokens once `setLocalOffset` has run.
 */
function buildGrid(days: DayPoint[]): Cell[] {
  const byDate = new Map(days.map((d) => [d.date, d]));
  const cells: Cell[] = [];

  const today = new Date();
  today.setHours(0, 0, 0, 0);
  // End on the Saturday of this week so the final column is whole.
  const end = new Date(today.getTime() + (6 - today.getDay()) * DAY_MS);
  const start = new Date(end.getTime() - (WEEKS * 7 - 1) * DAY_MS);

  for (let t = start.getTime(); t <= end.getTime(); t += DAY_MS) {
    const date = localIso(new Date(t));
    cells.push({ date, point: byDate.get(date) ?? null });
  }
  return cells;
}

/** `toISOString` is UTC and would shift the whole grid for anyone west of it. */
function localIso(d: Date): string {
  const month = `${d.getMonth() + 1}`.padStart(2, "0");
  const day = `${d.getDate()}`.padStart(2, "0");
  return `${d.getFullYear()}-${month}-${day}`;
}

function valueOf(point: DayPoint | null, metric: Metric): number | null {
  if (!point) return null;
  if (metric === "tokens") return point.tokens ?? null;
  return point.chats;
}

/**
 * Five steps against the busiest day. Relative rather than absolute because a
 * scale that suits one person's usage is meaningless for another's.
 */
function levelClass(value: number | null, peak: number): string {
  if (value === null) return "bg-muted/25";
  if (value <= 0 || peak <= 0) return "bg-muted/40";
  const ratio = value / peak;
  if (ratio > 0.75) return "bg-primary";
  if (ratio > 0.5) return "bg-primary/75";
  if (ratio > 0.25) return "bg-primary/50";
  return "bg-primary/30";
}

function describe(cell: Cell, metric: Metric): string {
  const when = formatDate(cell.date);
  if (!cell.point) return `${when} — no activity`;
  if (metric === "tokens") {
    return cell.point.tokens === undefined
      ? `${when} — not metered`
      : `${when} — ${compact(cell.point.tokens)} tokens`;
  }
  const { chats, messages } = cell.point;
  return `${when} — ${chats} chat${chats === 1 ? "" : "s"}, ${messages} messages`;
}

function Panel({
  title,
  action,
  children,
}: {
  title: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="mb-2 flex items-center justify-between gap-3">
        <h2 className="m-0 text-[13px] font-semibold tracking-[-0.1px]">{title}</h2>
        {action}
      </div>
      <div className="flex flex-col">{children}</div>
    </section>
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

function compact(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)} K`;
  return String(n);
}

function formatDuration(secs: number): string {
  if (secs <= 0) return "—";
  const hours = Math.floor(secs / 3600);
  const minutes = Math.round((secs % 3600) / 60);
  if (hours === 0) return `${minutes} min`;
  return `${hours} h ${minutes} min`;
}

function formatDate(iso: string): string {
  // Parsed as local parts, not `new Date(iso)` — that reads a bare ISO date as
  // UTC midnight and shows the previous day west of Greenwich.
  const [y, m, d] = iso.split("-").map(Number);
  if (!y || !m || !d) return iso;
  return new Date(y, m - 1, d).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function formatStamp(unixSecs: number): string {
  return new Date(unixSecs * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}
