//! The Cursor model catalogue: discovered from the CLI, cached on disk.
//!
//! `cursor-agent models` prints the models *this account* can actually use, so
//! a hand-written list is wrong for everyone else — a Grok-enabled plan showed
//! no Grok, because a built-in list can only ever describe whoever wrote it.
//!
//! # Two id spaces
//!
//! Cursor names a model two ways. Its `--model` flag takes a flat id whose
//! effort is a suffix (`cursor-grok-4.6-high`, `claude-opus-5-thinking-max`),
//! while ACP's `session/new` reports a parameterized one
//! (`claude-opus-5[thinking=true,effort=high]`). Zest has its own effort axis,
//! so the suffix is split off here: the catalogue offers `cursor-grok-4.6` with
//! efforts `[low, medium, high, xhigh]`, and [`wire_model`] puts the pair back
//! together at launch. Without that split Zest would list two hundred models
//! and still show an effort selector that changed nothing.
//!
//! `-fast` is not an effort. It stays part of the family id, because it is a
//! different model choice rather than more thinking about the same one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{context_window_for_model, ModelSpec, STANDARD_EFFORTS};

/// How long a discovered catalogue is trusted. Cursor adds models often enough
/// that a week would go stale, and `cursor-agent models` costs about a second,
/// which is far too slow to pay on every render of the provider list.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_FORMAT: u32 = 1;

/// Fallback when discovery has never succeeded.
///
/// Deliberately short and generic: a guess that names families every account is
/// likely to have, rather than one pretending to know a specific plan. The real
/// list replaces it as soon as the CLI answers once.
pub const BUILTIN_MODELS: &[&str] = &[
    "composer-2.5",
    "claude-opus-5-thinking",
    "claude-sonnet-5-thinking",
    "cursor-grok-4.6",
    "gpt-5.6-sol",
    "gemini-3.1-pro",
];

#[derive(Debug, Default, Serialize, Deserialize)]
struct CachedCatalogue {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    fetched_at: u64,
    #[serde(default)]
    models: Vec<ModelSpec>,
}

/// `<data dir>/zest/cursor-models.json`, beside the rate cache.
pub fn cache_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("zest").join("cursor-models.json"))
}

/// The catalogue for this account, refreshing the cache when it has gone stale.
///
/// Never fails: discovery problems fall back to whatever was cached, and then to
/// [`BUILTIN_MODELS`]. A model picker that empties itself because a CLI was busy
/// would be worse than one showing a slightly old list.
pub fn catalogue(command: &str) -> Vec<ModelSpec> {
    catalogue_with(cache_path(), command)
}

/// [`catalogue`] against an explicit cache location.
///
/// Split out so tests can exercise the staleness and fallback rules without
/// reading, or overwriting, the real cache belonging to whoever runs them.
fn catalogue_with(path: Option<PathBuf>, command: &str) -> Vec<ModelSpec> {
    let cached = path.as_deref().and_then(read_cache);
    if let Some(cached) = cached.as_ref() {
        if !is_stale(cached.fetched_at) && !cached.models.is_empty() {
            return cached.models.clone();
        }
    }

    match discover(command) {
        Some(models) if !models.is_empty() => {
            if let Some(path) = path {
                write_cache(&path, &models);
            }
            models
        }
        _ => match cached.filter(|cached| !cached.models.is_empty()) {
            Some(cached) => cached.models,
            None => fallback(),
        },
    }
}

/// The built-in list as specs, used until the CLI has answered once.
pub fn fallback() -> Vec<ModelSpec> {
    BUILTIN_MODELS
        .iter()
        .map(|id| spec(id.to_string(), efforts_for_fallback(id), None))
        .collect()
}

/// Every family Cursor ships supports the standard ladder except the ones that
/// take no effort at all, and we cannot tell which is which without discovery.
/// Offering the ladder is the recoverable guess: a rejected effort is one clear
/// error, while hiding a real one is invisible.
fn efforts_for_fallback(_id: &str) -> Vec<String> {
    STANDARD_EFFORTS.iter().map(|s| (*s).to_string()).collect()
}

fn is_stale(fetched_at: u64) -> bool {
    now_secs().saturating_sub(fetched_at) > CACHE_TTL.as_secs()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &Path) -> Option<CachedCatalogue> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<CachedCatalogue>(&raw)
        .ok()
        .filter(|cached| cached.format == CACHE_FORMAT)
}

fn write_cache(path: &Path, models: &[ModelSpec]) {
    let cached = CachedCatalogue {
        format: CACHE_FORMAT,
        fetched_at: now_secs(),
        models: models.to_vec(),
    };
    if let Ok(raw) = serde_json::to_vec_pretty(&cached) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = crate::fsutil::atomic_write(path, &raw);
    }
}

