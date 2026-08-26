//! The price book: what a model would cost at published API rates.
//!
//! This file exists to answer one question — "what did that turn cost?" — and it
//! is careful about how much authority the answer claims. Zest has no billing
//! relationship with any provider, so nothing here is a bill. It is arithmetic
//! over metered tokens, at rates somebody published.
//!
//! Rates themselves come from [`crate::rates`], which tracks the LiteLLM table.
//! This module owns two things that table cannot: the **override file**, where a
//! user corrects a rate for their own account, and the **arithmetic**, which has
//! to be right about how the four token classes bill.
//!
//! Three rules keep it honest:
//!
//! 1. **A model with no rate stays unpriced.** Not zero, not a guess from a
//!    similar-looking model. [`Prices::lookup`] returns `None` and the caller is
//!    expected to surface that as unpriced coverage rather than silently drop
//!    the tokens out of the total.
//! 2. **An unstated rate is not a zero rate.** An omitted cache price bills at
//!    the input rate — see [`ModelPrice::cache_write`]. Most published entries
//!    omit them, and an agent loop is mostly cache traffic, so reading absent as
//!    free would report a real bill as approximately nothing.
//! 3. **A subscription is not an API rate.** A figure derived from this book is
//!    what the same traffic would have cost on metered API access. For anyone on
//!    a plan it is an upper bound on value received, not money owed, and every
//!    surface that shows it is expected to say so.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rates::RateCatalog;

/// Bumped when the seeded template changes, so a fresh file can be told apart
/// from one a user has since curated.
pub const BUILTIN_REVISION: &str = "2026-08-08";

/// USD per million tokens, one model.
///
/// Four rates rather than the usual two because cache traffic is most of the
/// volume in an agent loop and bills nothing like fresh input — collapsing them
/// would overstate a cache-heavy day by an order of magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    /// Fresh (uncached) input.
    pub input: f64,
    pub output: f64,
    /// Writing a cache entry. Typically a premium over `input`.
    ///
    /// Optional, and absent is **not** zero — see [`ModelPrice::cache_write`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write: Option<f64>,
    /// Reading one back. Typically a large discount on `input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read: Option<f64>,
}

impl ModelPrice {
    pub fn new(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_write: Some(cache_write),
            cache_read: Some(cache_read),
        }
    }

    /// A model quoted with only the two headline rates.
    ///
    /// Cache traffic then bills at the input rate — see the accessors for why
    /// that, and not zero, is the safe reading.
    pub fn simple(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_write: None,
            cache_read: None,
        }
    }

    /// The cache-write rate, falling back to the input rate.
    ///
    /// The fallback is the whole point of these being optional. An omitted rate
    /// means "not stated", and the only safe reading of an unstated rate is that
    /// the tokens billed like ordinary input. Treating absent as zero would make
    /// a cache-heavy agent loop — which is most of the traffic — look free, and
    /// under-reporting spend is a worse failure than over-reporting it. A rate
    /// written explicitly as `0.0` is still honoured: some providers genuinely
    /// do not charge for cache writes, and that is a stated fact, not a gap.
    pub fn cache_write(&self) -> f64 {
        self.cache_write.unwrap_or(self.input)
    }

    /// The cache-read rate, falling back to the input rate. See
    /// [`ModelPrice::cache_write`].
    pub fn cache_read(&self) -> f64 {
        self.cache_read.unwrap_or(self.input)
    }

    /// What one bucket of metered tokens comes to, in USD.
    fn cost(&self, counts: &Counts) -> f64 {
        (counts.input_tokens as f64 * self.input
            + counts.output_tokens as f64 * self.output
            + counts.cache_write_tokens as f64 * self.cache_write()
            + counts.cache_read_tokens as f64 * self.cache_read())
            / 1_000_000.0
    }

    /// What the cache reads would have cost as fresh input, minus what they did
    /// cost. The saving is only meaningful against this model's own input rate,
    /// which is why it lives here rather than in the report.
    fn cache_saving(&self, cache_read_tokens: u64) -> f64 {
        let delta = (self.input - self.cache_read()).max(0.0);
        cache_read_tokens as f64 * delta / 1_000_000.0
    }
}

/// The subset of a usage bucket that pricing cares about.
///
/// Deliberately its own small type: [`crate::usage`] owns the accumulating
/// counters, and pricing should not be able to reach into a ledger and change
/// one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
}

