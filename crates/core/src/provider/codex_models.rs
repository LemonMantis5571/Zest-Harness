//! The Codex model catalogue: discovered from `codex app-server`, cached on disk.
//!
//! A ChatGPT-backed Codex session only accepts models in *that account and CLI
//! version's* accessible catalog (`model/list`). Offering a hand-written list
//! invites `400 model_not_in_accessible_catalog` whenever the installed CLI
//! predates a newer family or a plan withholds a tier. Discovering from the
//! local app-server once a day — and invalidating immediately when the CLI
//! binary changes on disk — keeps the picker honest without paying a subprocess
//! spawn on every frame.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    catalogue as build_catalogue, context_window_for_model, default_efforts_for_model,
    model_supports_vision, EffortPolicy, ModelSpec, CODEX_KNOWN_MODELS,
};
use crate::config::DEFAULT_CODEX_MODEL;

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_FORMAT: u32 = 1;
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredCodex {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelSpec>,
    /// Retired model id -> recommended replacement from `model/list` (`upgrade`).
    #[serde(default)]
    pub upgrades: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CachedCatalogue {
    #[serde(default)]
    format: u32,
    #[serde(default)]
    fetched_at: u64,
    #[serde(default)]
    binary_stamp: Option<u64>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    models: Vec<ModelSpec>,
    #[serde(default)]
    upgrades: BTreeMap<String, String>,
}

/// `<data dir>/zest/codex-models.json`, beside `cursor-models.json`.
pub fn cache_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("zest").join("codex-models.json"))
}

/// The accessible catalogue for the installed Codex CLI.
///
/// Unit tests stay hermetic and fast by using [`fallback`] directly; [`catalogue_with`]
/// exercises the cache, binary-stamp invalidation, and fallback rules against
/// isolated temp paths.
pub fn catalogue(command: &str) -> DiscoveredCodex {
    #[cfg(test)]
    {
        let _ = command;
        fallback()
    }
    #[cfg(not(test))]
    {
        let stamp = binary_stamp(command);
        catalogue_with(cache_path(), stamp, || discover(command))
    }
}

/// Persist a freshly discovered catalogue from an already-running app-server.
pub fn save_cache(command: &str, discovered: &DiscoveredCodex) {
    #[cfg(test)]
    {
        let _ = (command, discovered);
    }
    #[cfg(not(test))]
    {
        if !discovered.models.is_empty() {
            if let Some(path) = cache_path() {
                write_cache(&path, binary_stamp(command), discovered);
            }
        }
    }
}

fn catalogue_with<F>(
    path: Option<PathBuf>,
    current_stamp: Option<u64>,
    discover_fn: F,
) -> DiscoveredCodex
where
    F: FnOnce() -> Option<DiscoveredCodex>,
{
    let cached = path.as_deref().and_then(read_cache);
    if let Some(cached) = cached.as_ref() {
        let same_binary = cached.binary_stamp == current_stamp;
        if same_binary && !is_stale(cached.fetched_at) && !cached.models.is_empty() {
            return DiscoveredCodex {
                default_model: cached.default_model.clone(),
                models: cached.models.clone(),
                upgrades: cached.upgrades.clone(),
            };
        }
    }

    match discover_fn() {
        Some(discovered) if !discovered.models.is_empty() => {
            if let Some(path) = path {
                write_cache(&path, current_stamp, &discovered);
            }
            discovered
        }
        _ => match cached.filter(|c| !c.models.is_empty() && c.binary_stamp == current_stamp) {
            Some(cached) => DiscoveredCodex {
                default_model: cached.default_model,
                models: cached.models,
                upgrades: cached.upgrades,
            },
            None => fallback(),
        },
    }
}

/// Built-in shortlist used before the CLI has answered once (or when missing).
pub fn fallback() -> DiscoveredCodex {
    let mut upgrades = BTreeMap::new();
    upgrades.insert("gpt-5.6-sol".to_string(), "gpt-6.2-sol".to_string());
    upgrades.insert("gpt-5.6-terra".to_string(), "gpt-6.1-terra".to_string());
    DiscoveredCodex {
        default_model: Some(DEFAULT_CODEX_MODEL.to_string()),
        models: build_catalogue(
            DEFAULT_CODEX_MODEL,
            &[],
            CODEX_KNOWN_MODELS,
            EffortPolicy::Standard(&[]),
        ),
        upgrades,
    }
}

