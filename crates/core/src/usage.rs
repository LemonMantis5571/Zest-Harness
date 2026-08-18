//! Per-provider and external-worker usage accounting.
//!
//! Two numbers live here and they must never be merged, because they answer
//! different questions and have different reliability:
//!
//! | | Source | Reliability |
//! |---|---|---|
//! | **Spend** | Zest's own metering | Exact for Zest's traffic, blind to every other client on the same account |
//! | **Headroom** | The provider's response headers | Authoritative, but short-window throughput — not subscription quota |
//!
//! Account quota is provider-specific and is not represented by the local
//! ledger. The desktop may show a separate live provider check when an official
//! adapter exists; a figure labelled "remaining" that silently excludes what
//! another client spent is worse than no figure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fsutil;
use crate::pricing::{self, Prices};
use crate::provider::{Completion, RateLimitSnapshot};

/// Days of per-day history to keep. Enough to draw a year-long heatmap with room
/// to spare; old buckets are dropped rather than growing the file forever.
pub const DAILY_RETENTION_DAYS: usize = 400;

/// Minutes east of UTC, for deciding which day a turn belongs to.
///
/// A process global because the ledger is written from deep inside the agent
/// loop, far from anything that knows about the user's clock. Zero (UTC) is the
/// default so the CLI stays deterministic; the desktop sets the real offset at
/// startup, because a streak that resets at 6pm is worse than no streak.
static LOCAL_OFFSET_MINUTES: AtomicI32 = AtomicI32::new(0);

pub fn set_local_offset_minutes(minutes: i32) {
    // Guard against a nonsense value from the front end: real zones span
    // UTC-12..UTC+14.
    if (-12 * 60..=14 * 60).contains(&minutes) {
        LOCAL_OFFSET_MINUTES.store(minutes, Ordering::Relaxed);
    }
}

pub fn local_offset_minutes() -> i32 {
    LOCAL_OFFSET_MINUTES.load(Ordering::Relaxed)
}

/// Serialises tests that move the process-wide offset.
///
/// The offset is a global, so two tests changing it under `cargo test`'s thread
/// pool can read each other's value and fail for reasons unrelated to what they
/// assert. Any test that calls [`set_local_offset_minutes`] must hold this.
#[cfg(test)]
pub(crate) static LOCAL_OFFSET_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `YYYY-MM-DD` for a unix timestamp, in the configured local zone.
///
/// ISO order is lexicographic order, which is why the daily map can be a
/// `BTreeMap<String, _>` and still iterate chronologically.
pub fn day_key(unix_secs: u64) -> String {
    day_key_from_number(local_day_number(unix_secs))
}

/// Days since the epoch, in the configured local zone.
///
/// The form to compute with: "are these two days consecutive" is subtraction on
/// this, and calendar-string arithmetic would be a bug farm.
pub fn local_day_number(unix_secs: u64) -> i64 {
    let shifted = unix_secs as i64 + i64::from(local_offset_minutes()) * 60;
    shifted.div_euclid(86_400)
}

pub fn day_key_from_number(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Parse a `YYYY-MM-DD` key back to a day number. `None` if it is not one.
pub fn day_number_from_key(key: &str) -> Option<i64> {
    let mut parts = key.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    let max_day = match m {
        2 if is_leap_year(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if parts.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > max_day {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Calendar date to days since the epoch. Inverse of [`civil_from_days`].
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64; // March = 0
    let doy = (153 * mp + 2) / 5 + u64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// Days since the unix epoch to a calendar date.
///
/// Hinnant's civil-from-days, valid for any date in the proleptic Gregorian
/// calendar. Written out rather than pulled in: a date crate would be a new
/// dependency for one function, and this one has no configuration to get wrong.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe as i64 + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The five counters every usage bucket carries.
///
/// Its own type because the same five numbers are accumulated per provider, per
/// model, per day, and per model per day. Four hand-written copies of the same
/// addition is how one of them ends up forgetting cache writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
}

impl TokenCounts {
    /// Fold one completed turn in.
    ///
    /// The request is always counted; the tokens only when the provider actually
    /// reported them, so a silent endpoint shows as a request that spent an
    /// unknown amount rather than as a request that spent nothing.
    fn add(&mut self, completion: &Completion) {
        self.requests = self.requests.saturating_add(1);
        if !completion.usage_available {
            return;
        }
        let usage = &completion.usage;
        self.input_tokens = self
            .input_tokens
            .saturating_add(u64::from(usage.input_tokens));
        self.output_tokens = self
            .output_tokens
            .saturating_add(u64::from(usage.output_tokens));
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(u64::from(usage.cache_creation_input_tokens));
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(u64::from(usage.cache_read_input_tokens));
    }

    fn merge(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    /// The prompt half of [`Self::total_tokens`]: fresh input plus both cache
    /// columns.
    ///
    /// Same invariant as [`crate::anthropic::types::Usage::prompt_tokens`] —
    /// one name for one definition, across the two types that carry these
    /// counters. The three shares in [`RangeTotals`] partition exactly this.
    pub fn prompt_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    fn to_pricing(self) -> pricing::Counts {
        pricing::Counts {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_write_tokens: self.cache_write_tokens,
            cache_read_tokens: self.cache_read_tokens,
        }
    }
}

/// One day's measured spend, across every provider.
///
/// The day total is not split per provider, because the question the total
/// answers is "how much did I use Zest that day". `by_model` is a finer split
/// than that on purpose: a model is what a rate attaches to, so it is the only
/// grain from which a cost can be derived, and the provider is recoverable from
/// it because the key carries both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayUsage {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    /// Keyed as [`model_key`]. Absent for days recorded before attribution
    /// landed — those days keep real totals that no model can be assigned, which
    /// the report surfaces as unattributed rather than quietly dropping.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_model: BTreeMap<String, TokenCounts>,
}

impl DayUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    fn totals(&self) -> TokenCounts {
        TokenCounts {
            requests: self.requests,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_write_tokens: self.cache_write_tokens,
            cache_read_tokens: self.cache_read_tokens,
        }
    }

    fn add(&mut self, model_key: &str, completion: &Completion) {
        let mut totals = self.totals();
        totals.add(completion);
        self.requests = totals.requests;
        self.input_tokens = totals.input_tokens;
        self.output_tokens = totals.output_tokens;
        self.cache_write_tokens = totals.cache_write_tokens;
        self.cache_read_tokens = totals.cache_read_tokens;

        self.by_model
            .entry(model_key.to_string())
            .or_default()
            .add(completion);
    }

    /// Fold already-counted tokens in, for a source that is not a live
    /// completion — a CLI transcript read back off disk, for instance.
    ///
    /// The day total and the model bucket move together, so a merged day cannot
    /// end up with attribution that does not add up to its own total.
    pub fn merge_counts(&mut self, model_key: &str, counts: &TokenCounts) {
        self.requests = self.requests.saturating_add(counts.requests);
        self.input_tokens = self.input_tokens.saturating_add(counts.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(counts.output_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(counts.cache_write_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(counts.cache_read_tokens);
        self.by_model
            .entry(model_key.to_string())
            .or_default()
            .merge(counts);
    }

    /// Tokens on this day that no model can be assigned, because they were
    /// recorded before per-model attribution existed.
    fn unattributed_tokens(&self) -> u64 {
        let attributed = self.by_model.values().fold(0u64, |total, counts| {
            total.saturating_add(counts.total_tokens())
        });
        self.total_tokens().saturating_sub(attributed)
    }
}

/// The ledger key for one model, carrying its provider.
///
/// Not `provider + "/" + model` parsed back apart later: model ids legitimately
/// contain slashes (`anthropic/claude-sonnet-4-6` on an aggregator), so the pair
/// is stored on the value rather than recovered from the string.
pub fn model_key(provider_id: &str, model_id: &str) -> String {
    format!("{provider_id}\u{1f}{model_id}")
}

/// Lifetime totals for one model, on one provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModelUsage {
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(flatten)]
    pub counts: TokenCounts,
    #[serde(default)]
    pub first_seen: u64,
    #[serde(default)]
    pub last_seen: u64,
}

/// What Zest itself has spent against one provider, plus the last headroom that
/// provider reported.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    /// Unix seconds. Zero means never used.
    pub first_seen: u64,
    pub last_seen: u64,
    /// Last figures the provider reported about itself. `None` means it reports
    /// nothing — which is information, not zero.
    #[serde(default)]
    pub headroom: Option<RateLimitSnapshot>,
    /// When `headroom` was captured. Throughput limits refill continuously, so a
    /// stale snapshot should be shown with its age rather than as current fact.
    #[serde(default)]
    pub headroom_at: Option<u64>,
}

impl ProviderUsage {
    /// Everything Zest sent and received. Cache reads are counted because they
    /// were still tokens the provider processed, even though they bill lower and
    /// mostly do not count against throughput limits.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_read_tokens)
    }
}

/// Usage a CLI/ACP worker volunteered for one delegated run.
///
/// External workers own their own authentication and billing, so these values
/// must never be folded into Zest's provider ledger. Every field is optional on
/// purpose: a worker can report context size without token counts, or report
/// nothing at all. `None` means unavailable; it is not zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalUsageReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ExternalCost>,
}

impl ExternalUsageReport {
    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.thought_tokens.is_none()
            && self.cached_read_tokens.is_none()
            && self.cached_write_tokens.is_none()
            && self.context_used.is_none()
            && self.context_size.is_none()
            && self.cost.is_none()
    }

    pub fn has_tokens(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.thought_tokens.is_some()
            || self.cached_read_tokens.is_some()
            || self.cached_write_tokens.is_some()
    }

    /// Merge later stream updates over earlier ones. ACP commonly sends
    /// context/cost updates before the final response, while headless CLIs may
    /// put token usage only on their final result envelope.
    pub fn merge(&mut self, newer: &Self) {
        if newer.input_tokens.is_some() {
            self.input_tokens = newer.input_tokens;
        }
        if newer.output_tokens.is_some() {
            self.output_tokens = newer.output_tokens;
        }
        if newer.thought_tokens.is_some() {
            self.thought_tokens = newer.thought_tokens;
        }
        if newer.cached_read_tokens.is_some() {
            self.cached_read_tokens = newer.cached_read_tokens;
        }
        if newer.cached_write_tokens.is_some() {
            self.cached_write_tokens = newer.cached_write_tokens;
        }
        if newer.context_used.is_some() {
            self.context_used = newer.context_used;
        }
        if newer.context_size.is_some() {
            self.context_size = newer.context_size;
        }
        if newer.cost.is_some() {
            self.cost = newer.cost.clone();
        }
    }
}

