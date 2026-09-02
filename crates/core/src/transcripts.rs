//! Usage read back out of the coding CLIs' own session transcripts.
//!
//! Zest's ledger records what Zest sent. That is exact, and it is also blind to
//! every turn you ran in Claude Code or Codex directly — which on most machines
//! is the majority of the spend. Both CLIs already write complete per-turn usage
//! to disk, so this module reads it rather than asking you to have routed
//! everything through Zest to be counted.
//!
//! Read-only, and nothing here is uploaded anywhere. It parses files that are
//! already on the machine and produces the same daily/per-model buckets the
//! ledger keeps, so [`crate::usage`] can merge the two instead of special-casing
//! either.
//!
//! Three things make this harder than "sum the usage fields", and getting any of
//! them wrong silently inflates the number:
//!
//! 1. **Claude Code repeats itself.** One record is written per assistant
//!    *content block*, and every one repeats the parent message's complete
//!    `usage` object. Summing them naively overcounts — measured at 2.1x on a
//!    real machine — so records are de-duplicated by message/request id.
//! 2. **Codex re-emits.** An unchanged `token_count` event reappears on some
//!    stream boundaries, so identical consecutive payloads are dropped.
//! 3. **Codex counts input inclusive of cache.** `input_tokens` already contains
//!    the cached and cache-written portions, which must be subtracted out before
//!    the four classes can be priced apart.
//!
//! The parsers are pure and take a line at a time, so a multi-gigabyte transcript
//! directory streams through without being held in memory.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::usage::{day_key, model_key, DayUsage, TokenCounts};

/// Which CLI a record came from. Also the provider id it reports under, kept
/// distinct from Zest's own `claude`/`codex` providers so the usage screen never
/// implies Zest sent a turn it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Claude,
    Codex,
}

impl CliKind {
    pub fn provider_id(self) -> &'static str {
        match self {
            CliKind::Claude => "claude-cli",
            CliKind::Codex => "codex-cli",
        }
    }

    /// Cheap substring gate applied before attempting to parse a line.
    ///
    /// Transcripts are mostly tool output; only a minority of lines carry usage
    /// at all. Skipping the rest without invoking the JSON parser is the
    /// difference between a scan that feels instant and one that does not.
    fn might_carry_usage(self, line: &str) -> bool {
        match self {
            CliKind::Claude => line.contains("\"usage\""),
            CliKind::Codex => line.contains("\"token_count\"") || line.contains("\"model\""),
        }
    }
}

/// One turn's usage, as recorded by the CLI that ran it.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub kind: CliKind,
    /// Unix seconds. The day bucket is derived from this in the local zone.
    pub timestamp_secs: u64,
    pub model: String,
    pub session_id: String,
    pub counts: TokenCounts,
    /// Cost the CLI reported for this turn, when it reports one. Measured, so it
    /// outranks anything derived from a rate table.
    pub reported_cost_usd: Option<f64>,
    /// Cross-file de-duplication key, or `None` when the record is inherently
    /// unique and needs none.
    pub dedupe_key: Option<String>,
}

/// What a scan found, in the shape [`crate::usage`] already reports on.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    /// Per local day, with `by_model` keyed exactly as the ledger keys it.
    pub daily: BTreeMap<String, DayUsage>,
    /// Cost the CLIs reported, per day per model key. Present only for CLIs and
    /// versions that record one; absent is not zero.
    pub reported_cost: BTreeMap<String, BTreeMap<String, f64>>,
    pub status: ScanStatus,
}

/// How the scan went, so the UI can distinguish "no usage" from "nothing read".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// Files actually parsed this time.
    pub files_scanned: u32,
    /// Files answered from the parse cache because they had not changed.
    pub files_cached: u32,
    /// Files skipped because they were last modified before the window.
    pub files_skipped: u32,
    /// Files that could not be opened. Counted, not fatal — one unreadable
    /// transcript must not blank the whole screen.
    pub files_failed: u32,
    pub records: u32,
    /// Records dropped as repeats. Surfaced because a sudden change here is the
    /// first sign a CLI changed its transcript format.
    pub duplicates_dropped: u32,
    /// Directories that were looked in, whether or not they existed.
    pub roots: Vec<ScanRoot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRoot {
    pub provider_id: String,
    pub path: String,
    pub exists: bool,
}

/// Where Claude Code keeps its transcripts.
///
/// `CLAUDE_CONFIG_DIR` overrides the location, and when it does the transcripts
/// sit directly under it rather than under a `.claude` subdirectory.
pub fn claude_transcript_dir() -> Option<PathBuf> {
    if let Ok(configured) = std::env::var("CLAUDE_CONFIG_DIR") {
        let root = PathBuf::from(configured);
        if !root.as_os_str().is_empty() {
            return Some(root.join("projects"));
        }
    }
    dirs::home_dir().map(|home| home.join(".claude").join("projects"))
}

/// Where Codex keeps its rollout files.
pub fn codex_transcript_dir() -> Option<PathBuf> {
    if let Ok(configured) = std::env::var("CODEX_HOME") {
        let root = PathBuf::from(configured);
        if !root.as_os_str().is_empty() {
            return Some(root.join("sessions"));
        }
    }
    dirs::home_dir().map(|home| home.join(".codex").join("sessions"))
}

/// How far back transcripts are parsed and kept in the scan cache.
///
/// Deliberately larger than any window the UI offers, so switching 7 → 90 days
/// re-reads nothing: the expensive part is parsing, and it is done once at the
/// widest horizon rather than again per window.
pub const SCAN_RETENTION_DAYS: u64 = 90;