#[cfg(not(test))]
fn binary_stamp(command: &str) -> Option<u64> {
    let resolved = crate::tools::external_agent::resolve_program(command);
    std::fs::metadata(resolved)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
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

fn write_cache(path: &Path, binary_stamp: Option<u64>, discovered: &DiscoveredCodex) {
    let cached = CachedCatalogue {
        format: CACHE_FORMAT,
        fetched_at: now_secs(),
        binary_stamp,
        default_model: discovered.default_model.clone(),
        models: discovered.models.clone(),
        upgrades: discovered.upgrades.clone(),
    };
    if let Ok(raw) = serde_json::to_vec_pretty(&cached) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = crate::fsutil::atomic_write(path, &raw);
    }
}

/// Spawn `codex app-server`, run `initialize` + `model/list`, and parse the
/// accessible catalog under a bounded timeout.
#[cfg_attr(test, allow(dead_code))]
fn discover(command: &str) -> Option<DiscoveredCodex> {
    let mut process = Command::new(crate::tools::external_agent::resolve_program(command));
    process
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::tools::external_agent::prepare_sync_external_command(&mut process);

    let mut child = process.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Option<DiscoveredCodex> {
            let mut reader = BufReader::new(stdout);
            writeln!(
                stdin,
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": {
                            "name": "zest",
                            "title": "Zest",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "capabilities": { "experimentalApi": false }
                    }
                })
            )
            .ok()?;
            stdin.flush().ok()?;
            let init = read_response_for(&mut reader, 1)?;
            if init.get("error").is_some() {
                return None;
            }

            writeln!(
                stdin,
                "{}",
                json!({ "jsonrpc": "2.0", "method": "initialized" })
            )
            .ok()?;
            writeln!(
                stdin,
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "model/list",
                    "params": { "includeHidden": false }
                })
            )
            .ok()?;
            stdin.flush().ok()?;
            let response = read_response_for(&mut reader, 2)?;
            if response.get("error").is_some() {
                return None;
            }
            let parsed = parse_model_list(&response);
            (!parsed.models.is_empty()).then_some(parsed)
        })();
        let _ = tx.send(result);
    });

    let discovered = rx.recv_timeout(DISCOVER_TIMEOUT).ok().flatten();
    let _ = child.kill();
    let _ = child.wait();
    discovered
}

#[cfg_attr(test, allow(dead_code))]
fn read_response_for(reader: &mut impl BufRead, expected_id: u64) -> Option<Value> {
    let mut line = String::new();
    // Skip unsolicited startup notifications (e.g. `remoteControl/status/changed`)
    // until the matching RPC reply arrives, bounded so noisy output cannot spin.
    for _ in 0..64 {
        line.clear();
        let bytes = reader.read_line(&mut line).ok()?;
        if bytes == 0 {
            return None;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return Some(value);
        }
    }
    None
}