/// The price book as read from disk.
///
/// Two layers, and the order between them is the point. The published catalogue
/// ([`crate::rates`]) covers thousands of models and stays current on its own.
/// This file covers the handful the catalogue gets wrong for *you* — a negotiated
/// rate, a gateway markup, a model too new to be published yet — and always wins.
/// Neither layer can be edited by the other: Zest seeds this file once and never
/// rewrites it, and refreshing the catalogue never touches it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prices {
    /// Which built-in table this file was seeded from. Purely informational —
    /// Zest never migrates a file the user may have edited.
    #[serde(default)]
    pub revision: String,
    /// Keyed by model id (`claude-opus-4-8`), or by `provider/model` when one
    /// provider serves the same model at a different rate. The qualified key
    /// wins; see [`Prices::lookup`].
    #[serde(default)]
    pub models: BTreeMap<String, ModelPrice>,
    /// The published rate table, consulted only when this file has no answer.
    #[serde(skip)]
    catalog: RateCatalog,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Default for Prices {
    fn default() -> Self {
        Self {
            revision: BUILTIN_REVISION.to_string(),
            models: BTreeMap::new(),
            catalog: RateCatalog::default(),
            path: None,
        }
    }
}

impl Prices {
    /// `<data dir>/zest/prices.toml`, beside the ledger it prices.
    ///
    /// Outside the project for the same reason the ledger is: rates follow the
    /// account, not the repository you happen to have open.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("zest").join("prices.toml"))
    }

    /// Load the book, seeding the file from the built-in table if it is missing.
    ///
    /// A seed failure is not an error: pricing must never be the reason a
    /// session refuses to start, and an in-memory book prices exactly as well as
    /// a written one.
    pub fn load() -> Self {
        let mut prices = match Self::default_path() {
            Some(path) => Self::load_from(path),
            None => Self::default(),
        };
        prices.catalog = RateCatalog::load();
        prices
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let mut prices = toml::from_str::<Prices>(&raw).unwrap_or_else(|_| {
                    // A hand-edited file with a typo in it must not silently
                    // become "everything is free". Dropping to no overrides
                    // leaves the published catalogue pricing as usual, and the
                    // user's file is left untouched so the mistake is findable.
                    Self::default()
                });
                prices.path = Some(path);
                prices
            }
            Err(_) => {
                let prices = Self {
                    path: Some(path.clone()),
                    ..Self::default()
                };
                let _ = prices.seed(&path);
                prices
            }
        }
    }

    /// Attach a catalogue explicitly. Used by callers that have just refreshed
    /// one and should not re-read it from disk.
    pub fn with_catalog(mut self, catalog: RateCatalog) -> Self {
        self.catalog = catalog;
        self
    }

    pub fn catalog(&self) -> &RateCatalog {
        &self.catalog
    }

    /// Write the override file out as a commented, empty template.
    fn seed(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fsutil::atomic_write(path, self.to_toml().as_bytes())
    }

    /// Render the book as TOML with the header that explains what it is.
    ///
    /// Hand-written rather than `toml::to_string` so the file a user opens leads
    /// with the caveats instead of with data.
    ///
    /// A fresh file carries **no** model stanzas. Seeding it with copies of
    /// published rates would be actively harmful: those copies would outrank the
    /// catalogue forever and go stale the first time a provider changed a price,
    /// which is the exact failure this file exists to let you fix.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(concat!(
            "# Zest price overrides — USD per million tokens.\n",
            "#\n",
            "# Zest prices tokens against the published LiteLLM rate table, refreshed\n",
            "# daily and cached next to this file. You do not need to list anything here\n",
            "# for the common case; this file is only for rates that table gets wrong for\n",
            "# you — a negotiated price, a gateway markup, or a model too new to be\n",
            "# published. Anything you write here wins.\n",
            "#\n",
            "#   [models.\"my-model-id\"]\n",
            "#   input = 1.25        # fresh, uncached input\n",
            "#   output = 10.0\n",
            "#   cache_write = 1.5625\n",
            "#   cache_read = 0.125\n",
            "#\n",
            "# Use \"provider/model\" as the key when one provider serves a model at its\n",
            "# own rate; that form is checked before the bare model name.\n",
            "#\n",
            "# Omitting cache_write/cache_read is not the same as setting them to zero:\n",
            "# an omitted rate bills at the input rate, because an unstated discount is\n",
            "# not a discount. Write an explicit 0.0 for genuinely free cache traffic.\n",
            "#\n",
            "# None of this is a bill. Zest has no billing relationship with any provider\n",
            "# and a subscription does not charge at list rates at all — every figure\n",
            "# derived from these numbers is \"what this traffic would have cost on\n",
            "# metered API access\".\n",
            "#\n",
            "# Zest seeds this file once and never rewrites it.\n\n",
        ));
        out.push_str(&format!("revision = \"{}\"\n", self.revision));
        for (id, price) in &self.models {
            out.push_str(&format!("\n[models.\"{id}\"]\n"));
            out.push_str(&format!("input = {}\n", price.input));
            out.push_str(&format!("output = {}\n", price.output));
            out.push_str(&format!("cache_write = {}\n", price.cache_write()));
            out.push_str(&format!("cache_read = {}\n", price.cache_read()));
        }
        out
    }

    /// The rate for one model, or `None` if nothing knows it.
    ///
    /// Three layers, most specific first:
    ///
    /// 1. `provider/model` in the override file — a gateway reselling a model at
    ///    its own rate, priced without shadowing that model everywhere else.
    /// 2. the bare model name in the override file;
    /// 3. the published catalogue.
    ///
    /// The override file is consulted before the catalogue and never merged into
    /// it, so a correction stays a correction rather than being silently undone
    /// by the next daily refresh.
    pub fn lookup(&self, provider_id: &str, model_id: &str) -> Option<&ModelPrice> {
        self.models
            .get(&format!("{provider_id}/{model_id}"))
            .or_else(|| self.models.get(model_id))
            .or_else(|| self.catalog.get(model_id))
    }

    /// Cost and cache saving for one model's metered tokens.
    ///
    /// `None` all the way through for an unknown model — the caller decides how
    /// to report the gap, and none of the options is "zero".
    pub fn price(&self, provider_id: &str, model_id: &str, counts: &Counts) -> Option<PricedUsage> {
        let price = self.lookup(provider_id, model_id)?;
        Some(PricedUsage {
            cost_usd: price.cost(counts),
            cache_saving_usd: price.cache_saving(counts.cache_read_tokens),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// What one model's tokens came to.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PricedUsage {
    pub cost_usd: f64,
    /// What the cache reads in this bucket would have cost as fresh input,
    /// beyond what they actually cost.
    pub cache_saving_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> Counts {
        Counts {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_write_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
        }
    }

    /// A book backed by a one-model catalogue, standing in for the published
    /// table so these tests never touch the network or the user's real cache.
    fn with_catalog() -> Prices {
        Prices::default().with_catalog(crate::rates::RateCatalog::for_test([(
            "claude-sonnet-4-6",
            ModelPrice::new(3.0, 15.0, 3.75, 0.3),
        )]))
    }

    #[test]
    fn an_unknown_model_is_unpriced_not_free() {
        let prices = with_catalog();
        assert!(prices.lookup("codex", "gpt-nonexistent").is_none());
        assert!(prices
            .price("codex", "gpt-nonexistent", &counts())
            .is_none());
    }

    #[test]
    fn each_token_class_bills_at_its_own_rate() {
        let prices = with_catalog();
        let priced = prices
            .price("anthropic", "claude-sonnet-4-6", &counts())
            .expect("known model");
        // 3 + 15 + 3.75 + 0.30, one million of each.
        assert!((priced.cost_usd - 22.05).abs() < 1e-9, "{priced:?}");
        // A cache read cost 0.30 where fresh input would have cost 3.00.
        assert!((priced.cache_saving_usd - 2.70).abs() < 1e-9, "{priced:?}");
    }

    #[test]
    fn a_provider_qualified_rate_wins_without_shadowing_the_model() {
        let mut prices = with_catalog();
        prices.models.insert(
            "gateway/claude-sonnet-4-6".into(),
            ModelPrice::new(6.0, 30.0, 7.5, 0.6),
        );

        assert_eq!(
            prices.lookup("gateway", "claude-sonnet-4-6").unwrap().input,
            6.0
        );
        // The catalogue still answers for everyone else.
        assert_eq!(
            prices
                .lookup("anthropic", "claude-sonnet-4-6")
                .unwrap()
                .input,
            3.0
        );
    }

    #[test]
    fn an_override_outranks_the_catalogue_and_survives_a_refresh() {
        // The whole point of the override file: a correction must not be undone
        // by tomorrow's fetch.
        let mut prices = with_catalog();
        prices.models.insert(
            "claude-sonnet-4-6".into(),
            ModelPrice::new(1.0, 2.0, 3.0, 4.0),
        );
        assert_eq!(
            prices
                .lookup("anthropic", "claude-sonnet-4-6")
                .unwrap()
                .input,
            1.0
        );

        let refreshed = prices.with_catalog(crate::rates::RateCatalog::for_test([(
            "claude-sonnet-4-6",
            ModelPrice::new(99.0, 99.0, 99.0, 99.0),
        )]));
        assert_eq!(
            refreshed
                .lookup("anthropic", "claude-sonnet-4-6")
                .unwrap()
                .input,
            1.0,
            "the override still wins after the catalogue changed"
        );
    }

    #[test]
    fn a_fresh_override_file_carries_no_rates_of_its_own() {
        // Seeding it with copies of published rates would outrank the catalogue
        // forever and go stale the first time a provider changed a price.
        let seeded = Prices::default().to_toml();
        // The commented example is guidance; nothing may be live.
        assert!(
            !seeded.lines().any(|line| line.starts_with("[models.")),
            "no live stanzas, only guidance:\n{seeded}"
        );
        let reparsed: Prices = toml::from_str(&seeded).expect("template parses");
        assert!(reparsed.models.is_empty());
    }

    #[test]
    fn a_seeded_file_round_trips_through_the_written_format() {
        let prices = Prices::default();
        let reparsed: Prices = toml::from_str(&prices.to_toml()).expect("seeded file parses");

        assert_eq!(reparsed.revision, BUILTIN_REVISION);
        assert_eq!(reparsed.models, prices.models);
    }

    #[test]
    fn a_broken_file_drops_to_the_catalogue_rather_than_to_free() {
        let dir = std::env::temp_dir().join("zest-prices-broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prices.toml");
        std::fs::write(&path, "this is not toml [[[").unwrap();

        let prices =
            Prices::load_from(&path).with_catalog(crate::rates::RateCatalog::for_test([(
                "claude-sonnet-4-6",
                ModelPrice::new(3.0, 15.0, 3.75, 0.3),
            )]));
        // One unparseable override file must not make every model free.
        assert!(prices.lookup("anthropic", "claude-sonnet-4-6").is_some());
        // The user's file is left alone so the typo is still there to be found.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not toml [[["
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_seeded_and_reloads_identically() {
        let dir = std::env::temp_dir().join("zest-prices-seed");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prices.toml");

        let seeded = Prices::load_from(&path);
        assert!(path.is_file(), "first load writes the file");

        let reloaded = Prices::load_from(&path);
        assert_eq!(reloaded.models, seeded.models);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unstated_cache_rate_bills_as_input_rather_than_as_free() {
        // The failure this guards: an agent loop is mostly cache traffic, so
        // reading "absent" as zero would report a real bill as roughly nothing.
        let prices: Prices = toml::from_str(
            r#"
            [models."quoted-headline-only"]
            input = 3.0
            output = 15.0
            "#,
        )
        .unwrap();

        let rate = prices.lookup("any", "quoted-headline-only").unwrap();
        assert_eq!(rate.cache_write(), 3.0);
        assert_eq!(rate.cache_read(), 3.0);

        let priced = prices
            .price("any", "quoted-headline-only", &counts())
            .unwrap();
        // 3 + 15 + 3 + 3 across a million tokens of each.
        assert!((priced.cost_usd - 24.0).abs() < 1e-9, "{priced:?}");
        // Nothing was saved, because nothing was discounted.
        assert_eq!(priced.cache_saving_usd, 0.0);
    }

    #[test]
    fn a_rate_written_as_zero_is_honoured_as_free() {
        // Distinct from the case above: some providers really do not charge for
        // cache writes, and a stated zero is a fact rather than a gap.
        let prices: Prices = toml::from_str(
            r#"
            [models."free-cache-writes"]
            input = 3.0
            output = 15.0
            cache_write = 0.0
            cache_read = 0.3
            "#,
        )
        .unwrap();

        let rate = prices.lookup("any", "free-cache-writes").unwrap();
        assert_eq!(rate.cache_write(), 0.0);
        assert_eq!(rate.cache_read(), 0.3);
    }

    #[test]
    fn an_empty_book_prices_nothing_and_says_so() {
        // The shape a user gets if they delete every stanza. It must report no
        // coverage, not a total of zero dollars.
        let prices: Prices = toml::from_str("revision = \"custom\"").unwrap();
        assert!(prices.models.is_empty());
        assert!(prices
            .price("anthropic", "claude-sonnet-4-6", &counts())
            .is_none());
    }
}