/// Scan both CLIs for usage in the last `days` local days.
///
/// Parsing is cached per file by modification time and size, because transcript
/// directories are large — over a gigabyte on an active machine — and a session
/// file never changes once its session ends. The first scan pays for the parse;
/// every later one reads only what was appended.
pub fn scan(days: u32) -> ScanResult {
    let roots: Vec<(CliKind, PathBuf)> = [
        (CliKind::Claude, claude_transcript_dir()),
        (CliKind::Codex, codex_transcript_dir()),
    ]
    .into_iter()
    .filter_map(|(kind, dir)| dir.map(|dir| (kind, dir)))
    .collect();

    let mut cache = ScanCache::load();
    let result = scan_roots(days, &roots, &mut cache);
    let _ = cache.save();
    result
}

/// Scan explicit directories. The roots are a parameter rather than resolved
/// inside so tests can point at a fixture tree without setting process-global
/// environment variables that would race between them.
fn scan_roots(days: u32, roots: &[(CliKind, PathBuf)], cache: &mut ScanCache) -> ScanResult {
    let now = now_secs();
    let horizon = now.saturating_sub(SCAN_RETENTION_DAYS.saturating_add(1) * 86_400);
    // A day of slack past the window start: a long-running session's file is
    // modified at the end while its earliest records belong to an earlier day.
    let oldest_day = day_key(
        now.saturating_sub(
            u64::from(days.max(1))
                .saturating_sub(1)
                .saturating_mul(86_400),
        ),
    );

    let mut status = ScanStatus::default();
    let mut files: Vec<(CliKind, PathBuf, u64, u64)> = Vec::new();

    for (kind, dir) in roots {
        status.roots.push(ScanRoot {
            provider_id: kind.provider_id().to_string(),
            path: dir.display().to_string(),
            exists: dir.is_dir(),
        });
        if dir.is_dir() {
            collect(*kind, dir, horizon, &mut files, &mut status);
        }
    }

    // Stable order so cross-file de-duplication always keeps the same copy, and
    // a record cannot drift between days from one scan to the next.
    files.sort_by(|a, b| a.1.cmp(&b.1));

    let mut fresh: BTreeMap<String, CachedFile> = BTreeMap::new();
    let mut result = ScanResult::default();
    let mut seen: HashSet<String> = HashSet::new();

    for (kind, path, mtime, size) in files {
        let key = path.display().to_string();
        let parsed = match cache.files.get(&key) {
            // Size as well as mtime: a same-second rewrite that changes length
            // is exactly the case a timestamp alone misses.
            Some(hit) if hit.mtime == mtime && hit.size == size => {
                status.files_cached += 1;
                hit.clone()
            }
            _ => match parse_file(kind, &path, mtime, size) {
                Some(parsed) => {
                    status.files_scanned += 1;
                    parsed
                }
                None => {
                    status.files_failed += 1;
                    continue;
                }
            },
        };

        status.duplicates_dropped += parsed.within_file_duplicates;
        for record in &parsed.records {
            // The day is derived here, not read from the cache: see
            // `CachedRecord::at` for why storing it would be wrong.
            let day = day_key(record.at);
            if day < oldest_day {
                continue;
            }
            let Some(model) = parsed.models.get(record.m) else {
                continue;
            };
            if let Some(dedupe) = &record.key {
                if !seen.insert(dedupe.clone()) {
                    status.duplicates_dropped += 1;
                    continue;
                }
            }
            status.records += 1;
            result.add(&day, model, unpack(record.c), record.cost);
        }

        fresh.insert(key, parsed);
    }

    // Entries for files that vanished or aged out are dropped rather than kept
    // forever; the cache should not outlive what it describes.
    cache.files = fresh;
    result.status = status;
    result
}

fn collect(
    kind: CliKind,
    dir: &Path,
    horizon: u64,
    out: &mut Vec<(CliKind, PathBuf, u64, u64)>,
    status: &mut ScanStatus,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        status.files_failed += 1;
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect(kind, &path, horizon, out, status),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "jsonl") => {
                let Ok(meta) = entry.metadata() else {
                    status.files_failed += 1;
                    continue;
                };
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                let mtime = modified
                    .map(|d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
                    .unwrap_or(0);
                let mtime_secs = modified.map(|d| d.as_secs()).unwrap_or(0);
                // A file untouched since before the horizon cannot hold a record
                // inside it. This is what keeps a year of transcripts cheap.
                if mtime_secs > 0 && mtime_secs < horizon {
                    status.files_skipped += 1;
                    continue;
                }
                out.push((kind, path, mtime, meta.len()));
            }
            _ => {}
        }
    }
}

/// Parse one transcript into the form the cache stores.
fn parse_file(kind: CliKind, path: &Path, mtime: u64, size: u64) -> Option<CachedFile> {
    let file = std::fs::File::open(path).ok()?;
    let mut codex = CodexScanState::default();
    let mut records: Vec<CachedRecord> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let mut within_file: HashSet<String> = HashSet::new();
    let mut within_file_duplicates = 0u32;

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if !kind.might_carry_usage(&line) {
            continue;
        }
        let record = match kind {
            CliKind::Claude => parse_claude_line(&line),
            CliKind::Codex => parse_codex_line(&line, &mut codex),
        };
        let Some(record) = record else { continue };

        // Within-file repeats collapse now. The cross-file pass still needs the
        // key, so the record itself is kept whole.
        if let Some(key) = &record.dedupe_key {
            if !within_file.insert(key.clone()) {
                within_file_duplicates += 1;
                continue;
            }
        }

        let model = model_key(record.kind.provider_id(), &record.model);
        let m = match models.iter().position(|known| known == &model) {
            Some(index) => index,
            None => {
                models.push(model);
                models.len() - 1
            }
        };

        records.push(CachedRecord {
            at: record.timestamp_secs,
            m,
            c: pack(&record.counts),
            key: record.dedupe_key,
            cost: record.reported_cost_usd,
        });
    }

    Some(CachedFile {
        mtime,
        size,
        models,
        records,
        within_file_duplicates,
    })
}