/// Parse a `model/list` RPC reply (or its inner `result` object) into specs.
pub fn parse_model_list(value: &Value) -> DiscoveredCodex {
    let root = value.get("result").unwrap_or(value);
    let items = root
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| root.get("models").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();

    let mut default_model: Option<String> = None;
    let mut upgrades = BTreeMap::new();
    let mut models = Vec::new();

    for item in items {
        if item.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(id) = item
            .get("id")
            .or_else(|| item.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
        else {
            continue;
        };

        if default_model.is_none() && item.get("isDefault").and_then(Value::as_bool) == Some(true) {
            default_model = Some(id.clone());
        }
        if let Some(upgrade) = item
            .get("upgrade")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|target| !target.is_empty())
        {
            upgrades.insert(id.clone(), upgrade.to_string());
        }

        let efforts = match item
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
        {
            Some(entries) => {
                let mut parsed = Vec::with_capacity(entries.len());
                for entry in entries {
                    let raw = entry.as_str().or_else(|| {
                        entry
                            .get("reasoningEffort")
                            .or_else(|| entry.get("effort"))
                            .and_then(Value::as_str)
                    });
                    if let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) {
                        let normalized = match raw {
                            "extra" | "extra_high" => "xhigh".to_string(),
                            other => other.to_string(),
                        };
                        if !parsed.contains(&normalized) {
                            parsed.push(normalized);
                        }
                    }
                }
                parsed
            }
            None if matches!(id.as_str(), "gpt-6-sol" | "gpt-6-luna") => {
                default_efforts_for_model(&id)
            }
            None => Vec::new(),
        };

        let supports_vision = item
            .get("supportsVision")
            .and_then(Value::as_bool)
            .or_else(|| {
                item.get("inputModalities")
                    .and_then(Value::as_array)
                    .map(|modalities| {
                        modalities
                            .iter()
                            .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("image")))
                    })
            })
            .unwrap_or_else(|| model_supports_vision(&id));

        let context_window = item
            .get("contextWindow")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| context_window_for_model(&id));

        models.push(ModelSpec {
            id,
            efforts,
            context_window,
            supports_tools: true,
            supports_vision,
            resolves_to: None,
            needs_cli: None,
        });
    }

    if default_model.is_none() {
        default_model = models.first().map(|model| model.id.clone());
    }

    DiscoveredCodex {
        default_model,
        models,
        upgrades,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_app_server_object_efforts_vision_default_and_upgrades() {
        let payload = json!({
            "id": 2,
            "result": {
                "data": [
                    {
                        "id": "gpt-6.2-sol",
                        "model": "gpt-6.2-sol",
                        "hidden": false,
                        "isDefault": true,
                        "inputModalities": ["text", "image"],
                        "supportedReasoningEfforts": [
                            {"reasoningEffort": "low", "description": "Fast"},
                            {"reasoningEffort": "medium", "description": "Balanced"},
                            {"reasoningEffort": "high", "description": "Deep"},
                            {"reasoningEffort": "xhigh", "description": "Extra"},
                            {"reasoningEffort": "max", "description": "Max"},
                            {"reasoningEffort": "ultra", "description": "Ultra"}
                        ]
                    },
                    {
                        "id": "gpt-5.6-sol",
                        "model": "gpt-5.6-sol",
                        "upgrade": "gpt-6.2-sol",
                        "hidden": false,
                        "isDefault": false,
                        "inputModalities": ["text", "image"],
                        "supportedReasoningEfforts": [
                            {"reasoningEffort": "low"},
                            {"reasoningEffort": "high"}
                        ]
                    },
                    {
                        "id": "gpt-reserve",
                        "hidden": true
                    }
                ]
            }
        });

        let parsed = parse_model_list(&payload);
        assert_eq!(parsed.default_model.as_deref(), Some("gpt-6.2-sol"));
        assert_eq!(parsed.models.len(), 2, "hidden models must be excluded");
        assert_eq!(
            parsed.models[0].efforts,
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert!(parsed.models[0].supports_vision);
        assert_eq!(
            parsed.upgrades.get("gpt-5.6-sol").map(String::as_str),
            Some("gpt-6.2-sol")
        );
    }

    #[test]
    fn cache_reuses_fresh_entry_and_invalidates_when_cli_binary_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codex-models.json");

        let v1 = DiscoveredCodex {
            default_model: Some("gpt-5.6-sol".into()),
            models: vec![ModelSpec {
                id: "gpt-5.6-sol".into(),
                efforts: vec!["low".into(), "high".into()],
                context_window: 1_050_000,
                supports_tools: true,
                supports_vision: true,
                resolves_to: None,
                needs_cli: None,
            }],
            upgrades: BTreeMap::new(),
        };

        let first = catalogue_with(Some(path.clone()), Some(100), || Some(v1.clone()));
        assert_eq!(first, v1);

        // Same binary stamp + fresh TTL -> cache hit without calling discover_fn.
        let cached = catalogue_with(Some(path.clone()), Some(100), || {
            panic!("fresh cache for same binary must not re-spawn app-server")
        });
        assert_eq!(cached, v1);

        // Upgrading `@openai/codex` changes the binary stamp -> immediate refresh.
        let v2 = DiscoveredCodex {
            default_model: Some("gpt-6.2-sol".into()),
            models: vec![ModelSpec {
                id: "gpt-6.2-sol".into(),
                efforts: vec!["low".into(), "high".into(), "ultra".into()],
                context_window: 1_050_000,
                supports_tools: true,
                supports_vision: true,
                resolves_to: None,
                needs_cli: None,
            }],
            upgrades: BTreeMap::from([("gpt-5.6-sol".into(), "gpt-6.2-sol".into())]),
        };
        let upgraded = catalogue_with(Some(path), Some(200), || Some(v2.clone()));
        assert_eq!(upgraded, v2);
    }
}