/// A reported worker cost kept as text so the ledger never rounds or invents a
/// currency. Providers may use different units and decimal precision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCost {
    pub amount: String,
    pub currency: String,
}

/// Lifetime accounting for one configured external worker.
///
/// Token fields are cumulative only across runs that reported that field. The
/// report count makes partial coverage visible instead of presenting an exact
/// looking total when some worker runs were silent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ExternalWorkerUsage {
    pub invocations: u64,
    #[serde(default)]
    pub usage_reports: u64,
    #[serde(default)]
    pub token_reports: u64,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub thought_tokens: Option<u64>,
    #[serde(default)]
    pub cached_read_tokens: Option<u64>,
    #[serde(default)]
    pub cached_write_tokens: Option<u64>,
    /// The most recent context reading. Context is a live window, not a
    /// lifetime total, so summing it would be misleading.
    #[serde(default)]
    pub context_used: Option<u64>,
    #[serde(default)]
    pub context_size: Option<u64>,
    /// The most recent cost reported for a run. Current workers create a fresh
    /// session per delegation, so this is a per-run figure, not an account
    /// balance or subscription total.
    #[serde(default)]
    pub last_cost: Option<ExternalCost>,
    /// Unix seconds. Zero means never used.
    #[serde(default)]
    pub first_seen: u64,
    #[serde(default)]
    pub last_seen: u64,
}

impl ExternalWorkerUsage {
    fn add_optional(total: &mut Option<u64>, value: Option<u64>) {
        if let Some(value) = value {
            *total = Some(total.unwrap_or(0).saturating_add(value));
        }
    }

    fn record(&mut self, report: Option<&ExternalUsageReport>, now: u64) {
        if self.first_seen == 0 {
            self.first_seen = now;
        }
        self.last_seen = now;
        self.invocations = self.invocations.saturating_add(1);

        let Some(report) = report.filter(|report| !report.is_empty()) else {
            return;
        };
        self.usage_reports = self.usage_reports.saturating_add(1);
        if report.has_tokens() {
            self.token_reports = self.token_reports.saturating_add(1);
        }
        Self::add_optional(&mut self.input_tokens, report.input_tokens);
        Self::add_optional(&mut self.output_tokens, report.output_tokens);
        Self::add_optional(&mut self.thought_tokens, report.thought_tokens);
        Self::add_optional(&mut self.cached_read_tokens, report.cached_read_tokens);
        Self::add_optional(&mut self.cached_write_tokens, report.cached_write_tokens);
        if report.context_used.is_some() {
            self.context_used = report.context_used;
        }
        if report.context_size.is_some() {
            self.context_size = report.context_size;
        }
        if report.cost.is_some() {
            self.last_cost = report.cost.clone();
        }
    }