/// Run `cursor-agent models` and parse it. `None` when the CLI is missing,
/// signed out, or slow — every one of which is a reason to keep the old list.
fn discover(command: &str) -> Option<Vec<ModelSpec>> {
    let mut process =
        std::process::Command::new(crate::tools::external_agent::resolve_program(command));
    process.arg("models");
    crate::tools::external_agent::prepare_sync_external_command(&mut process);
    let output = process.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// Turn `cursor-agent models` output into per-family specs.
///
/// Lines look like `claude-opus-5-thinking-high - Claude Opus 5 1M Thinking`.
/// Anything that is not `id - Display Name` is a heading or blank and skipped.
pub fn parse(stdout: &str) -> Vec<ModelSpec> {
    // BTreeMap so the picker order is stable between runs rather than following
    // whatever order the account happens to report.
    let mut families: BTreeMap<String, (Vec<String>, Option<u64>)> = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        let Some((id, display)) = line.split_once(" - ") else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || !id.chars().all(is_model_id_char) {
            continue;
        }
        // `composer-2.5 - Composer 2.5 (current)` marks the CLI's own default.
        let display = display.trim().trim_end_matches("(current)").trim();
        let (family, effort) = split_effort(id);
        let entry = families.entry(family).or_default();
        if let Some(effort) = effort {
            if !entry.0.contains(&effort) {
                entry.0.push(effort);
            }
        }
        if entry.1.is_none() {
            entry.1 = context_from_display(display);
        }
    }

    families
        .into_iter()
        .map(|(family, (mut efforts, context))| {
            // Report the ladder in its own order, not discovery order.
            efforts.sort_by_key(|effort| {
                STANDARD_EFFORTS
                    .iter()
                    .position(|known| known == effort)
                    .unwrap_or(usize::MAX)
            });
            spec(family, efforts, context)
        })
        .collect()
}

fn spec(id: String, efforts: Vec<String>, context: Option<u64>) -> ModelSpec {
    ModelSpec {
        context_window: context.unwrap_or_else(|| context_window_for_model(&id)),
        id,
        efforts,
        supports_tools: true,
        // Every Cursor model runs behind the same agent, and `initialize`
        // reports `promptCapabilities.image: true` for the session rather than
        // per model.
        supports_vision: true,
    }
}

fn is_model_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'
}

/// Split a trailing effort out of a flat Cursor id.
///
/// `-fast` is stripped first and re-attached to the family, because it can sit
/// on either side of an effort (`composer-2.5-fast`, `grok-4.6-high-fast`) and
/// is a model choice rather than an effort level.
fn split_effort(id: &str) -> (String, Option<String>) {
    let (body, fast) = match id.strip_suffix("-fast") {
        Some(body) => (body, true),
        None => (id, false),
    };
    for effort in STANDARD_EFFORTS {
        if let Some(family) = body.strip_suffix(&format!("-{effort}")) {
            if family.is_empty() {
                break;
            }
            return (with_fast(family, fast), Some((*effort).to_string()));
        }
    }
    (with_fast(body, fast), None)
}

fn with_fast(family: &str, fast: bool) -> String {
    if fast {
        format!("{family}-fast")
    } else {
        family.to_string()
    }
}

/// Rebuild the flat id `--model` expects from a family and Zest's effort.
///
/// The inverse of [`split_effort`], and the reason the family keeps `-fast` as
/// a suffix: the effort goes *before* it, which is the order Cursor uses.
pub fn wire_model(family: &str, effort: Option<&str>) -> String {
    let effort = effort.map(str::trim).filter(|effort| {
        !effort.is_empty() && STANDARD_EFFORTS.iter().any(|known| known == effort)
    });
    let Some(effort) = effort else {
        return family.to_string();
    };
    match family.strip_suffix("-fast") {
        Some(base) => format!("{base}-{effort}-fast"),
        None => format!("{family}-{effort}"),
    }
}

