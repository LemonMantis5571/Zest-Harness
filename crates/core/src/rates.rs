//! The published rate catalogue, fetched rather than hand-maintained.
//!
//! Zest prices tokens against LiteLLM's `model_prices_and_context_window.json`
//! — the same table `ccusage` and T3 Code price against. Hand-keeping rates for
//! every model on every provider is a losing job: the list changes weekly, and a
//! table that is quietly six months stale is worse than one that says so.
//!
//! Three properties this module is built around:
//!
//! 1. **The network is never on the hot path.** [`RateCatalog::load`] only ever
//!    reads the local cache. [`refresh`] is the only thing that fetches, and it
//!    is called from startup and from the usage screen's Refresh button — never
//!    from a turn, and never from rendering a report. A usage screen that hangs
//!    on a GitHub outage is a worse product than one showing yesterday's rates.
//! 2. **Staleness is reported, not hidden.** The cache carries when it was
//!    fetched, and the report passes that through so the UI can say how old the
//!    rates are instead of implying they are current.
//! 3. **Nothing about you leaves the machine.** This is an unauthenticated GET
//!    of one public JSON file. No model names, no token counts, no identifiers —
//!    the request is identical whoever makes it. Zest's local-first promise is
//!    about not shipping your data out, and this does not.
//!
//! Rates in the published document are USD *per token*; everything inside Zest
//! is per million, so the projection converts once, here, at the boundary.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::fsutil;
use crate::pricing::ModelPrice;

/// Where the published table lives.
pub const DEFAULT_RATES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// How long a cached catalogue is treated as current. Published rates change on
/// the order of weeks, so a day is comfortably fresh and keeps Zest to at most
/// one request per day.
pub const RATES_TTL_SECS: u64 = 24 * 60 * 60;

/// Bumped when the projected shape changes, so an old cache is re-fetched rather
/// than misread. The cache stores the projection, not the 1.6 MB source
/// document: it is what every read actually wants, and a sixth of the size.
const CACHE_FORMAT: u32 = 1;

/// Model names that must never resolve to a rate.
///
/// `<synthetic>` marks locally generated messages that were never billed at all.
/// The bare family names are genuinely ambiguous — "opus" has meant several
/// models at several prices — so they are reported as unpriced rather than
/// silently costed at whichever generation happens to sort first.
const UNPRICEABLE: &[&str] = &[
    "<synthetic>",
    "synthetic",
    "opus",
    "sonnet",
    "haiku",
    "fable",
    "unknown",
];