impl ScanResult {
    fn add(&mut self, day: &str, model: &str, counts: TokenCounts, cost: Option<f64>) {
        self.daily
            .entry(day.to_string())
            .or_default()
            .merge_counts(model, &counts);
        if let Some(cost) = cost.filter(|value| value.is_finite() && *value >= 0.0) {
            *self
                .reported_cost
                .entry(day.to_string())
                .or_default()
                .entry(model.to_string())
                .or_insert(0.0) += cost;
        }
    }
}

/* -------------------------------------------------------------------------- */
/* Scan cache                                                                 */
/* -------------------------------------------------------------------------- */

/// Bumped when the cached shape changes, so an old file is re-parsed rather than
/// misread.
///
/// 2: records carry a timestamp instead of a pre-computed day key. Version 1
/// baked the day in, which meant a cache written by the CLI (which buckets in
/// UTC) was read back by the desktop (which buckets in the machine's zone) as
/// though the days matched — silently misplacing every turn in the offset
/// window, six hours a day at UTC-6.
///
/// 3: rows are positional arrays and the file is written compactly.
///
/// 4: file modification times are stored with sub-second precision, so a
/// same-size rewrite in one second cannot be mistaken for an unchanged file.
const CACHE_FORMAT: u32 = 4;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ScanCache {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    files: BTreeMap<String, CachedFile>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// One transcript's parse result, keyed by how the file looked when it was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFile {
    mtime: u64,
    size: u64,
    /// Model keys this file touched. Records hold an index rather than the
    /// string: a transcript uses a handful of models across tens of thousands
    /// of turns, and repeating the name each time is most of the file.
    #[serde(default)]
    models: Vec<String>,
    /// Every usage record, individually.
    ///
    /// Not pre-aggregated per day, because a day depends on the reader's
    /// timezone — see [`CachedRecord::at`]. Not pre-aggregated per model
    /// either, because the same message legitimately appears in more than one
    /// transcript when a session is resumed or forked, and only a pass across
    /// all files can tell. Measured at 14% of Claude Code records on a real
    /// machine, so collapsing them per file would overcount by that much.
    #[serde(default)]
    records: Vec<CachedRecord>,
    /// Repeats collapsed while parsing this file.
    ///
    /// Carried in the cache so the reported duplicate count means the same thing
    /// on a cached scan as on a cold one. Without it the figure would appear to
    /// collapse the first time the cache was warm, which is exactly the kind of
    /// unexplained change it exists to make visible.
    #[serde(default)]
    within_file_duplicates: u32,
}

/// Counts are a fixed array rather than named fields purely for size: this is
/// the row that repeats tens of thousands of times in the cache file.
type Packed = [u64; 5];

fn pack(counts: &TokenCounts) -> Packed {
    [
        counts.requests,
        counts.input_tokens,
        counts.output_tokens,
        counts.cache_write_tokens,
        counts.cache_read_tokens,
    ]
}

fn unpack(c: Packed) -> TokenCounts {
    TokenCounts {
        requests: c[0],
        input_tokens: c[1],
        output_tokens: c[2],
        cache_write_tokens: c[3],
        cache_read_tokens: c[4],
    }
}

/// One cached turn.
///
/// Serialised as a bare array rather than an object — see the `Serialize` impl.
#[derive(Debug, Clone, PartialEq)]
struct CachedRecord {
    /// Unix seconds of the turn.
    ///
    /// The timestamp, never a day key. Which day a turn falls on depends on the
    /// reader's timezone, and the two front ends do not agree on one: the CLI
    /// buckets in UTC so its output is deterministic, while the desktop uses the
    /// machine's zone. They share this file. Caching a day computed by whichever
    /// process wrote last would hand the other one someone else's calendar, and
    /// it would also survive the user changing timezone, which it must not.
    at: u64,
    /// Index into [`CachedFile::models`].
    m: usize,
    c: Packed,
    /// De-duplication key, for records that carry one.
    key: Option<String>,
    cost: Option<f64>,
}

/// Positional encoding: `[at, m, requests, in, out, cache_write, cache_read]`,
/// with `key` and `cost` appended only when present.
///
/// Field names are a third of this file once the rows are counted in tens of
/// thousands, and they are the same eight names on every row. The trailing
/// optionals are omitted rather than written as `null` because the overwhelming
/// majority of rows — every Codex turn — have neither.
impl Serialize for CachedRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let extra = match (self.key.is_some(), self.cost.is_some()) {
            (_, true) => 2,
            (true, false) => 1,
            (false, false) => 0,
        };
        let mut seq = serializer.serialize_seq(Some(7 + extra))?;
        seq.serialize_element(&self.at)?;
        seq.serialize_element(&self.m)?;
        for value in self.c {
            seq.serialize_element(&value)?;
        }
        if extra >= 1 {
            seq.serialize_element(&self.key)?;
        }
        if extra >= 2 {
            seq.serialize_element(&self.cost)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for CachedRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor;

        impl<'de> serde::de::Visitor<'de> for RowVisitor {
            type Value = CachedRecord;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a cached usage row of at least 7 elements")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<CachedRecord, A::Error> {
                use serde::de::Error as _;
                let mut next = |index: usize| {
                    seq.next_element::<u64>()?
                        .ok_or_else(|| A::Error::invalid_length(index, &"7 numbers"))
                };
                let at = next(0)?;
                let m = next(1)? as usize;
                let mut c: Packed = [0; 5];
                for (index, slot) in c.iter_mut().enumerate() {
                    *slot = next(2 + index)?;
                }
                // Absent and null both mean "not recorded": a short row simply
                // stopped early, which is how the common case is written.
                let key = seq.next_element::<Option<String>>()?.flatten();
                let cost = seq.next_element::<Option<f64>>()?.flatten();
                Ok(CachedRecord {
                    at,
                    m,
                    c,
                    key,
                    cost,
                })
            }
        }

        deserializer.deserialize_seq(RowVisitor)
    }
}