/// Cursor writes the window into the display name (`Claude Opus 5 1M Thinking`),
/// which is the only place the CLI states it at all.
fn context_from_display(display: &str) -> Option<u64> {
    for word in display.split_whitespace() {
        let word = word.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        if let Some(millions) = word.strip_suffix('M').or_else(|| word.strip_suffix('m')) {
            if let Ok(value) = millions.parse::<u64>() {
                return Some(value * 1_000_000);
            }
        }
        if let Some(thousands) = word.strip_suffix('k').or_else(|| word.strip_suffix('K')) {
            if let Ok(value) = thousands.parse::<u64>() {
                return Some(value * 1_000);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Available models\n\
\n\
auto - Auto (default)\n\
composer-2.5 - Composer 2.5 (current)\n\
composer-2.5-fast - Composer 2.5 Fast\n\
cursor-grok-4.6-low - Cursor Grok 4.6 Low\n\
cursor-grok-4.6-medium - Cursor Grok 4.6 Medium\n\
cursor-grok-4.6-high - Cursor Grok 4.6\n\
cursor-grok-4.6-high-fast - Cursor Grok 4.6 Fast\n\
cursor-grok-4.6-xhigh - Cursor Grok 4.6 Extra High\n\
claude-opus-5-thinking-max - Claude Opus 5 1M Max Thinking\n\
gpt-5.6-sol-high - GPT-5.6 Sol 1M High\n";

    fn find<'a>(models: &'a [ModelSpec], id: &str) -> &'a ModelSpec {
        models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("no `{id}` in {:?}", ids(models)))
    }

    fn ids(models: &[ModelSpec]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    #[test]
    fn the_grok_family_survives_discovery_as_one_model_with_a_ladder() {
        // The bug this whole module exists for: a hand-written catalogue had no
        // Grok at all, on an account that has seven of them.
        let models = parse(SAMPLE);
        let grok = find(&models, "cursor-grok-4.6");
        assert_eq!(grok.efforts, vec!["low", "medium", "high", "xhigh"]);
    }

    #[test]
    fn fast_is_a_family_and_not_an_effort() {
        let models = parse(SAMPLE);
        // `-fast` splits into its own row rather than collapsing into the
        // plain family, because picking it is picking a different model.
        assert_eq!(find(&models, "composer-2.5").efforts, Vec::<String>::new());
        assert_eq!(
            find(&models, "cursor-grok-4.6-fast").efforts,
            vec!["high".to_string()]
        );
        assert!(ids(&models).contains(&"composer-2.5-fast"));
    }

    #[test]
    fn a_family_and_effort_round_trip_back_to_the_flat_cli_id() {
        for (family, effort, expected) in [
            ("cursor-grok-4.6", Some("high"), "cursor-grok-4.6-high"),
            (
                "cursor-grok-4.6-fast",
                Some("high"),
                "cursor-grok-4.6-high-fast",
            ),
            (
                "claude-opus-5-thinking",
                Some("max"),
                "claude-opus-5-thinking-max",
            ),
            ("composer-2.5", None, "composer-2.5"),
            // An effort the ladder does not name must not be pasted on.
            ("composer-2.5", Some("turbo"), "composer-2.5"),
            ("composer-2.5-fast", None, "composer-2.5-fast"),
        ] {
            assert_eq!(
                wire_model(family, effort),
                expected,
                "{family} + {effort:?}"
            );
        }
    }

    #[test]
    fn every_parsed_id_rebuilds_into_something_cursor_listed() {
        // The round trip has to hold for the real output shape, not just the
        // cases someone thought to write down.
        let listed: Vec<&str> = SAMPLE
            .lines()
            .filter_map(|line| line.split_once(" - "))
            .map(|(id, _)| id.trim())
            .collect();
        for model in parse(SAMPLE) {
            if model.efforts.is_empty() {
                assert!(listed.contains(&model.id.as_str()), "{}", model.id);
                continue;
            }
            for effort in &model.efforts {
                let wire = wire_model(&model.id, Some(effort));
                assert!(listed.contains(&wire.as_str()), "{wire}");
            }
        }
    }

    #[test]
    fn the_context_window_comes_from_cursors_own_label() {
        let models = parse(SAMPLE);
        assert_eq!(
            find(&models, "claude-opus-5-thinking").context_window,
            1_000_000
        );
        assert_eq!(find(&models, "gpt-5.6-sol").context_window, 1_000_000);
        // No label, so the shared heuristic answers instead of a guess of ours.
        assert_eq!(
            find(&models, "composer-2.5").context_window,
            context_window_for_model("composer-2.5")
        );
    }

    #[test]
    fn a_fresh_cache_is_used_and_a_failed_discovery_still_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor-models.json");
        write_cache(&path, &parse(SAMPLE));

        // A command that cannot run stands in for a missing or signed-out CLI:
        // the answer comes from the cache, not from an empty picker.
        let models = catalogue_with(Some(path.clone()), "cursor-agent-not-installed");
        assert!(ids(&models).contains(&"cursor-grok-4.6"));

        // Stale plus undiscoverable still prefers the old list over nothing.
        let stale = CachedCatalogue {
            format: CACHE_FORMAT,
            fetched_at: now_secs() - CACHE_TTL.as_secs() - 60,
            models: parse(SAMPLE),
        };
        std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
        let models = catalogue_with(Some(path), "cursor-agent-not-installed");
        assert!(ids(&models).contains(&"cursor-grok-4.6"));
    }

    #[test]
    fn no_cache_and_no_cli_falls_back_to_the_builtin_list() {
        let dir = tempfile::tempdir().unwrap();
        let models = catalogue_with(
            Some(dir.path().join("absent.json")),
            "cursor-agent-not-installed",
        );
        assert_eq!(ids(&models), BUILTIN_MODELS.to_vec());
    }

    #[test]
    fn headings_and_junk_lines_are_not_models() {
        let models = parse(SAMPLE);
        assert!(!ids(&models).contains(&"Available models"));
        assert!(parse("Available models\n\nnot a model line\n").is_empty());
    }
}