    pub fn reported_token_total(&self) -> Option<u64> {
        let values = [
            self.input_tokens,
            self.output_tokens,
            self.thought_tokens,
            self.cached_read_tokens,
            self.cached_write_tokens,
        ];
        values
            .iter()
            .flatten()
            .copied()
            .reduce(|total, value| total.saturating_add(value))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    providers: BTreeMap<String, ProviderUsage>,
    /// Spend per local day, newest last, capped at [`DAILY_RETENTION_DAYS`].
    ///
    /// Added after the per-provider totals, so an existing ledger simply starts
    /// empty here — there is no history to backfill, and inventing one would be
    /// worse than an honest gap.
    #[serde(default)]
    daily: BTreeMap<String, DayUsage>,
    /// Lifetime totals per model, keyed by [`model_key`].
    ///
    /// Kept alongside the daily buckets rather than derived from them because
    /// daily history is trimmed at [`DAILY_RETENTION_DAYS`] and a lifetime figure
    /// should not shrink when a year-old day is dropped.
    #[serde(default)]
    models: BTreeMap<String, ModelUsage>,
    /// Usage reported by external CLI/ACP workers. Kept separate from
    /// provider spend because those workers own their own accounts.
    #[serde(default)]
    external_workers: BTreeMap<String, ExternalWorkerUsage>,
    /// Where to persist. Not serialized — it is where the file is, not part of it.
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Ledger {
    /// `<data dir>/zest/usage.json`.
    ///
    /// Deliberately outside the project: an account's spend is the same account
    /// whichever repository you happen to be sitting in.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("zest").join("usage.json"))
    }

    /// Load from the default location, or start empty if there isn't one.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_from(path),
            None => Self::default(),
        }
    }

    /// Load from an explicit path.
    ///
    /// A missing or unreadable file yields an empty ledger rather than an error.
    /// Usage accounting must never be the reason a session refuses to start.
    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut ledger = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Ledger>(&raw).ok())
            .unwrap_or_default();
        ledger.path = Some(path);
        ledger
    }

    /// Fold one completed turn into the running totals.
    ///
    /// `model_id` should be what the endpoint actually served, not what was
    /// asked for. A substituted model spends at its own rate, and billing the
    /// requested one would put the cost against a model that never ran.
    ///
    /// Persists immediately, and ignores write failures — losing a usage figure
    /// is not worth failing a turn the user already paid for.
    pub fn record(&mut self, provider_id: &str, model_id: &str, completion: &Completion) {
        let now = now_secs();
        let entry = self.providers.entry(provider_id.to_string()).or_default();

        if entry.first_seen == 0 {
            entry.first_seen = now;
        }
        entry.last_seen = now;
        entry.requests = entry.requests.saturating_add(1);
        if completion.usage_available {
            entry.input_tokens = entry
                .input_tokens
                .saturating_add(u64::from(completion.usage.input_tokens));
            entry.output_tokens = entry
                .output_tokens
                .saturating_add(u64::from(completion.usage.output_tokens));
            entry.cache_write_tokens = entry
                .cache_write_tokens
                .saturating_add(u64::from(completion.usage.cache_creation_input_tokens));
            entry.cache_read_tokens = entry
                .cache_read_tokens
                .saturating_add(u64::from(completion.usage.cache_read_input_tokens));
        }

        // Only overwrite when the provider actually reported something, so a
        // gateway turn doesn't erase a real reading from a native one.
        if let Some(limits) = &completion.limits {
            entry.headroom = Some(limits.clone());
            entry.headroom_at = Some(now);
        }

        let key = model_key(provider_id, model_id);
        let model = self.models.entry(key.clone()).or_default();
        if model.first_seen == 0 {
            model.first_seen = now;
            model.provider_id = provider_id.to_string();
            model.model_id = model_id.to_string();
        }
        model.last_seen = now;
        model.counts.add(completion);

        self.daily
            .entry(day_key(now))
            .or_default()
            .add(&key, completion);
        self.trim_daily();

        let _ = self.save();
    }

    /// Record one completed external-worker invocation without pretending that
    /// the parent provider paid for it. A missing report is still a real run;
    /// only the token/context/cost fields remain unavailable.
    pub fn record_external(&mut self, worker_id: &str, report: Option<&ExternalUsageReport>) {
        let now = now_secs();
        self.external_workers
            .entry(worker_id.to_string())
            .or_default()
            .record(report, now);
        let _ = self.save();
    }

    /// Drop the oldest buckets past the retention window.
    ///
    /// Keys are ISO dates, so `BTreeMap` order is chronological and the oldest
    /// are simply the first ones.
    fn trim_daily(&mut self) {
        while self.daily.len() > DAILY_RETENTION_DAYS {
            let Some(oldest) = self.daily.keys().next().cloned() else {
                break;
            };
            self.daily.remove(&oldest);
        }
    }

    /// Per-day spend, keyed by ISO date so iteration is chronological. Empty for
    /// a ledger written before daily buckets existed, until the next turn.
    pub fn daily(&self) -> &BTreeMap<String, DayUsage> {
        &self.daily
    }

    /// Lifetime totals across every provider, for the headline figures.
    pub fn lifetime(&self) -> (u64, u64) {
        self.providers
            .values()
            .fold((0, 0), |(tokens, requests), p| {
                (
                    tokens.saturating_add(p.total_tokens()),
                    requests.saturating_add(p.requests),
                )
            })
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        fsutil::atomic_write_json(path, self)
    }

    /// Reload spend totals from disk (doctor / external writers).
    pub fn reload_from_disk(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let reloaded = Self::load_from(path);
        self.providers = reloaded.providers;
        self.daily = reloaded.daily;
        self.models = reloaded.models;
        self.external_workers = reloaded.external_workers;
    }

    pub fn get(&self, provider_id: &str) -> Option<&ProviderUsage> {
        self.providers.get(provider_id)
    }

    /// Every provider with recorded spend, alphabetically.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &ProviderUsage)> {
        self.providers.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.external_workers.is_empty()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Snapshot for UI/CLI: measured spend vs provider-reported headroom, never merged.
    pub fn snapshot(&self) -> UsageSnapshot {
        let providers = self
            .providers
            .iter()
            .map(|(id, usage)| ProviderUsageView::from_entry(id, usage))
            .collect();
        let external_workers = self
            .external_workers
            .iter()
            .map(|(id, usage)| ExternalWorkerUsageView::from_entry(id, usage))
            .collect();
        UsageSnapshot {
            providers,
            external_workers,
        }
    }

    /// Everything the usage screen draws, for the last `days` local days.
    ///
    /// Computed here rather than in the front end so the CLI and the desktop
    /// agree by construction, and so the one piece of arithmetic that can
    /// mislead — turning tokens into dollars — happens in exactly one place.
    ///
    /// Days with no traffic are emitted as zeroes rather than skipped: a gap in
    /// a time series should read as a quiet day, and a chart that closes the gap
    /// silently rescales the week around it.
    /// `scan` folds in usage read back from the coding CLIs' own transcripts.
    /// Those turns are traffic Zest never sent, so they add to the totals rather
    /// than duplicating them — the ledger and the transcripts describe disjoint
    /// requests, and the provider ids (`codex` vs `codex-cli`) keep which is
    /// which visible on screen.
    pub fn report(
        &self,
        days: u32,
        prices: &Prices,
        scan: Option<&crate::transcripts::ScanResult>,
    ) -> UsageReport {
        let today = local_day_number(now_secs());
        let span = i64::from(days.max(1));
        let first = today - (span - 1);

        let mut series = Vec::with_capacity(span as usize);
        let mut model_totals: BTreeMap<String, ModelReportTotals> = BTreeMap::new();
        let mut totals = TokenCounts::default();
        let mut unattributed_tokens = 0u64;
        let mut active_days = 0u32;
        let mut cost_usd = 0.0;
        let mut cache_savings_usd = 0.0;

        for day in first..=today {
            let date = day_key_from_number(day);

            // The two sources are merged per day rather than reported side by
            // side, so one chart, one set of totals, and one place where the
            // per-day arithmetic can be wrong.
            let usage = match (self.daily.get(&date), scan.and_then(|s| s.daily.get(&date))) {
                (None, None) => {
                    series.push(DayCostPoint {
                        date,
                        ..Default::default()
                    });
                    continue;
                }
                (Some(ledger), None) => ledger.clone(),
                (None, Some(scanned)) => scanned.clone(),
                (Some(ledger), Some(scanned)) => {
                    let mut merged = ledger.clone();
                    for (key, counts) in &scanned.by_model {
                        merged.merge_counts(key, counts);
                    }
                    merged
                }
            };
            let reported_today = scan.and_then(|s| s.reported_cost.get(&date));

            totals.merge(&usage.totals());
            unattributed_tokens = unattributed_tokens.saturating_add(usage.unattributed_tokens());
            if usage.requests > 0 {
                active_days = active_days.saturating_add(1);
            }

            let mut day_cost = 0.0;
            let mut by_provider: BTreeMap<String, ProviderDayPoint> = BTreeMap::new();
            for (key, counts) in &usage.by_model {
                let (provider_id, model_id) = split_model_key(key);
                // A cost the CLI actually reported beats one derived from a rate
                // table, every time: one is what was charged, the other is what
                // a public list price says it might have been.
                let reported = reported_today
                    .and_then(|m| m.get(key))
                    .copied()
                    .filter(|cost| valid_cost(*cost));
                let (model_cost, source) = match reported {
                    Some(reported) => (Some(reported), CostSource::ProviderReported),
                    None => match prices.price(provider_id, model_id, &counts.to_pricing()) {
                        Some(priced)
                            if valid_cost(priced.cost_usd)
                                && valid_cost(priced.cache_saving_usd) =>
                        {
                            cache_savings_usd += priced.cache_saving_usd;
                            (Some(priced.cost_usd), CostSource::ModelPriced)
                        }
                        _ => (None, CostSource::Unpriced),
                    },
                };

                let model = model_totals.entry(key.clone()).or_default();
                model.counts.merge(counts);
                let tokens = counts.total_tokens();
                match source {
                    CostSource::ProviderReported => {
                        model.provider_reported = true;
                        model.provider_reported_tokens =
                            model.provider_reported_tokens.saturating_add(tokens);
                    }
                    CostSource::ModelPriced => {
                        model.model_priced = true;
                        model.model_priced_tokens =
                            model.model_priced_tokens.saturating_add(tokens);
                    }
                    CostSource::Unpriced | CostSource::Mixed => {
                        model.unpriced = true;
                        model.unpriced_tokens = model.unpriced_tokens.saturating_add(tokens);
                    }
                }
                if let Some(cost) = model_cost {
                    model.cost_usd += cost;
                    model.cost_known = true;
                    day_cost += cost;
                }

                let bucket = by_provider
                    .entry(provider_id.to_string())
                    .or_insert_with(|| ProviderDayPoint {
                        provider_id: provider_id.to_string(),
                        cost_usd: 0.0,
                        tokens: 0,
                    });
                bucket.cost_usd += model_cost.unwrap_or(0.0);
                bucket.tokens = bucket.tokens.saturating_add(counts.total_tokens());
            }
            cost_usd += day_cost;

            series.push(DayCostPoint {
                date,
                cost_usd: day_cost,
                tokens: usage.total_tokens(),
                requests: usage.requests,
                by_provider: by_provider.into_values().collect(),
            });
        }

        // Coverage is measured against the whole window, so the figures add up
        // to what was actually metered — including the tokens no model owns.
        let mut reported_tokens = 0u64;
        let mut priced_tokens = 0u64;
        let mut unpriced_tokens = 0u64;
        let mut unpriced_models = Vec::new();
        let mut models = Vec::new();
        let mut providers: BTreeMap<String, ProviderCostRow> = BTreeMap::new();

        for (key, totals) in model_totals {
            let (provider_id, model_id) = split_model_key(&key);
            let tokens = totals.counts.total_tokens();
            let model_cost = totals.cost_known.then_some(totals.cost_usd);
            let source = totals.source();

            reported_tokens = reported_tokens.saturating_add(totals.provider_reported_tokens);
            priced_tokens = priced_tokens.saturating_add(totals.model_priced_tokens);
            unpriced_tokens = unpriced_tokens.saturating_add(totals.unpriced_tokens);
            if totals.unpriced && !unpriced_models.iter().any(|m| m == model_id) {
                unpriced_models.push(model_id.to_string());
            }

            let row = providers
                .entry(provider_id.to_string())
                .or_insert_with(|| ProviderCostRow {
                    provider_id: provider_id.to_string(),
                    cost_usd: 0.0,
                    tokens: 0,
                    share_percent: 0.0,
                });
            row.cost_usd += model_cost.unwrap_or(0.0);
            row.tokens = row.tokens.saturating_add(tokens);

            models.push(ModelCostRow {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                cost_usd: model_cost,
                cost_source: source,
                share_percent: 0.0,
                requests: totals.counts.requests,
                tokens,
                input_tokens: totals.counts.input_tokens,
                output_tokens: totals.counts.output_tokens,
                cache_write_tokens: totals.counts.cache_write_tokens,
                cache_read_tokens: totals.counts.cache_read_tokens,
            });
        }

        for model in &mut models {
            model.share_percent = percent_of(model.cost_usd.unwrap_or(0.0), cost_usd);
        }
        // Priced models first, biggest spend at the top; unpriced fall to the
        // bottom ordered by volume, where they read as a coverage gap rather
        // than as the cheapest thing on the list.
        models.sort_by(|a, b| {
            b.cost_usd
                .unwrap_or(-1.0)
                .partial_cmp(&a.cost_usd.unwrap_or(-1.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.tokens.cmp(&a.tokens))
        });

        let mut providers: Vec<ProviderCostRow> = providers.into_values().collect();
        for provider in &mut providers {
            provider.share_percent = percent_of(provider.cost_usd, cost_usd);
        }
        providers.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.tokens.cmp(&a.tokens))
        });

        let metered = totals.total_tokens();
        // The full prompt is fresh input + cache reads + cache writes, and the
        // three shares below partition exactly that.
        let observed_prompt = totals.prompt_tokens();
        let served_from_cache_percent =
            percent_of(totals.cache_read_tokens as f64, observed_prompt as f64);

        UsageReport {
            days: days.max(1),
            start_date: day_key_from_number(first),
            end_date: day_key_from_number(today),
            totals: RangeTotals {
                cost_usd,
                requests: totals.requests,
                processed_tokens: metered,
                uncached_input_tokens: totals.input_tokens,
                cached_input_tokens: totals.cache_read_tokens,
                cache_write_tokens: totals.cache_write_tokens,
                output_tokens: totals.output_tokens,
                cache_savings_usd,
                active_days,
                tokens_per_active_day: if active_days == 0 {
                    0
                } else {
                    metered / u64::from(active_days)
                },
                served_from_cache_percent,
                written_to_cache_percent: percent_of(
                    totals.cache_write_tokens as f64,
                    observed_prompt as f64,
                ),
                read_fresh_percent: percent_of(totals.input_tokens as f64, observed_prompt as f64),
                cache_reuse_ratio: (totals.cache_write_tokens > 0)
                    .then(|| totals.cache_read_tokens as f64 / totals.cache_write_tokens as f64),
                cache_hit_percent: served_from_cache_percent,
                unattributed_tokens,
            },
            series,
            providers,
            models,
            quality: CostQuality {
                provider_reported_percent: percent_of(reported_tokens as f64, metered as f64),
                priced_percent: percent_of(priced_tokens as f64, metered as f64),
                unpriced_percent: percent_of(unpriced_tokens as f64, metered as f64),
                unattributed_percent: percent_of(unattributed_tokens as f64, metered as f64),
                unpriced_models,
                cache_savings_usd,
                savings_multiple: if cost_usd > 0.0 {
                    Some(cache_savings_usd / cost_usd)
                } else {
                    None
                },
            },
            scan: scan.map(|s| s.status.clone()).unwrap_or_default(),
            external_workers: self
                .external_workers
                .iter()
                .map(|(id, usage)| ExternalWorkerUsageView::from_entry(id, usage))
                .collect(),
            prices_path: prices.path().map(|p| p.display().to_string()),
            rates: RatesStatus::of(prices),
        }
    }
}