impl ScanCache {
    fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("zest").join("usage-scan-cache.json"))
    }

    fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::default();
        };
        let mut cache = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<ScanCache>(&raw).ok())
            .filter(|c| c.format == CACHE_FORMAT)
            .unwrap_or_default();
        cache.path = Some(path);
        cache
    }

    fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        self.format = CACHE_FORMAT;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fsutil::atomic_write_json_compact(&path, self)
    }
}

/* -------------------------------------------------------------------------- */
/* Claude Code                                                                */
/* -------------------------------------------------------------------------- */

/// Parse one line of a Claude Code transcript.
///
/// Every assistant content block gets its own record carrying a full copy of the
/// parent message's `usage`, so the returned `dedupe_key` is not optional
/// bookkeeping — without it the totals are roughly double.
pub fn parse_claude_line(line: &str) -> Option<UsageRecord> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message.get("model")?.as_str()?;
    // `<synthetic>` marks messages Claude Code generated locally — an interrupt
    // notice, a hook result. They were never sent anywhere and never billed, so
    // counting them would pad the unpriced column with traffic that does not
    // exist rather than reveal a gap in the rate table.
    if model.is_empty() || model == "<synthetic>" {
        return None;
    }
    let timestamp_secs = parse_rfc3339_secs(value.get("timestamp")?.as_str()?)?;

    let message_id = message.get("id").and_then(|v| v.as_str());
    let request_id = value.get("requestId").and_then(|v| v.as_str());
    // Dedupe on the message/request pair, falling back to whichever half
    // exists. A record with neither cannot be de-duplicated and is kept.
    let dedupe_key = match (message_id, request_id) {
        (None, None) => None,
        (m, r) => Some(format!("{}:{}", m.unwrap_or(""), r.unwrap_or(""))),
    };

    Some(UsageRecord {
        kind: CliKind::Claude,
        timestamp_secs,
        model: model.to_string(),
        session_id: value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        counts: TokenCounts {
            requests: 1,
            input_tokens: u64_at(usage, "input_tokens"),
            output_tokens: u64_at(usage, "output_tokens"),
            cache_write_tokens: u64_at(usage, "cache_creation_input_tokens"),
            cache_read_tokens: u64_at(usage, "cache_read_input_tokens"),
        },
        reported_cost_usd: value
            .get("costUSD")
            .and_then(|v| v.as_f64())
            .filter(|cost| cost.is_finite() && *cost >= 0.0),
        dedupe_key,
    })
}

/* -------------------------------------------------------------------------- */
/* Codex                                                                      */
/* -------------------------------------------------------------------------- */

/// Rolling state for one Codex rollout file.
///
/// `token_count` events carry no model of their own, so it is carried forward
/// from the most recent `turn_context`. A session that switches model mid-run
/// therefore attributes correctly from the switch onward.
#[derive(Debug, Clone, Default)]
pub struct CodexScanState {
    model: String,
    session_id: String,
    last_signature: Option<String>,
}

/// Feed one line of a Codex rollout in, returning a record if it was a usage
/// event.
pub fn parse_codex_line(line: &str, state: &mut CodexScanState) -> Option<UsageRecord> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload")?;

    match value.get("type").and_then(|v| v.as_str()) {
        Some("session_meta") => {
            if let Some(id) = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(|v| v.as_str())
            {
                state.session_id = id.to_string();
            }
            return None;
        }
        Some("turn_context") => {
            if let Some(model) = payload.get("model").and_then(|v| v.as_str()) {
                state.model = model.to_string();
            }
            return None;
        }
        _ => {}
    }

    if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
        return None;
    }
    let last = payload.get("info")?.get("last_token_usage")?;

    // Eligibility is checked *before* the duplicate signature is consumed. A
    // token_count that arrives ahead of its turn_context has no model yet; if it
    // poisoned the signature, the re-emitted copy that arrives once the model is
    // known would be dropped as a repeat and those tokens never counted.
    let timestamp_secs = parse_rfc3339_secs(value.get("timestamp")?.as_str()?)?;
    if state.model.is_empty() {
        return None;
    }

    let signature = last.to_string();
    if state.last_signature.as_deref() == Some(signature.as_str()) {
        return None;
    }
    state.last_signature = Some(signature);

    let input = u64_at(last, "input_tokens");
    let cached = u64_at(last, "cached_input_tokens");
    let cache_write = u64_at(last, "cache_write_input_tokens");
    let output = u64_at(last, "output_tokens");

    let counts = TokenCounts {
        requests: 1,
        // Codex reports input inclusive of the cached and cache-written parts.
        input_tokens: input.saturating_sub(cached).saturating_sub(cache_write),
        output_tokens: output,
        cache_write_tokens: cache_write,
        cache_read_tokens: cached,
    };
    if counts.total_tokens() == 0 {
        return None;
    }

    Some(UsageRecord {
        kind: CliKind::Codex,
        timestamp_secs,
        model: state.model.clone(),
        session_id: state.session_id.clone(),
        counts,
        // Codex records no cost in the rollout.
        reported_cost_usd: None,
        // Rollout files are per session, so these need no global de-duplication.
        dedupe_key: None,
    })
}

