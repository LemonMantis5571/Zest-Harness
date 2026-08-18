//! In-memory migration of `kind = "gateway"` provider entries.
//!
//! Zest used to bundle and supervise a CLIProxyAPI sidecar, and a subscription
//! was reached by pointing a `gateway` provider at it. The sidecar is gone and
//! both subscriptions are now driven through the vendor runtimes, so the
//! `gateway` kind no longer exists.
//!
//! Existing `zest.toml` files still say `gateway`. Rather than refuse to start,
//! [`migrate`] rewrites the document in memory so the strict parser accepts it.
//! **Nothing on disk changes** — this is deliberately not `config_edit.rs`. A
//! user who never opens Settings keeps a working config, and a user who does
//! gets the new shape written by the normal editing path.
//!
//! The strict parse runs first and this runs only when it fails, so a genuine
//! typo still reports serde's own error with its span. See [`crate::config::Config::parse`].

use toml_edit::{DocumentMut, Item, Table, Value};

/// What a migration did, for the caller to report.
#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    /// One human-readable notice per rewritten provider.
    pub migrations: Vec<String>,
    /// Providers that were dropped, as `(id, reason)`.
    pub unsupported: Vec<(String, String)>,
}

/// The two ids that map onto a vendor runtime, and how.
///
/// Model carrying is asymmetric on purpose. A `codex` gateway's model ids *are*
/// the strings the Codex CLI accepts, so they survive. A `claude` gateway's are
/// API model ids (`claude-opus-5`), which the Claude Code CLI does not take as
/// aliases — carrying them would produce a picker full of entries that fail.
enum Destination {
    CodexCli,
    ClaudeCode,
}

fn destination_for(id: &str) -> Option<Destination> {
    match id {
        "codex" => Some(Destination::CodexCli),
        "claude" => Some(Destination::ClaudeCode),
        _ => None,
    }
}

/// Rewrite every `gateway` provider in `raw`, or return `None` if there are none.
///
/// `None` means "this document has nothing I can fix", which the caller must
/// treat as "report the original parse error" rather than as success.
pub fn migrate(raw: &str) -> Option<(String, MigrationReport)> {
    let mut doc = raw.parse::<DocumentMut>().ok()?;
    let providers = doc.get_mut("providers")?.as_table_mut()?;

    let gateway_ids: Vec<String> = providers
        .iter()
        .filter(|(_, item)| is_gateway(item))
        .map(|(id, _)| id.to_string())
        .collect();
    if gateway_ids.is_empty() {
        return None;
    }

    let mut report = MigrationReport::default();
    // Providers whose model list did not survive: a `[default].model` naming one
    // of them would fail `validate_selection` at runtime, which is a hard build
    // error rather than a warning.
    let mut lost_models: Vec<String> = Vec::new();

    for id in gateway_ids {
        let Some(table) = providers.get_mut(&id).and_then(Item::as_table_mut) else {
            continue;
        };
        match destination_for(&id) {
            Some(Destination::CodexCli) => {
                to_codex_cli(table);
                report.migrations.push(format!(
                    "provider `{id}` used the removed `gateway` kind and now runs the Codex CLI \
                     directly. Codex owns its own tools, so Zest's file, shell, browser, and \
                     delegation tools are not used on this provider. zest.toml was not changed."
                ));
            }
            Some(Destination::ClaudeCode) => {
                to_claude_code(table);
                lost_models.push(id.clone());
                report.migrations.push(format!(
                    "provider `{id}` used the removed `gateway` kind and now runs the Claude Code \
                     CLI directly. Its model list was dropped because gateway model ids are not \
                     CLI aliases, and Claude Code owns its own tools, so Zest's file, shell, \
                     browser, and delegation tools are not used on this provider. zest.toml was \
                     not changed."
                ));
            }
            None => {
                providers.remove(&id);
                lost_models.push(id.clone());
                report.unsupported.push((
                    id.clone(),
                    "the `gateway` kind was removed with the bundled proxy. Reach this account \
                     through its own CLI (`claude_code` or `codex_cli`) or an `anthropic` key."
                        .to_string(),
                ));
            }
        }
    }

    strip_lost_default_models(&mut doc, &lost_models);
    Some((doc.to_string(), report))
}

fn is_gateway(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("kind"))
        .and_then(Item::as_str)
        == Some("gateway")
}

/// `codex` keeps its model and its effort allow-list; the transport fields go.
fn to_codex_cli(table: &mut Table) {
    table["kind"] = value_str("codex_cli");
    table["command"] = value_str("codex");
    table.remove("base_url");
    table.remove("api_key_env");
}

/// `claude` keeps only its identity — everything else described the proxy.
fn to_claude_code(table: &mut Table) {
    table["kind"] = value_str("claude_code");
    table["command"] = value_str("claude");
    table.remove("base_url");
    table.remove("api_key_env");
    table.remove("model");
    table.remove("models");
    table.remove("efforts");
}

fn value_str(text: &str) -> Item {
    Item::Value(Value::from(text))
}