/// Recover the provider and model from a [`model_key`].
///
/// Safe to split because the separator is US (`\x1f`), which no provider or
/// model id contains — the reason the key does not simply use a slash, since
/// model ids on aggregators legitimately do.
fn split_model_key(key: &str) -> (&str, &str) {
    key.split_once('\u{1f}').unwrap_or(("unknown", key))
}

/// Percentage, with an empty denominator reported as zero rather than NaN.
fn percent_of(part: f64, whole: f64) -> f64 {
    if whole <= 0.0 {
        0.0
    } else {
        part / whole * 100.0
    }
}

fn valid_cost(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

/// Honest usage projection: Zest metering and provider headroom stay separate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub providers: Vec<ProviderUsageView>,
    #[serde(default)]
    pub external_workers: Vec<ExternalWorkerUsageView>,
}

/// Everything the usage screen draws for one time window.
///
/// Cost figures use provider-reported values where available and local rates for
/// the remainder. See [`crate::pricing`] for what derived values do and do not
/// claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub days: u32,
    pub start_date: String,
    pub end_date: String,
    pub totals: RangeTotals,
    /// One entry per day in the window, oldest first, including quiet days.
    pub series: Vec<DayCostPoint>,
    pub providers: Vec<ProviderCostRow>,
    pub models: Vec<ModelCostRow>,
    pub quality: CostQuality,
    /// Shown beside the chart, never added to it: workers bill their own
    /// accounts and report their own figures.
    pub external_workers: Vec<ExternalWorkerUsageView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prices_path: Option<String>,
    pub rates: RatesStatus,
    /// How the CLI-transcript scan went. All zeroes when no scan was supplied,
    /// which the UI reads as "not enabled" rather than "found nothing".
    pub scan: crate::transcripts::ScanStatus,
}