fn u64_at(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|v| v.as_u64())
        .filter(|n| *n < u64::from(u32::MAX) * 1_000)
        .unwrap_or(0)
}

/// Seconds since the epoch from an RFC 3339 timestamp.
///
/// Written out rather than pulling in a date crate for one field: transcripts
/// use a fixed `YYYY-MM-DDTHH:MM:SS` prefix with an optional fractional part and
/// zone, and only whole seconds matter for day bucketing.
fn parse_rfc3339_secs(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return None;
    }

    let days = crate::usage::day_number_from_key(&format!("{year:04}-{month:02}-{day:02}"))?;
    let mut secs = days * 86_400 + hour * 3_600 + minute * 60 + second;

    // A trailing offset means the wall-clock parts above are in that zone; shift
    // back to UTC so every record lands on one timeline before day bucketing
    // reapplies the *user's* zone.
    let mut rest = text.get(19..)?;
    if rest.starts_with('.') {
        let fraction = rest.get(1..)?;
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = fraction.get(digits..)?;
    }
    if rest.is_empty() || rest == "Z" || rest == "z" {
        return u64::try_from(secs).ok();
    }
    if rest.len() != 6
        || !matches!(rest.as_bytes().first(), Some(b'+' | b'-'))
        || rest.as_bytes().get(3) != Some(&b':')
    {
        return None;
    }
    let sign = if rest.starts_with('+') { 1 } else { -1 };
    let oh: i64 = rest.get(1..3)?.parse().ok()?;
    let om: i64 = rest.get(4..6)?.parse().ok()?;
    if oh > 23 || om > 59 {
        return None;
    }
    secs -= sign * (oh * 3_600 + om * 60);
    u64::try_from(secs).ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_LINE: &str = r#"{"type":"assistant","requestId":"req_1","sessionId":"sess_a",
        "timestamp":"2026-08-08T12:00:00.000Z","message":{"id":"msg_1","model":"claude-sonnet-5",
        "usage":{"input_tokens":2,"cache_creation_input_tokens":11700,
        "cache_read_input_tokens":29617,"output_tokens":123}}}"#;

    #[test]
    fn a_claude_line_yields_its_four_token_classes() {
        let record = parse_claude_line(CLAUDE_LINE).expect("parsed");
        assert_eq!(record.model, "claude-sonnet-5");
        assert_eq!(record.session_id, "sess_a");
        assert_eq!(record.counts.input_tokens, 2);
        assert_eq!(record.counts.cache_write_tokens, 11_700);
        assert_eq!(record.counts.cache_read_tokens, 29_617);
        assert_eq!(record.counts.output_tokens, 123);
        assert_eq!(record.dedupe_key.as_deref(), Some("msg_1:req_1"));
        assert_eq!(record.reported_cost_usd, None);
    }

    /// A throwaway transcript tree, scanned with a fresh in-memory cache.
    struct Fixture {
        dir: crate::fsutil::ScratchDir,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = crate::fsutil::ScratchDir::new(&format!("zest-transcripts-{name}-"));
            std::fs::create_dir_all(dir.join("claude")).unwrap();
            std::fs::create_dir_all(dir.join("codex")).unwrap();
            Self { dir }
        }

        fn write(&self, kind: &str, name: &str, lines: &[&str]) {
            let body = lines.join("\n");
            std::fs::write(self.dir.join(kind).join(name), body).unwrap();
        }

        fn scan(&self, days: u32) -> ScanResult {
            self.scan_with(days, &mut ScanCache::default())
        }

        fn scan_with(&self, days: u32, cache: &mut ScanCache) -> ScanResult {
            scan_roots(
                days,
                &[
                    (CliKind::Claude, self.dir.join("claude")),
                    (CliKind::Codex, self.dir.join("codex")),
                ],
                cache,
            )
        }
    }

    /// A Claude line carrying today's date, so window filtering keeps it.
    fn claude_today(message_id: &str, request_id: &str, output: u64) -> String {
        let date = day_key(now_secs());
        format!(
            r#"{{"type":"assistant","requestId":"{request_id}","sessionId":"s",
            "timestamp":"{date}T12:00:00.000Z","message":{{"id":"{message_id}",
            "model":"claude-sonnet-5","usage":{{"input_tokens":2,
            "cache_creation_input_tokens":11700,"cache_read_input_tokens":29617,
            "output_tokens":{output}}}}}}}"#
        )
        .replace('\n', "")
    }

    #[test]
    fn every_content_block_repeats_the_same_usage_and_is_counted_once() {
        // The 2.1x overcount this exists to prevent, in its real form: one
        // record per content block, each repeating the parent's whole usage.
        let fixture = Fixture::new("within-file-dedup");
        let line = claude_today("msg_1", "req_1", 123);
        fixture.write("claude", "a.jsonl", &[&line, &line, &line]);

        let result = fixture.scan(30);
        let total: u64 = result.daily.values().map(|day| day.total_tokens()).sum();
        assert_eq!(total, 2 + 11_700 + 29_617 + 123);
        assert_eq!(result.status.records, 1);
    }

    #[test]
    fn the_same_message_in_two_transcripts_is_counted_once() {
        // A resumed or forked session copies its history into a new file. 14% of
        // Claude Code records on a real machine, so per-file de-duplication
        // alone would overcount by that much.
        let fixture = Fixture::new("cross-file-dedup");
        let shared = claude_today("msg_1", "req_1", 123);
        let unique = claude_today("msg_2", "req_2", 7);
        fixture.write("claude", "a.jsonl", &[&shared]);
        fixture.write("claude", "b.jsonl", &[&shared, &unique]);

        let result = fixture.scan(30);
        assert_eq!(result.status.records, 2);
        assert_eq!(result.status.duplicates_dropped, 1);
        let total: u64 = result.daily.values().map(|day| day.total_tokens()).sum();
        assert_eq!(
            total,
            (2 + 11_700 + 29_617 + 123) + (2 + 11_700 + 29_617 + 7)
        );
    }

    #[test]
    fn an_unchanged_file_is_answered_from_the_cache() {
        // The scan reads over a gigabyte on an active machine. Re-parsing it on
        // every window switch is the difference between instant and unusable.
        let fixture = Fixture::new("cache-hit");
        fixture.write("claude", "a.jsonl", &[&claude_today("m", "r", 5)]);

        let mut cache = ScanCache::default();
        let first = fixture.scan_with(30, &mut cache);
        assert_eq!(first.status.files_scanned, 1);
        assert_eq!(first.status.files_cached, 0);

        let second = fixture.scan_with(30, &mut cache);
        assert_eq!(second.status.files_scanned, 0, "nothing re-parsed");
        assert_eq!(second.status.files_cached, 1);
        assert_eq!(second.status.records, first.status.records);

        // A rewrite changes the length, so the stale entry is not trusted.
        fixture.write(
            "claude",
            "a.jsonl",
            &[&claude_today("m", "r", 5), &claude_today("m2", "r2", 6)],
        );
        let third = fixture.scan_with(30, &mut cache);
        assert_eq!(third.status.files_scanned, 1, "changed file re-parsed");
        assert_eq!(third.status.records, 2);
    }

    #[test]
    fn a_cached_scan_still_de_duplicates_across_files() {
        // The trap in caching: if a file's records were collapsed at cache time,
        // the copy in the other file could no longer be recognised as a repeat.
        let fixture = Fixture::new("cache-dedup");
        let shared = claude_today("msg_1", "req_1", 123);
        fixture.write("claude", "a.jsonl", &[&shared]);
        fixture.write("claude", "b.jsonl", &[&shared]);

        let mut cache = ScanCache::default();
        let first = fixture.scan_with(30, &mut cache);
        let second = fixture.scan_with(30, &mut cache);

        assert_eq!(second.status.files_cached, 2, "both served from cache");
        assert_eq!(second.status.records, first.status.records);
        assert_eq!(second.status.records, 1);
        assert_eq!(second.status.duplicates_dropped, 1);
    }

    #[test]
    fn a_codex_rollout_survives_the_cache_round_trip() {
        let fixture = Fixture::new("codex-cache");
        let date = day_key(now_secs());
        let lines: Vec<String> = vec![
            format!(
                r#"{{"type":"session_meta","timestamp":"{date}T12:00:00Z","payload":{{"id":"s"}}}}"#
            ),
            format!(
                r#"{{"type":"turn_context","timestamp":"{date}T12:00:01Z","payload":{{"model":"gpt-5.6-terra"}}}}"#
            ),
            format!(
                r#"{{"type":"event_msg","timestamp":"{date}T12:00:02Z","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":34911,"cached_input_tokens":6016,"output_tokens":174}}}}}}}}"#
            ),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        fixture.write("codex", "roll.jsonl", &refs);

        let mut cache = ScanCache::default();
        let first = fixture.scan_with(30, &mut cache);
        let second = fixture.scan_with(30, &mut cache);

        assert_eq!(second.status.files_cached, 1);
        let key = model_key("codex-cli", "gpt-5.6-terra");
        for result in [&first, &second] {
            let day = result.daily.get(&date).expect("today");
            assert_eq!(day.by_model[&key].input_tokens, 34_911 - 6_016);
            assert_eq!(day.by_model[&key].cache_read_tokens, 6_016);
            assert_eq!(day.by_model[&key].output_tokens, 174);
        }
    }

    #[test]
    fn a_record_with_no_ids_is_kept_rather_than_treated_as_a_repeat() {
        // Without a key there is nothing to compare, so dropping it would lose
        // real usage. Two such records are two turns.
        let date = day_key(now_secs());
        let line = format!(
            r#"{{"type":"assistant","timestamp":"{date}T12:00:00Z","message":{{"model":"claude-sonnet-5","usage":{{"output_tokens":10}}}}}}"#
        );
        assert_eq!(parse_claude_line(&line).unwrap().dedupe_key, None);

        let fixture = Fixture::new("no-ids");
        fixture.write("claude", "a.jsonl", &[&line, &line]);
        let result = fixture.scan(30);
        assert_eq!(result.status.records, 2, "no key, no dedup");
        assert_eq!(result.status.duplicates_dropped, 0);
    }

    #[test]
    fn a_reported_cost_is_carried_through() {
        let line = r#"{"type":"assistant","requestId":"r","timestamp":"2026-08-08T12:00:00Z",
            "costUSD":0.1823282,"message":{"id":"m","model":"claude-opus-5",
            "usage":{"output_tokens":10}}}"#;
        let record = parse_claude_line(line).expect("parsed");
        assert_eq!(record.reported_cost_usd, Some(0.1823282));
    }

    #[test]
    fn malformed_timestamps_and_costs_are_ignored() {
        for timestamp in [
            "2026-02-30T12:00:00Z",
            "2026-08-08T24:00:00Z",
            "2026-08-08T12:60:00Z",
            "2026-08-08T12:00:00+99:00",
        ] {
            let line = format!(
                r#"{{"type":"assistant","timestamp":"{timestamp}","message":{{"model":"claude-sonnet-5","usage":{{"output_tokens":10}}}}}}"#
            );
            assert!(parse_claude_line(&line).is_none(), "accepted {timestamp}");
        }

        let negative = r#"{"type":"assistant","timestamp":"2026-08-08T12:00:00Z",
            "costUSD":-0.01,"message":{"model":"claude-sonnet-5","usage":{"output_tokens":10}}}"#;
        assert_eq!(
            parse_claude_line(negative)
                .expect("usage still parses")
                .reported_cost_usd,
            None
        );
    }

    #[test]
    fn a_non_assistant_line_is_ignored() {
        assert!(parse_claude_line(r#"{"type":"user","message":{"usage":{}}}"#).is_none());
        assert!(parse_claude_line("not json at all").is_none());
    }

    #[test]
    fn a_locally_generated_message_is_not_usage() {
        // Never sent, never billed. Counting it would pad the unpriced column
        // with traffic that does not exist.
        let line = r#"{"type":"assistant","requestId":"r","timestamp":"2026-08-08T12:00:00Z",
            "message":{"id":"m","model":"<synthetic>","usage":{"input_tokens":5,"output_tokens":9}}}"#;
        assert!(parse_claude_line(line).is_none());
    }

    fn codex_lines() -> [&'static str; 4] {
        [
            r#"{"type":"session_meta","timestamp":"2026-08-08T12:00:00Z","payload":{"id":"sess_c"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-08-08T12:00:01Z","payload":{"model":"gpt-5.6-terra"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-08-08T12:00:02Z","payload":{"type":"token_count",
               "info":{"last_token_usage":{"input_tokens":34911,"cached_input_tokens":6016,
               "output_tokens":174,"reasoning_output_tokens":20}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-08-08T12:00:03Z","payload":{"type":"token_count",
               "info":{"last_token_usage":{"input_tokens":34911,"cached_input_tokens":6016,
               "output_tokens":174,"reasoning_output_tokens":20}}}}"#,
        ]
    }

    #[test]
    fn codex_input_is_split_out_of_its_cached_portion() {
        let lines = codex_lines();
        let mut state = CodexScanState::default();
        assert!(parse_codex_line(lines[0], &mut state).is_none());
        assert!(parse_codex_line(lines[1], &mut state).is_none());

        let record = parse_codex_line(lines[2], &mut state).expect("usage event");
        assert_eq!(record.model, "gpt-5.6-terra");
        assert_eq!(record.session_id, "sess_c");
        // 34911 reported input includes the 6016 that were cached.
        assert_eq!(record.counts.input_tokens, 34_911 - 6_016);
        assert_eq!(record.counts.cache_read_tokens, 6_016);
        assert_eq!(record.counts.output_tokens, 174);
    }

    #[test]
    fn an_unchanged_codex_event_is_not_counted_twice() {
        let lines = codex_lines();
        let mut state = CodexScanState::default();
        for line in &lines[..3] {
            let _ = parse_codex_line(line, &mut state);
        }
        assert!(
            parse_codex_line(lines[3], &mut state).is_none(),
            "an identical consecutive payload is a re-emit, not a new turn"
        );
    }

    #[test]
    fn a_token_count_before_its_turn_context_does_not_poison_the_repeat_guard() {
        // The event is re-emitted once the model is known; dropping that copy
        // would lose the turn entirely.
        let lines = codex_lines();
        let mut state = CodexScanState::default();
        assert!(
            parse_codex_line(lines[2], &mut state).is_none(),
            "no model yet"
        );
        assert!(parse_codex_line(lines[1], &mut state).is_none());
        assert!(
            parse_codex_line(lines[2], &mut state).is_some(),
            "the re-emitted copy must still count"
        );
    }

    #[test]
    fn a_zero_token_event_is_not_a_turn() {
        let line = r#"{"type":"event_msg","timestamp":"2026-08-08T12:00:02Z","payload":{
            "type":"token_count","info":{"last_token_usage":{"input_tokens":0,"output_tokens":0}}}}"#;
        let mut state = CodexScanState {
            model: "gpt-5.6-terra".into(),
            ..Default::default()
        };
        assert!(parse_codex_line(line, &mut state).is_none());
    }

    #[test]
    fn one_file_can_serve_a_narrow_window_and_a_wide_one() {
        // Records are filtered by day at merge time, not at parse time, so the
        // cache is built once at the widest horizon and every window reads it.
        let fixture = Fixture::new("window");
        let today = day_key(now_secs());
        let long_ago = day_key(now_secs() - 40 * 86_400);
        let dated = |date: &str, id: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{id}","timestamp":"{date}T12:00:00Z","message":{{"id":"{id}","model":"claude-sonnet-5","usage":{{"output_tokens":10}}}}}}"#
            )
        };
        fixture.write(
            "claude",
            "a.jsonl",
            &[&dated(&today, "now"), &dated(&long_ago, "old")],
        );

        let mut cache = ScanCache::default();
        assert_eq!(fixture.scan_with(7, &mut cache).status.records, 1);
        let wide = fixture.scan_with(90, &mut cache);
        assert_eq!(wide.status.records, 2, "the older day is in range now");
        assert_eq!(wide.status.files_cached, 1, "without re-parsing the file");
    }

    #[test]
    fn timestamps_parse_with_and_without_a_zone() {
        let utc = parse_rfc3339_secs("2026-08-08T12:00:00Z").unwrap();
        assert_eq!(parse_rfc3339_secs("2026-08-08T12:00:00.123Z").unwrap(), utc);
        // Noon in UTC-06:00 is 18:00 UTC.
        assert_eq!(
            parse_rfc3339_secs("2026-08-08T12:00:00-06:00").unwrap(),
            utc + 6 * 3_600
        );
        assert_eq!(
            parse_rfc3339_secs("2026-08-08T12:00:00+02:00").unwrap(),
            utc - 2 * 3_600
        );
        assert!(parse_rfc3339_secs("not a timestamp").is_none());
    }

    #[test]
    fn scanned_records_land_in_ledger_shaped_buckets() {
        let fixture = Fixture::new("ledger-shape");
        fixture.write("claude", "a.jsonl", &[&claude_today("m", "r", 123)]);
        let result = fixture.scan(30);

        let day = result.daily.values().next().expect("one day");
        // Keyed exactly as the ledger keys models, so reporting merges the two
        // rather than special-casing either.
        let expected = model_key("claude-cli", "claude-sonnet-5");
        assert!(day.by_model.contains_key(&expected));
        assert_eq!(day.by_model[&expected].requests, 1);
        assert_eq!(day.requests, 1);
        assert_eq!(day.total_tokens(), day.by_model[&expected].total_tokens());
    }

    #[test]
    fn a_cached_scan_buckets_by_the_readers_timezone_not_the_writers() {
        // The bug this guards: the CLI buckets days in UTC and the desktop in
        // the machine's zone, and they share one cache file. Caching a day key
        // meant whichever wrote last decided the calendar for both, silently
        // misplacing every turn inside the offset window.
        let _guard = crate::usage::LOCAL_OFFSET_TEST_LOCK.lock();
        let fixture = Fixture::new("tz-independent");
        // 02:00 UTC on the 9th is still the 8th anywhere west of Greenwich.
        let line = r#"{"type":"assistant","requestId":"r","timestamp":"2026-08-09T02:00:00Z","message":{"id":"m","model":"claude-sonnet-5","usage":{"output_tokens":10}}}"#;
        fixture.write("claude", "a.jsonl", &[line]);

        let mut cache = ScanCache::default();

        // Written by a process bucketing in UTC...
        crate::usage::set_local_offset_minutes(0);
        let utc = fixture.scan_with(3650, &mut cache);
        assert!(
            utc.daily.contains_key("2026-08-09"),
            "{:?}",
            utc.daily.keys()
        );

        // ...and read back by one six hours behind, off the same cache.
        crate::usage::set_local_offset_minutes(-6 * 60);
        let local = fixture.scan_with(3650, &mut cache);
        assert_eq!(local.status.files_cached, 1, "served from the same cache");
        assert!(
            local.daily.contains_key("2026-08-08"),
            "the reader's day, not the writer's: {:?}",
            local.daily.keys()
        );

        crate::usage::set_local_offset_minutes(0);
    }

    #[test]
    fn a_cached_row_is_a_bare_array_and_omits_what_it_does_not_have() {
        // The shape is the point: eight repeated field names across tens of
        // thousands of rows is most of the file.
        let plain = CachedRecord {
            at: 1_785_974_925,
            m: 0,
            c: [1, 2, 123, 11_700, 29_617],
            key: None,
            cost: None,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            "[1785974925,0,1,2,123,11700,29617]"
        );

        let keyed = CachedRecord {
            key: Some("msg:req".into()),
            ..plain.clone()
        };
        assert_eq!(
            serde_json::to_string(&keyed).unwrap(),
            "[1785974925,0,1,2,123,11700,29617,\"msg:req\"]"
        );

        let costed = CachedRecord {
            cost: Some(0.25),
            ..plain.clone()
        };
        // A cost with no key still needs the key slot held open.
        assert_eq!(
            serde_json::to_string(&costed).unwrap(),
            "[1785974925,0,1,2,123,11700,29617,null,0.25]"
        );

        for row in [plain, keyed, costed] {
            let json = serde_json::to_string(&row).unwrap();
            let back: CachedRecord = serde_json::from_str(&json).unwrap();
            assert_eq!(back, row, "round trip: {json}");
        }
    }

    #[test]
    fn a_truncated_row_is_refused_rather_than_read_as_zeroes() {
        // Silently reading a short row as zero tokens would under-report spend,
        // which is the failure mode this whole module is careful about.
        assert!(serde_json::from_str::<CachedRecord>("[1,0,1,2,123]").is_err());
    }

    #[test]
    fn a_cli_that_has_never_run_is_reported_as_absent_not_as_zero() {
        // "No transcripts found" and "found transcripts with no usage" are
        // different answers, and the roots list is what distinguishes them.
        let fixture = Fixture::new("missing-root");
        let result = scan_roots(
            30,
            &[(CliKind::Codex, fixture.dir.join("nowhere"))],
            &mut ScanCache::default(),
        );
        assert_eq!(result.status.files_scanned, 0);
        assert_eq!(result.status.roots.len(), 1);
        assert!(!result.status.roots[0].exists);
    }
}
