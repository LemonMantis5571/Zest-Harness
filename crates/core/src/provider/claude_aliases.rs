//! What the Claude Code CLI's bare model aliases resolve to.
//!
//! `opus`, `sonnet`, `haiku` and `fable` mean whatever the installed CLI says
//! they mean, and that is not the newest model: CLI 2.1.220 runs `opus` as
//! `claude-opus-5` and `haiku` as Haiku 4.5. Zest no longer offers them in its
//! own catalogue, but a model a user configured is still honoured, so an alias
//! can still be the one in use. The picker labels it with the model it ran,
//! which is only known once a turn has run: the CLI names it in `system:init`.
//! It is kept here, and on disk so the label survives a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::ModelSpec;
use crate::fsutil::atomic_write_json;

/// The aliases the Claude Code CLI accepts, each with the explicit model of the
/// same family Zest offers in its place.
const ALIASES: &[(&str, &str)] = &[
    ("sonnet", "claude-sonnet-5-5"),
    ("opus", "claude-opus-5-5"),
    ("haiku", "claude-haiku-5-5"),
    ("fable", "claude-fable-5-1"),
];

pub fn is_alias(model: &str) -> bool {
    ALIASES.iter().any(|(alias, _)| *alias == model)
}

/// The explicit model Zest offers in place of an alias it no longer lists.
pub fn successor(model: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == model)
        .map(|(_, explicit)| *explicit)
}

/// The model an alias last ran as on this machine, if a turn has used it.
pub fn resolved(alias: &str) -> Option<String> {
    store().lock().ok()?.get(alias)
}

/// Remember what an alias ran as. Ids that are not aliases are ignored.
pub fn record(alias: &str, model: &str) {
    if let Ok(mut store) = store().lock() {
        store.record(alias, model);
    }
}

/// Fill in [`ModelSpec::resolves_to`] for every alias in a catalogue.
pub fn label(mut models: Vec<ModelSpec>) -> Vec<ModelSpec> {
    for model in &mut models {
        if is_alias(&model.id) {
            model.resolves_to = resolved(&model.id);
        }
    }
    models
}

fn store() -> &'static Mutex<AliasStore> {
    static STORE: OnceLock<Mutex<AliasStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(match AliasStore::default_path() {
            Some(path) => AliasStore::load_from(path),
            None => AliasStore::default(),
        })
    })
}

/// Alias → resolved model, saved beside the other per-machine caches.
#[derive(Debug, Default)]
struct AliasStore {
    path: Option<PathBuf>,
    resolved: BTreeMap<String, String>,
}

impl AliasStore {
    /// `<data dir>/zest/claude-code-aliases.json`.
    fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("zest").join("claude-code-aliases.json"))
    }

    /// An unreadable or absent file is an empty store: the label waits for the
    /// next turn instead of failing anything.
    fn load_from(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let resolved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BTreeMap<String, String>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|(alias, model)| is_alias(alias) && !model.trim().is_empty())
            .collect();
        Self {
            path: Some(path),
            resolved,
        }
    }

    fn get(&self, alias: &str) -> Option<String> {
        self.resolved.get(alias).cloned()
    }

    fn record(&mut self, alias: &str, model: &str) {
        let model = model.trim();
        if !is_alias(alias) || model.is_empty() || model == alias {
            return;
        }
        if self.resolved.get(alias).map(String::as_str) == Some(model) {
            return;
        }
        self.resolved.insert(alias.to_string(), model.to_string());
        if let Some(path) = &self.path {
            save(path, &self.resolved);
        }
    }
}

/// Best effort: a label that is lost costs one more turn to relearn.
fn save(path: &Path, resolved: &BTreeMap<String, String>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = atomic_write_json(path, resolved);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_alias_has_an_explicit_successor_of_the_same_family() {
        for (alias, explicit) in ALIASES {
            assert!(is_alias(alias));
            assert_eq!(successor(alias), Some(*explicit));
            assert!(explicit.contains(alias), "{alias} -> {explicit}");
        }
        assert!(!is_alias("claude-opus-5-5"));
        assert_eq!(successor("claude-opus-5-5"), None);
    }

    #[test]
    fn a_recorded_resolution_is_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("aliases.json");

        let mut store = AliasStore::load_from(&path);
        assert_eq!(store.get("opus"), None);
        store.record("opus", "claude-opus-5");
        store.record("haiku", "claude-haiku-4-5-20251001");

        let reloaded = AliasStore::load_from(&path);
        assert_eq!(reloaded.get("opus").as_deref(), Some("claude-opus-5"));
        assert_eq!(
            reloaded.get("haiku").as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn only_aliases_with_a_real_model_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aliases.json");
        let mut store = AliasStore::load_from(&path);

        store.record("claude-opus-5-5", "claude-opus-5-5");
        store.record("opus", "  ");
        store.record("opus", "opus");
        assert!(store.resolved.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_corrupt_or_foreign_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aliases.json");

        std::fs::write(&path, "not json").unwrap();
        assert!(AliasStore::load_from(&path).resolved.is_empty());

        std::fs::write(&path, r#"{"gpt-5":"x","opus":"claude-opus-5"}"#).unwrap();
        let store = AliasStore::load_from(&path);
        assert_eq!(store.get("gpt-5"), None);
        assert_eq!(store.get("opus").as_deref(), Some("claude-opus-5"));
    }
}