/// Rates projected from the published table, keyed by normalised model name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateCatalog {
    #[serde(default)]
    format: u32,
    /// Unix seconds. `None` for a catalogue that has never been fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetched_at: Option<u64>,
    #[serde(default)]
    models: BTreeMap<String, ModelPrice>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl RateCatalog {
    /// `<data dir>/zest/model-rates.json`, beside the ledger it prices.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("zest").join("model-rates.json"))
    }

    /// Read the cached catalogue. Never fetches; an absent or unreadable cache
    /// yields an empty catalogue, which prices nothing rather than pricing wrong.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_from(path),
            None => Self::default(),
        }
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut catalog = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<RateCatalog>(&raw).ok())
            .filter(|c| c.format == CACHE_FORMAT)
            .unwrap_or_default();
        catalog.path = Some(path);
        catalog
    }

    /// A catalogue built in memory, for tests that must not touch the network or
    /// the user's real cache.
    #[doc(hidden)]
    pub fn for_test<'a>(entries: impl IntoIterator<Item = (&'a str, ModelPrice)>) -> Self {
        Self {
            format: CACHE_FORMAT,
            fetched_at: Some(now_secs()),
            models: entries
                .into_iter()
                .map(|(id, price)| (normalize_model_name(id), price))
                .collect(),
            path: None,
        }
    }

    pub fn get(&self, model_id: &str) -> Option<&ModelPrice> {
        let key = normalize_model_name(model_id);
        if key.is_empty() || UNPRICEABLE.contains(&key.as_str()) {
            return None;
        }
        self.models.get(&key)
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn fetched_at(&self) -> Option<u64> {
        self.fetched_at
    }

    /// Whether the cache is old enough to be worth re-fetching. A catalogue that
    /// has never been fetched is stale by definition.
    pub fn is_stale(&self) -> bool {
        match self.fetched_at {
            Some(at) => now_secs().saturating_sub(at) >= RATES_TTL_SECS,
            None => true,
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        fsutil::atomic_write_json(path, self)
    }
}

/// Fetch the published table and replace the cache, unless it is already fresh.
///
/// Returns the catalogue that should now be used, fetched or not. A failed fetch
/// is not an error the caller has to handle: the previous cache is returned and
/// keeps working, because rates that are a week old price far better than no
/// rates at all.
pub async fn refresh(force: bool) -> RateCatalog {
    let existing = RateCatalog::load();
    if !force && !existing.is_stale() {
        return existing;
    }

    let url = std::env::var("ZEST_RATES_URL").unwrap_or_else(|_| DEFAULT_RATES_URL.to_string());
    match fetch(&url).await {
        Ok(models) if !models.is_empty() => {
            let refreshed = RateCatalog {
                format: CACHE_FORMAT,
                fetched_at: Some(now_secs()),
                models,
                path: existing.path.clone(),
            };
            // A write failure costs one day's freshness, not correctness: the
            // in-memory catalogue is already right for this session.
            let _ = refreshed.save();
            refreshed
        }
        // An empty table would blank out every price. Keep what we had.
        _ => existing,
    }
}

async fn fetch(url: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("zest/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("rate table HTTP {}", response.status()));
    }
    let document: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    Ok(project(&document))
}

/// Project the published document into Zest's per-million rate table.
///
/// Public so the projection is testable against a real document without a
/// network round trip.
pub fn project(document: &serde_json::Value) -> BTreeMap<String, ModelPrice> {
    let mut table = BTreeMap::new();
    let Some(entries) = document.as_object() else {
        return table;
    };

    for (name, raw) in entries {
        let Some(entry) = raw.as_object() else {
            continue;
        };
        // An entry priced on one side only would under-report every turn that
        // used it. Reporting the model as unpriced is the honest failure.
        let (Some(input), Some(output)) = (
            finite(entry.get("input_cost_per_token")),
            finite(entry.get("output_cost_per_token")),
        ) else {
            continue;
        };

        let key = normalize_model_name(name);
        if key.is_empty() || UNPRICEABLE.contains(&key.as_str()) {
            continue;
        }

        // Absent cache rates are left absent rather than defaulted here, so the
        // "unstated bills as input" rule lives in exactly one place —
        // `ModelPrice`'s accessors — instead of being re-decided per source.
        let price = match (
            finite(entry.get("cache_creation_input_token_cost")),
            finite(entry.get("cache_read_input_token_cost")),
        ) {
            (Some(write), Some(read)) => ModelPrice::new(
                per_million(input),
                per_million(output),
                per_million(write),
                per_million(read),
            ),
            _ => ModelPrice::simple(per_million(input), per_million(output)),
        };
        table.insert(key, price);
    }
    table
}

/// The published table quotes per token; Zest works in per million throughout,
/// because a rate like `0.0000025` is unreadable in a config file a human edits.
fn per_million(per_token: f64) -> f64 {
    per_token * 1_000_000.0
}

fn finite(value: Option<&serde_json::Value>) -> Option<f64> {
    value.and_then(|v| v.as_f64()).filter(|v| v.is_finite())
}

/// Canonical form for lookup.
///
/// Lowercased, and the `provider/` prefix dropped — the published table carries
/// both `claude-opus-5` and `anthropic/claude-opus-5`, and transcripts are
/// inconsistent about which they record. Only the last segment is kept, matching
/// how the table itself is namespaced.
pub fn normalize_model_name(model: &str) -> String {
    let trimmed = model.trim().to_ascii_lowercase();
    match trimmed.rfind('/') {
        Some(slash) => trimmed[slash + 1..].to_string(),
        None => trimmed,
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
    use serde_json::json;

    #[test]
    fn per_token_rates_become_per_million() {
        let table = project(&json!({
            "claude-sonnet-4-6": {
                "input_cost_per_token": 0.000003,
                "output_cost_per_token": 0.000015,
                "cache_read_input_token_cost": 0.0000003,
                "cache_creation_input_token_cost": 0.00000375,
            }
        }));

        let price = table.get("claude-sonnet-4-6").expect("projected");
        assert!((price.input - 3.0).abs() < 1e-9);
        assert!((price.output - 15.0).abs() < 1e-9);
        assert!((price.cache_read() - 0.3).abs() < 1e-9);
        assert!((price.cache_write() - 3.75).abs() < 1e-9);
    }

    #[test]
    fn an_entry_priced_on_one_side_only_is_dropped() {
        // Keeping it would under-report every turn that used the model, which is
        // the one failure mode worse than reporting it unpriced.
        let table = project(&json!({
            "half-priced": { "input_cost_per_token": 0.000003 },
            "free-embedding": { "output_cost_per_token": 0.0 },
        }));
        assert!(table.is_empty());
    }

    #[test]
    fn an_absent_cache_rate_stays_absent_and_bills_as_input() {
        // The common case: most published entries quote no cache rates at all.
        let table = project(&json!({
            "headline-only": {
                "input_cost_per_token": 0.000003,
                "output_cost_per_token": 0.000015,
            }
        }));
        let price = table.get("headline-only").unwrap();
        assert_eq!(price.cache_read(), 3.0);
        assert_eq!(price.cache_write(), 3.0);
    }

    #[test]
    fn a_provider_prefixed_entry_is_reachable_by_its_bare_name() {
        let table = project(&json!({
            "anthropic/claude-opus-5": {
                "input_cost_per_token": 0.000005,
                "output_cost_per_token": 0.000025,
            }
        }));
        assert!(table.contains_key("claude-opus-5"));
    }

    #[test]
    fn ambiguous_family_names_are_never_priced() {
        // "opus" has meant several models at several prices. Costing it against
        // whichever one the table happens to list is a guess wearing a number.
        let table = project(&json!({
            "opus": { "input_cost_per_token": 0.000015, "output_cost_per_token": 0.000075 },
            "claude-opus-5": { "input_cost_per_token": 0.000005, "output_cost_per_token": 0.000025 },
        }));
        assert!(!table.contains_key("opus"));
        assert!(table.contains_key("claude-opus-5"));

        let catalog = RateCatalog {
            models: table,
            ..Default::default()
        };
        assert!(catalog.get("opus").is_none());
        assert!(catalog.get("<synthetic>").is_none());
        assert!(catalog.get("claude-opus-5").is_some());
    }

    #[test]
    fn lookup_ignores_casing_and_provider_prefixes() {
        let catalog = RateCatalog {
            models: project(&json!({
                "gpt-5.6-terra": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000012,
                }
            })),
            ..Default::default()
        };
        assert!(catalog.get("GPT-5.6-Terra").is_some());
        assert!(catalog.get("openai/gpt-5.6-terra").is_some());
        assert!(catalog.get("gpt-5.6-nonexistent").is_none());
    }

    #[test]
    fn a_never_fetched_catalogue_is_stale_and_prices_nothing() {
        let catalog = RateCatalog::default();
        assert!(catalog.is_stale());
        assert!(catalog.is_empty());
        assert!(catalog.get("claude-opus-5").is_none());
    }

    #[test]
    fn a_cache_written_in_an_older_format_is_discarded() {
        let dir = std::env::temp_dir().join("zest-rates-format");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model-rates.json");
        std::fs::write(
            &path,
            r#"{"format":0,"fetched_at":9999999999,"models":{"claude-opus-5":{"input":5.0,"output":25.0}}}"#,
        )
        .unwrap();

        let catalog = RateCatalog::load_from(&path);
        assert!(catalog.is_empty(), "an unreadable shape prices nothing");
        assert!(catalog.is_stale(), "and asks to be fetched again");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_catalogue_round_trips() {
        let dir = std::env::temp_dir().join("zest-rates-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("model-rates.json");

        let written = RateCatalog {
            format: CACHE_FORMAT,
            fetched_at: Some(now_secs()),
            models: project(&json!({
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.00003,
                    "cache_read_input_token_cost": 0.0000005,
                    "cache_creation_input_token_cost": 0.00000625,
                }
            })),
            path: Some(path.clone()),
        };
        written.save().unwrap();

        let reloaded = RateCatalog::load_from(&path);
        assert_eq!(reloaded.len(), 1);
        assert!(!reloaded.is_stale(), "just fetched");
        let price = reloaded.get("gpt-5.6-sol").unwrap();
        assert!((price.input - 5.0).abs() < 1e-9);
        assert!((price.cache_read() - 0.5).abs() < 1e-9);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