/// Where a cost figure came from, in descending order of authority.
///
/// The distinction is the whole reason the usage screen can be trusted: a number
/// the CLI itself recorded and a number multiplied out of a public rate table
/// look identical once they are both dollars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostSource {
    /// The CLI recorded what it was charged.
    ProviderReported,
    /// Computed from the rate table.
    ModelPriced,
    /// This model used more than one source in the selected window, or was
    /// priced on some days and unpriced on others.
    Mixed,
    /// No rate for this model. The tokens are real; the cost is unknown.
    Unpriced,
}

/// Where the rates behind this report came from, and how old they are.
///
/// Travels with every report because a cost figure and the age of the rates
/// behind it are one fact, not two. A screen that shows the money without the
/// date implies the rates are current, which after a week offline they are not.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesStatus {
    /// Models in the published catalogue. Zero means it has never been fetched,
    /// so only local overrides can price anything.
    pub catalog_models: usize,
    /// Rates the user has set by hand, which outrank the catalogue.
    pub overrides: usize,
    /// Unix seconds of the last successful fetch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    /// Whether a refresh is due. Not an error — stale rates still price.
    pub stale: bool,
    pub source_url: String,
}

impl RatesStatus {
    fn of(prices: &Prices) -> Self {
        let catalog = prices.catalog();
        Self {
            catalog_models: catalog.len(),
            overrides: prices.models.len(),
            fetched_at: catalog.fetched_at(),
            stale: catalog.is_stale(),
            source_url: crate::rates::DEFAULT_RATES_URL.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeTotals {
    /// Traffic with a known cost, either provider-reported or model-priced. Read
    /// with [`CostQuality`] beside it: a large number over thin coverage is a
    /// small number wearing a hat.
    pub cost_usd: f64,
    pub requests: u64,
    pub processed_tokens: u64,
    /// Input the provider actually had to read. Cache reads are counted
    /// separately because they bill at a fraction of it.
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub cache_savings_usd: f64,
    pub active_days: u32,
    pub tokens_per_active_day: u64,
    /// Where every prompt token went, as three shares that add up to 100%.
    ///
    /// One number could not answer the question people actually ask. A hit rate
    /// alone counts cache *writes* as failures, but a write is the price of the
    /// next read — the first turn of a healthy session is nearly all writes and
    /// looks identical to a session whose cache never worked at all. Splitting
    /// the denominator out separates "the cache is not being used" from "the
    /// cache is being filled".
    pub served_from_cache_percent: f64,
    pub written_to_cache_percent: f64,
    pub read_fresh_percent: f64,
    /// Cache reads per cache write: how many times the average cached token was
    /// reused before it expired. This is the number that says whether caching
    /// paid off, because it is the one the pricing turns on — a write costs
    /// 1.25x a fresh read (2x at the hour TTL) and a read costs 0.1x, so
    /// anything above roughly 0.3 is already cheaper than not caching. `None`
    /// when nothing was ever written, where a ratio would be division by zero
    /// dressed as a fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_reuse_ratio: Option<f64>,
    /// Retained under its original name for existing readers; identical to
    /// [`Self::served_from_cache_percent`].
    pub cache_hit_percent: f64,
    /// Metered before per-model attribution existed, so unpriceable. Real
    /// tokens, and visible as such rather than dropped from the totals.
    pub unattributed_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayCostPoint {
    pub date: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub requests: u64,
    #[serde(default)]
    pub by_provider: Vec<ProviderDayPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDayPoint {
    pub provider_id: String,
    pub cost_usd: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCostRow {
    pub provider_id: String,
    pub cost_usd: f64,
    pub tokens: u64,
    pub share_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostRow {
    pub provider_id: String,
    pub model_id: String,
    /// `None` when nothing could price this model. Not zero — the tokens were
    /// real and the cost is unknown.
    pub cost_usd: Option<f64>,
    pub cost_source: CostSource,
    pub share_percent: f64,
    pub requests: u64,
    pub tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
}

/// How much of the window the cost figure actually covers.
///
/// The point of the usage screen is not the headline dollar amount; it is this,
/// sitting next to it. A total derived from 40% of the tokens is a different
/// claim from one derived from 99%, and the difference has to be on screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostQuality {
    /// Share of tokens whose cost the CLI itself recorded. The only part of the
    /// total that is measured rather than estimated.
    pub provider_reported_percent: f64,
    pub priced_percent: f64,
    pub unpriced_percent: f64,
    pub unattributed_percent: f64,
    /// Models the book has no rate for, so the UI can name what to add.
    pub unpriced_models: Vec<String>,
    pub cache_savings_usd: f64,
    /// Savings as a multiple of what was actually spent. `None` when nothing
    /// priced, because a ratio against zero is not a large number, it is no
    /// number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub savings_multiple: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageView {
    pub provider_id: String,
    /// Exact for Zest's own traffic — label as "Measured by Zest".
    pub measured: MeasuredUsage,
    /// Authoritative short-window throughput when present — never a subscription balance.
    pub headroom: HeadroomView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasuredUsage {
    pub label: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum HeadroomView {
    /// Provider reported throughput headroom; `age_secs` is how stale the reading is.
    ProviderReported {
        label: String,
        age_secs: Option<u64>,
        requests_limit: Option<u64>,
        requests_remaining: Option<u64>,
        requests_reset: Option<String>,
        tokens_limit: Option<u64>,
        tokens_remaining: Option<u64>,
        input_tokens_remaining: Option<u64>,
        output_tokens_remaining: Option<u64>,
        tokens_reset: Option<String>,
        retry_after_secs: Option<u64>,
        quota_window: Option<String>,
        quota_status: Option<String>,
        quota_used_percent: Option<f64>,
        quota_reset_at: Option<u64>,
        quota_overage_status: Option<String>,
        quota_overage_reset_at: Option<u64>,
        quota_is_using_overage: Option<bool>,
    },
    NotReported {
        label: String,
    },
}

impl ProviderUsageView {
    fn from_entry(provider_id: &str, usage: &ProviderUsage) -> Self {
        let measured = MeasuredUsage {
            label: "Measured by Zest".into(),
            requests: usage.requests,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            total_tokens: usage.total_tokens(),
        };
        let headroom = match &usage.headroom {
            Some(h) if !h.is_empty() => {
                let age_secs = usage.headroom_at.map(|at| now_secs().saturating_sub(at));
                HeadroomView::ProviderReported {
                    label: "Provider reported".into(),
                    age_secs,
                    requests_limit: h.requests_limit,
                    requests_remaining: h.requests_remaining,
                    requests_reset: h.requests_reset.clone(),
                    tokens_limit: h.tokens_limit,
                    tokens_remaining: h.tokens_remaining,
                    input_tokens_remaining: h.input_tokens_remaining,
                    output_tokens_remaining: h.output_tokens_remaining,
                    tokens_reset: h.tokens_reset.clone(),
                    retry_after_secs: h.retry_after_secs,
                    quota_window: h.quota_window.clone(),
                    quota_status: h.quota_status.clone(),
                    quota_used_percent: h.quota_used_percent,
                    quota_reset_at: h.quota_reset_at,
                    quota_overage_status: h.quota_overage_status.clone(),
                    quota_overage_reset_at: h.quota_overage_reset_at,
                    quota_is_using_overage: h.quota_is_using_overage,
                }
            }
            _ => HeadroomView::NotReported {
                label: "Not reported".into(),
            },
        };
        Self {
            provider_id: provider_id.to_string(),
            measured,
            headroom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalWorkerUsageView {
    pub worker_id: String,
    pub invocations: u64,
    pub usage_reports: u64,
    pub token_reports: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub reported_token_total: Option<u64>,
    pub context_used: Option<u64>,
    pub context_size: Option<u64>,
    pub last_cost: Option<ExternalCost>,
    pub last_seen: u64,
}

impl ExternalWorkerUsageView {
    fn from_entry(worker_id: &str, usage: &ExternalWorkerUsage) -> Self {
        Self {
            worker_id: worker_id.to_string(),
            invocations: usage.invocations,
            usage_reports: usage.usage_reports,
            token_reports: usage.token_reports,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            thought_tokens: usage.thought_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            cached_write_tokens: usage.cached_write_tokens,
            reported_token_total: usage.reported_token_total(),
            context_used: usage.context_used,
            context_size: usage.context_size,
            last_cost: usage.last_cost.clone(),
            last_seen: usage.last_seen,
        }
    }
}

#[derive(Debug, Default)]
struct ModelReportTotals {
    counts: TokenCounts,
    cost_usd: f64,
    cost_known: bool,
    provider_reported_tokens: u64,
    model_priced_tokens: u64,
    unpriced_tokens: u64,
    provider_reported: bool,
    model_priced: bool,
    unpriced: bool,
}

impl ModelReportTotals {
    fn source(&self) -> CostSource {
        let known_sources = [self.provider_reported, self.model_priced]
            .into_iter()
            .filter(|present| *present)
            .count();
        if self.unpriced || known_sources > 1 {
            CostSource::Mixed
        } else if self.provider_reported {
            CostSource::ProviderReported
        } else if self.model_priced {
            CostSource::ModelPriced
        } else {
            CostSource::Unpriced
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::Usage;

    fn completion(input: u32, output: u32, limits: Option<RateLimitSnapshot>) -> Completion {
        Completion {
            content: vec![],
            stop_reason: Some("end_turn".into()),
            usage: Usage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            usage_available: true,
            limits,
            served_model: None,
            provider_session: None,
        }
    }

    #[test]
    fn accumulates_across_turns() {
        let mut ledger = Ledger::default();
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(100, 20, None));
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(50, 10, None));

        let usage = ledger.get("anthropic").expect("recorded");
        assert_eq!(usage.requests, 2);
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.total_tokens(), 180);
    }

    #[test]
    fn keeps_providers_separate() {
        let mut ledger = Ledger::default();
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(100, 20, None));
        ledger.record("codex", "gpt-5.6-sol", &completion(7, 3, None));

        assert_eq!(ledger.get("anthropic").unwrap().input_tokens, 100);
        assert_eq!(ledger.get("codex").unwrap().input_tokens, 7);
        assert_eq!(ledger.entries().count(), 2);
    }

    #[test]
    fn a_silent_provider_does_not_erase_a_real_reading() {
        let mut ledger = Ledger::default();

        ledger.record(
            "anthropic",
            "claude-sonnet-4-6",
            &completion(
                10,
                5,
                Some(RateLimitSnapshot {
                    requests_remaining: Some(3914),
                    ..Default::default()
                }),
            ),
        );
        // A turn through a gateway reports nothing. The earlier reading must survive.
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(10, 5, None));

        let usage = ledger.get("anthropic").unwrap();
        assert_eq!(
            usage.headroom.as_ref().unwrap().requests_remaining,
            Some(3914)
        );
        assert_eq!(usage.requests, 2, "spend still accumulated");
    }

    #[test]
    fn absent_headroom_stays_absent() {
        let mut ledger = Ledger::default();
        ledger.record("codex", "gpt-5.6-sol", &completion(10, 5, None));

        // None must not be flattened to a fabricated zero anywhere.
        assert!(ledger.get("codex").unwrap().headroom.is_none());
        assert!(ledger.get("codex").unwrap().headroom_at.is_none());
    }

    #[test]
    fn survives_a_save_load_round_trip() {
        let dir = std::env::temp_dir().join("zest-ledger-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("usage.json");

        let mut ledger = Ledger::load_from(&path);
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(100, 20, None));
        ledger.save().expect("write");

        let reloaded = Ledger::load_from(&path);
        assert_eq!(reloaded.get("anthropic").unwrap().input_tokens, 100);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_starts_empty_instead_of_failing() {
        let dir = std::env::temp_dir().join("zest-ledger-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.json");
        std::fs::write(&path, "{ this is not json").unwrap();

        let ledger = Ledger::load_from(&path);
        assert!(ledger.is_empty());
        // ...and remains writable, so the next turn repairs it.
        assert!(ledger.path().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_keeps_measured_and_headroom_separate() {
        let mut ledger = Ledger::default();
        ledger.record(
            "codex",
            "gpt-5.6-sol",
            &completion(
                10,
                5,
                Some(RateLimitSnapshot {
                    requests_remaining: Some(9),
                    ..Default::default()
                }),
            ),
        );
        ledger.record("anthropic", "claude-sonnet-4-6", &completion(1, 1, None));

        let snap = ledger.snapshot();
        assert_eq!(snap.providers.len(), 2);
        let codex = snap
            .providers
            .iter()
            .find(|p| p.provider_id == "codex")
            .unwrap();
        assert_eq!(codex.measured.label, "Measured by Zest");
        assert_eq!(codex.measured.requests, 1);
        match &codex.headroom {
            HeadroomView::ProviderReported {
                label,
                requests_remaining,
                ..
            } => {
                assert_eq!(label, "Provider reported");
                assert_eq!(*requests_remaining, Some(9));
            }
            other => panic!("expected reported headroom, got {other:?}"),
        }
        let anth = snap
            .providers
            .iter()
            .find(|p| p.provider_id == "anthropic")
            .unwrap();
        match &anth.headroom {
            HeadroomView::NotReported { label } => assert_eq!(label, "Not reported"),
            other => panic!("expected not reported, got {other:?}"),
        }
    }

    #[test]
    fn external_usage_keeps_silent_runs_and_reported_tokens_distinct() {
        let mut ledger = Ledger::default();
        let report = ExternalUsageReport {
            input_tokens: Some(120),
            output_tokens: Some(30),
            context_used: Some(800),
            context_size: Some(16_000),
            cost: Some(ExternalCost {
                amount: "0.0042".into(),
                currency: "USD".into(),
            }),
            ..Default::default()
        };

        ledger.record_external("claude", Some(&report));
        ledger.record_external("claude", None);

        let usage = ledger
            .external_workers
            .values()
            .next()
            .expect("worker usage");
        assert_eq!(usage.invocations, 2);
        assert_eq!(usage.usage_reports, 1);
        assert_eq!(usage.token_reports, 1);
        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.reported_token_total(), Some(150));
        assert_eq!(usage.context_used, Some(800));
        assert_eq!(usage.last_cost.as_ref().unwrap().amount, "0.0042");

        let worker = ledger
            .snapshot()
            .external_workers
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(worker.invocations, 2);
        assert_eq!(worker.token_reports, 1);
        assert_eq!(worker.reported_token_total, Some(150));
    }

    #[test]
    fn old_ledgers_load_without_external_worker_data() {
        let raw = r#"{"providers":{},"daily":{}}"#;
        let ledger: Ledger = serde_json::from_str(raw).unwrap();
        assert!(ledger.external_workers.is_empty());
        assert!(ledger.snapshot().external_workers.is_empty());
    }
}

#[cfg(test)]
mod daily_tests {
    use super::*;

    /// Dates the algorithm is most likely to get wrong: epoch, leap days, and
    /// century rules. Checked against known-good calendar values.
    #[test]
    fn civil_dates_are_correct_at_the_awkward_boundaries() {
        for (days, expected) in [
            (0i64, (1970, 1, 1)),
            (-1, (1969, 12, 31)),
            (59, (1970, 3, 1)),
            // 2000 was a leap year (divisible by 400); 1900 was not.
            (11016, (2000, 2, 29)),
            (11017, (2000, 3, 1)),
            // 2100 is not a leap year, so Feb 28 is followed by Mar 1.
            (47540, (2100, 2, 28)),
            (47541, (2100, 3, 1)),
            (19723, (2024, 1, 1)),
            (20543, (2026, 3, 31)),
        ] {
            assert_eq!(civil_from_days(days), expected, "days={days}");
        }
    }

    #[test]
    fn day_keys_sort_chronologically() {
        // The retention trim and the heatmap both rely on this.
        let mut keys = vec![
            day_key(1_760_000_000),
            day_key(1_700_000_000),
            day_key(1_780_000_000),
        ];
        let original = keys.clone();
        keys.sort();
        assert_eq!(keys[0], original[1]);
        assert_eq!(keys[2], original[2]);
    }

    #[test]
    fn the_local_offset_decides_which_day_a_turn_lands_on() {
        let _guard = LOCAL_OFFSET_TEST_LOCK.lock();
        // 2026-01-01T02:00:00Z is still 2025-12-31 in UTC-6. A user's late
        // evening belongs to their day, not to tomorrow.
        let two_am_utc = 1_767_232_800;
        set_local_offset_minutes(0);
        assert_eq!(day_key(two_am_utc), "2026-01-01");
        set_local_offset_minutes(-6 * 60);
        assert_eq!(day_key(two_am_utc), "2025-12-31");
        set_local_offset_minutes(0);
    }

    #[test]
    fn an_absurd_offset_is_refused() {
        let _guard = LOCAL_OFFSET_TEST_LOCK.lock();
        set_local_offset_minutes(0);
        set_local_offset_minutes(99_999);
        assert_eq!(local_offset_minutes(), 0, "kept the sane value");
    }

    #[test]
    fn daily_history_is_capped() {
        let mut ledger = Ledger::default();
        for day in 0..(DAILY_RETENTION_DAYS + 25) {
            ledger
                .daily
                .insert(format!("2020-01-{day:05}"), DayUsage::default());
        }
        ledger.trim_daily();
        assert_eq!(ledger.daily.len(), DAILY_RETENTION_DAYS);
        // The oldest went, not the newest.
        assert!(ledger
            .daily
            .contains_key(&format!("2020-01-{:05}", DAILY_RETENTION_DAYS + 24)));
        assert!(!ledger.daily.contains_key("2020-01-00000"));
    }

    #[test]
    fn recording_a_turn_fills_todays_bucket() {
        let mut ledger = Ledger::default();
        ledger.record("codex", "gpt-5.6-sol", &completion_with(10, 4));
        ledger.record("codex", "gpt-5.6-sol", &completion_with(6, 1));

        let days: Vec<_> = ledger.daily().values().collect();
        assert_eq!(days.len(), 1, "same day, one bucket");
        let usage = days[0];
        assert_eq!(usage.requests, 2);
        assert_eq!(usage.total_tokens(), 21);
        // The lifetime totals still agree with the day.
        assert_eq!(ledger.get("codex").unwrap().total_tokens(), 21);
    }

    /// A ledger written before daily buckets existed must still load.
    #[test]
    fn an_older_ledger_loads_with_no_daily_history() {
        let raw = r#"{"providers":{"codex":{"requests":3,"input_tokens":10,"output_tokens":5,
            "cache_write_tokens":0,"cache_read_tokens":0,"first_seen":1,"last_seen":2}}}"#;
        let ledger: Ledger = serde_json::from_str(raw).unwrap();
        assert_eq!(ledger.get("codex").unwrap().requests, 3);
        assert!(ledger.daily().is_empty());
        assert_eq!(ledger.lifetime(), (15, 3));
    }

    /// A price book backed by a fixed one-model catalogue.
    ///
    /// Explicit rather than `Prices::load()`: these tests assert exact dollar
    /// amounts, and reading the machine's real cached rates would make them pass
    /// or fail on whatever a provider charged this week.
    fn priced() -> Prices {
        Prices::default().with_catalog(crate::rates::RateCatalog::for_test([(
            "claude-sonnet-4-6",
            crate::pricing::ModelPrice::new(3.0, 15.0, 3.75, 0.3),
        )]))
    }

    #[test]
    fn a_days_models_add_up_to_that_days_total() {
        let mut ledger = Ledger::default();
        ledger.record("codex", "gpt-5.6-sol", &completion_with(10, 4));
        ledger.record("codex", "gpt-5.6-terra", &completion_with(6, 1));

        let day = ledger.daily().values().next().expect("today");
        assert_eq!(day.total_tokens(), 21);
        assert_eq!(day.by_model.len(), 2, "one bucket per model");
        let attributed: u64 = day.by_model.values().map(|c| c.total_tokens()).sum();
        assert_eq!(attributed, day.total_tokens());
        assert_eq!(day.unattributed_tokens(), 0);
    }

    #[test]
    fn the_same_model_on_two_providers_stays_two_rows() {
        // They authenticate separately and can bill at different rates, so
        // collapsing them would make one account's spend unreadable.
        let mut ledger = Ledger::default();
        ledger.record("anthropic", "claude-sonnet-4-6", &completion_with(10, 5));
        ledger.record("gateway", "claude-sonnet-4-6", &completion_with(10, 5));

        assert_eq!(ledger.models.len(), 2);
        let report = ledger.report(7, &Prices::default(), None);
        assert_eq!(report.models.len(), 2);
        assert_eq!(report.providers.len(), 2);
    }

    #[test]
    fn an_unpriced_model_reports_no_cost_rather_than_no_spend() {
        let mut ledger = Ledger::default();
        // One million output tokens on a model the book has never heard of.
        ledger.record("codex", "gpt-nonexistent", &completion_with(0, 1_000_000));

        let report = ledger.report(7, &priced(), None);
        let row = &report.models[0];
        assert_eq!(row.cost_usd, None, "unknown rate is not a zero rate");
        assert_eq!(row.tokens, 1_000_000, "the tokens are still counted");
        assert_eq!(report.totals.cost_usd, 0.0);
        assert_eq!(report.quality.priced_percent, 0.0);
        assert_eq!(report.quality.unpriced_percent, 100.0);
        assert_eq!(report.quality.unpriced_models, vec!["gpt-nonexistent"]);
        // No priced spend means the savings ratio is undefined, not infinite.
        assert_eq!(report.quality.savings_multiple, None);
    }

    #[test]
    fn a_priced_model_costs_what_the_book_says() {
        let mut ledger = Ledger::default();
        ledger.record(
            "anthropic",
            "claude-sonnet-4-6",
            &completion_with(1_000_000, 1_000_000),
        );

        let report = ledger.report(7, &priced(), None);
        // 1M in at $3 plus 1M out at $15.
        assert!((report.totals.cost_usd - 18.0).abs() < 1e-9, "{report:?}");
        assert_eq!(report.models[0].share_percent, 100.0);
        assert_eq!(report.quality.priced_percent, 100.0);
    }

    #[test]
    fn a_partial_provider_report_keeps_daily_and_model_costs_consistent() {
        let today = local_day_number(now_secs());
        let model = model_key("claude-cli", "claude-sonnet-4-6");
        let counts = TokenCounts {
            requests: 1,
            input_tokens: 1_000_000,
            ..Default::default()
        };
        let mut scan = crate::transcripts::ScanResult::default();
        scan.daily
            .entry(day_key_from_number(today - 1))
            .or_default()
            .merge_counts(&model, &counts);
        scan.daily
            .entry(day_key_from_number(today))
            .or_default()
            .merge_counts(&model, &counts);
        scan.reported_cost
            .entry(day_key_from_number(today))
            .or_default()
            .insert(model, 4.0);

        let report = Ledger::default().report(2, &priced(), Some(&scan));
        let row = report.models.first().expect("model row");
        assert_eq!(row.cost_source, CostSource::Mixed);
        assert_eq!(row.cost_usd, Some(7.0));
        assert_eq!(report.providers[0].cost_usd, 7.0);
        assert_eq!(report.totals.cost_usd, 7.0);
        assert_eq!(row.share_percent, 100.0);
        assert_eq!(report.quality.provider_reported_percent, 50.0);
        assert_eq!(report.quality.priced_percent, 50.0);
        assert_eq!(report.quality.unpriced_percent, 0.0);
    }

    #[test]
    fn days_before_attribution_are_reported_as_unattributed_not_dropped() {
        // The shape an existing install has: real day totals, no model split.
        let mut ledger = Ledger::default();
        let today = day_key(now_secs());
        ledger.daily.insert(
            today,
            DayUsage {
                requests: 3,
                input_tokens: 700,
                output_tokens: 300,
                ..Default::default()
            },
        );

        let report = ledger.report(7, &Prices::default(), None);
        assert_eq!(report.totals.processed_tokens, 1_000, "totals are intact");
        assert_eq!(report.totals.unattributed_tokens, 1_000);
        assert_eq!(report.quality.unattributed_percent, 100.0);
        assert!(report.models.is_empty(), "nothing to attribute them to");
    }

    #[test]
    fn the_series_covers_every_day_in_the_window_including_quiet_ones() {
        let mut ledger = Ledger::default();
        ledger.record("codex", "gpt-5.6-sol", &completion_with(10, 4));

        let report = ledger.report(30, &Prices::default(), None);
        assert_eq!(
            report.series.len(),
            30,
            "a gap is a quiet day, not a missing one"
        );
        assert_eq!(report.series.last().unwrap().date, report.end_date);
        assert_eq!(report.series[0].date, report.start_date);
        assert_eq!(report.totals.active_days, 1);
        // Every quiet day is present and empty rather than absent.
        assert!(report.series[..29].iter().all(|d| d.requests == 0));
    }

    #[test]
    fn a_day_outside_the_window_is_excluded() {
        let mut ledger = Ledger::default();
        let long_ago = day_key_from_number(local_day_number(now_secs()) - 40);
        ledger.daily.insert(
            long_ago,
            DayUsage {
                requests: 9,
                input_tokens: 9_000,
                ..Default::default()
            },
        );
        ledger.record("codex", "gpt-5.6-sol", &completion_with(10, 4));

        let week = ledger.report(7, &Prices::default(), None);
        assert_eq!(week.totals.requests, 1);
        let quarter = ledger.report(90, &Prices::default(), None);
        assert_eq!(quarter.totals.requests, 10);
    }

    #[test]
    fn cache_reads_are_reported_apart_from_fresh_input() {
        let mut ledger = Ledger::default();
        ledger.record(
            "anthropic",
            "claude-sonnet-4-6",
            &Completion {
                content: vec![],
                stop_reason: None,
                usage: crate::anthropic::types::Usage {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    cache_creation_input_tokens: 500,
                    cache_read_input_tokens: 9_000,
                },
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            },
        );

        let report = ledger.report(7, &priced(), None);
        assert_eq!(report.totals.uncached_input_tokens, 1_000);
        assert_eq!(report.totals.cached_input_tokens, 9_000);
        assert_eq!(report.totals.cache_write_tokens, 500);
        // Measure hits against the full 10.5k prompt: fresh input, cache reads,
        // and the 500 tokens written to cache on this request.
        let expected_hit_rate = 9_000.0 / 10_500.0 * 100.0;
        assert!((report.totals.cache_hit_percent - expected_hit_rate).abs() < 1e-9);
        // Those reads cost 0.1x input, so they saved 0.9x of 9k at $3/M.
        assert!((report.totals.cache_savings_usd - 0.0243).abs() < 1e-9);
    }

    #[test]
    fn the_three_prompt_shares_account_for_every_prompt_token() {
        let mut ledger = Ledger::default();
        ledger.record(
            "anthropic",
            "claude-sonnet-4-6",
            &Completion {
                content: vec![],
                stop_reason: None,
                usage: crate::anthropic::types::Usage {
                    input_tokens: 1_000,
                    output_tokens: 100,
                    cache_creation_input_tokens: 500,
                    cache_read_input_tokens: 9_000,
                },
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            },
        );

        let totals = ledger.report(7, &priced(), None).totals;
        let sum = totals.served_from_cache_percent
            + totals.written_to_cache_percent
            + totals.read_fresh_percent;
        assert!(
            (sum - 100.0).abs() < 1e-9,
            "the shares must partition the prompt, got {sum}"
        );
        assert_eq!(totals.served_from_cache_percent, totals.cache_hit_percent);
        // 9k read back off 500 written: every cached token paid for itself
        // eighteen times over.
        assert_eq!(totals.cache_reuse_ratio, Some(18.0));
    }

    /// A ratio against nothing is not zero reuse, it is no measurement — and a
    /// "0.0x" on screen would read as caching having failed.
    #[test]
    fn reuse_is_absent_rather_than_zero_when_nothing_was_cached() {
        let mut ledger = Ledger::default();
        ledger.record("codex", "gpt-5.6-sol", &completion_with(10, 4));
        assert_eq!(
            ledger
                .report(7, &Prices::default(), None)
                .totals
                .cache_reuse_ratio,
            None
        );
    }

    /// Providers whose wire format keeps cached tokens inside the prompt total
    /// used to land entirely in `input_tokens`, which pinned their measured hit
    /// rate at zero however well their cache was working.
    #[test]
    fn a_provider_reported_cache_hit_is_not_filed_as_fresh_input() {
        let mut ledger = Ledger::default();
        ledger.record(
            "codex",
            "gpt-5.6-sol",
            &Completion {
                content: vec![],
                stop_reason: None,
                usage: crate::anthropic::types::Usage {
                    input_tokens: 2_000,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 8_000,
                },
                usage_available: true,
                limits: None,
                served_model: None,
                provider_session: None,
            },
        );

        let totals = ledger.report(7, &Prices::default(), None).totals;
        assert_eq!(totals.uncached_input_tokens, 2_000);
        assert_eq!(totals.cached_input_tokens, 8_000);
        assert!((totals.served_from_cache_percent - 80.0).abs() < 1e-9);
    }

    #[test]
    fn a_model_id_containing_a_slash_survives_the_round_trip() {
        // Aggregators serve `vendor/model`, which is why the ledger key does not
        // join on a slash.
        let mut ledger = Ledger::default();
        ledger.record(
            "openrouter",
            "anthropic/claude-sonnet-4-6",
            &completion_with(5, 5),
        );

        let report = ledger.report(7, &Prices::default(), None);
        assert_eq!(report.models[0].provider_id, "openrouter");
        assert_eq!(report.models[0].model_id, "anthropic/claude-sonnet-4-6");
    }

    fn completion_with(input: u32, output: u32) -> Completion {
        Completion {
            content: vec![],
            stop_reason: None,
            usage: crate::anthropic::types::Usage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            usage_available: true,
            limits: None,
            served_model: None,
            provider_session: None,
        }
    }
}