/// Drop a `model` pin that named a provider which just lost its catalogue.
///
/// `RuntimeBuilder::build` runs `validate_selection` unconditionally, so leaving
/// the pin in place turns a migrated config into a startup failure. Only the
/// model is removed — the provider choice itself is still what the user meant.
fn strip_lost_default_models(doc: &mut DocumentMut, lost: &[String]) {
    if lost.is_empty() {
        return;
    }
    for path in [&["default"][..], &["routing", "default"][..]] {
        let Some(target) = descend_mut(doc, path) else {
            continue;
        };
        let names_lost = target
            .get("provider")
            .and_then(Item::as_str)
            .is_some_and(|provider| lost.iter().any(|id| id == provider));
        if names_lost {
            target.remove("model");
        }
    }
}

fn descend_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for key in path {
        table = table.get_mut(key)?.as_table_mut()?;
    }
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated(raw: &str) -> (String, MigrationReport) {
        migrate(raw).expect("a document with a gateway provider migrates")
    }

    #[test]
    fn a_legacy_gateway_codex_entry_migrates_to_the_codex_cli() {
        let (out, report) = migrated(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
api_key_env = "ZEST_GATEWAY_KEY"
model = "gpt-5.6-sol"
models = ["gpt-5.6-sol", "gpt-5.6"]
efforts = ["medium", "high"]
"#,
        );

        assert!(out.contains(r#"kind = "codex_cli""#));
        assert!(out.contains(r#"command = "codex""#));
        assert!(
            out.contains(r#"model = "gpt-5.6-sol""#) && out.contains(r#""gpt-5.6""#),
            "Codex gateway model ids are the strings the CLI accepts, so they survive: {out}"
        );
        assert!(out.contains(r#"efforts = ["medium", "high"]"#));
        assert!(!out.contains("base_url"), "the proxy origin is gone: {out}");
        assert!(
            !out.contains("api_key_env"),
            "the proxy token is gone: {out}"
        );
        assert_eq!(report.unsupported.len(), 0);
        assert_eq!(report.migrations.len(), 1);
        assert!(
            report.migrations[0].contains("zest.toml was not changed"),
            "the notice says the file was left alone: {}",
            report.migrations[0]
        );
    }

    #[test]
    fn a_legacy_gateway_claude_entry_resets_its_model_list() {
        let (out, _) = migrated(
            r#"
[providers.claude]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "claude-opus-5"
models = ["claude-opus-5", "claude-sonnet-5"]
"#,
        );

        assert!(out.contains(r#"kind = "claude_code""#));
        assert!(out.contains(r#"command = "claude""#));
        assert!(
            !out.contains("claude-opus-5"),
            "API model ids are not CLI aliases, so the list is dropped: {out}"
        );
        assert!(
            !out.contains("models"),
            "no empty allow-list is left: {out}"
        );
    }

    #[test]
    fn an_unrecognised_gateway_entry_is_skipped_with_a_reason_naming_the_replacement() {
        let (out, report) = migrated(
            r#"
[providers.gemini]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gemini-3.1-pro"
"#,
        );

        assert!(!out.contains("gemini"), "the entry is removed: {out}");
        assert_eq!(report.unsupported.len(), 1);
        assert_eq!(report.unsupported[0].0, "gemini");
        assert!(
            report.unsupported[0].1.contains("claude_code")
                && report.unsupported[0].1.contains("codex_cli"),
            "the reason names what to use instead: {}",
            report.unsupported[0].1
        );
    }

    #[test]
    fn a_default_model_that_the_migrated_provider_lost_is_stripped() {
        let (out, _) = migrated(
            r#"
[providers.claude]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "claude-opus-5"

[default]
provider = "claude"
model = "claude-opus-5"
effort = "high"
"#,
        );

        assert!(
            !out.contains("claude-opus-5"),
            "a pin the provider can no longer offer is a hard startup error: {out}"
        );
        assert!(
            out.contains(r#"provider = "claude""#),
            "the provider choice is still what the user meant: {out}"
        );
        assert!(out.contains(r#"effort = "high""#), "effort survives: {out}");
    }

    #[test]
    fn a_legacy_routing_default_model_is_stripped_too() {
        let (out, _) = migrated(
            r#"
[providers.claude]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "claude-opus-5"

[routing.default]
provider = "claude"
model = "claude-opus-5"
"#,
        );

        assert!(!out.contains("claude-opus-5"), "{out}");
    }

    #[test]
    fn a_codex_default_model_survives_because_codex_keeps_its_catalogue() {
        let (out, _) = migrated(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-sol"

[default]
provider = "codex"
model = "gpt-5.6-sol"
"#,
        );

        assert!(
            out.matches("gpt-5.6-sol").count() >= 2,
            "the pin is still selectable, so it stays: {out}"
        );
    }

    #[test]
    fn a_document_with_no_gateway_provider_is_left_for_the_strict_parser() {
        assert!(
            migrate(
                r#"
[providers.anthropic]
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
"#
            )
            .is_none(),
            "nothing to migrate must not look like a successful migration"
        );
    }

    #[test]
    fn a_document_that_is_not_even_toml_is_left_for_the_strict_parser() {
        assert!(migrate("[providers.codex\nkind = ").is_none());
    }

    #[test]
    fn comments_and_unrelated_sections_survive_the_rewrite() {
        let (out, _) = migrated(
            r#"
# keep me
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-sol"

[tools]
bash_timeout_ms = 1234
"#,
        );

        assert!(out.contains("# keep me"), "{out}");
        assert!(out.contains("bash_timeout_ms = 1234"), "{out}");
    }
}
