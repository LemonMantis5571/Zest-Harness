//! Setting up chat providers from a settings screen: which providers exist (configured ones
//! plus the standard choices not configured yet), whether each is signed in or has its key,
//! adding one to `zest.toml`, storing an API key in the OS credential manager, and choosing
//! the provider new chats use.
//!
//! API keys never go into `zest.toml`: an entry names a credential-manager account
//! (`credential`), or an environment variable (`api_key_env`).

use std::path::Path;

use crate::auth::{self, AuthStatus};
use crate::config::{Config, ProviderConfig};
use crate::config_edit::{self, AnthropicProviderInput, OpenAiProviderInput};
use crate::mcp_admin::editable_config_path;

/// The kinds of provider a settings screen can add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChoice {
    /// The `claude` CLI, signed in to a Claude account.
    ClaudeCode,
    /// The `codex` CLI.
    CodexCli,
    /// ChatGPT sign-in, with Zest's own agent loop.
    ChatGpt,
    /// The `cursor-agent` CLI.
    Cursor,
    /// An Anthropic API key.
    Anthropic,
    /// Any OpenAI-compatible endpoint with an API key.
    OpenAiCompatible,
}

impl ProviderChoice {
    pub const ALL: [ProviderChoice; 6] = [
        ProviderChoice::ClaudeCode,
        ProviderChoice::CodexCli,
        ProviderChoice::ChatGpt,
        ProviderChoice::Cursor,
        ProviderChoice::Anthropic,
        ProviderChoice::OpenAiCompatible,
    ];

    /// The id a new entry of this kind gets in `[providers.<id>]`.
    pub fn default_id(self) -> &'static str {
        match self {
            ProviderChoice::ClaudeCode => "claude",
            ProviderChoice::CodexCli => "codex",
            ProviderChoice::ChatGpt => "codex-chatgpt",
            ProviderChoice::Cursor => "cursor",
            ProviderChoice::Anthropic => "anthropic",
            ProviderChoice::OpenAiCompatible => "openai",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderChoice::ClaudeCode => "Claude Code",
            ProviderChoice::CodexCli => "Codex CLI",
            ProviderChoice::ChatGpt => "ChatGPT",
            ProviderChoice::Cursor => "Cursor",
            ProviderChoice::Anthropic => "Anthropic API",
            ProviderChoice::OpenAiCompatible => "OpenAI-compatible API",
        }
    }

    pub fn method(self) -> &'static str {
        match self {
            ProviderChoice::ClaudeCode => "Claude subscription through the claude CLI",
            ProviderChoice::CodexCli => "OpenAI account through the codex CLI",
            ProviderChoice::ChatGpt => "ChatGPT sign-in",
            ProviderChoice::Cursor => "Cursor subscription through cursor-agent",
            ProviderChoice::Anthropic => "API key",
            ProviderChoice::OpenAiCompatible => "API key and endpoint URL",
        }
    }

    /// Whether adding it needs an API key.
    pub fn needs_key(self) -> bool {
        matches!(
            self,
            ProviderChoice::Anthropic | ProviderChoice::OpenAiCompatible
        )
    }

    fn of(config: &ProviderConfig) -> ProviderChoice {
        match config {
            ProviderConfig::ClaudeCode { .. } => ProviderChoice::ClaudeCode,
            ProviderConfig::CodexCli { .. } => ProviderChoice::CodexCli,
            ProviderConfig::CodexOAuth { .. } => ProviderChoice::ChatGpt,
            ProviderConfig::CursorAcp { .. } => ProviderChoice::Cursor,
            ProviderConfig::Anthropic { .. } => ProviderChoice::Anthropic,
            ProviderConfig::OpenaiCompatible { .. } => ProviderChoice::OpenAiCompatible,
        }
    }
}

/// Whether a provider can serve a turn now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderState {
    /// Signed in, or its key is stored or set.
    Ready,
    /// Configured, but not signed in or without a key.
    NeedsSignIn,
    /// Zest cannot tell (credentials kept somewhere it does not inspect). Turns may work.
    Unknown,
    /// Not in `zest.toml` yet.
    NotAdded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRow {
    /// `[providers.<id>]`, or the id it would get when added.
    pub id: String,
    pub label: String,
    pub method: String,
    pub choice: Option<ProviderChoice>,
    pub state: ProviderState,
    /// A short line: the signed-in account, what is missing, or the endpoint.
    pub detail: String,
    /// New chats use this provider.
    pub is_default: bool,
    /// An API-key provider: a key can be stored for it.
    pub takes_key: bool,
}

/// Configured providers first (default one on top), then the standard choices that are not
/// configured yet.
pub fn rows(config: &Config) -> Vec<ProviderRow> {
    let default = default_provider(config);
    let mut rows: Vec<ProviderRow> = config
        .providers
        .iter()
        .map(|(id, provider)| configured_row(id, provider, default.as_deref() == Some(id.as_str())))
        .collect();
    rows.sort_by_key(|row| !row.is_default);
    for choice in ProviderChoice::ALL {
        let taken = config
            .providers
            .values()
            .any(|p| ProviderChoice::of(p) == choice);
        if taken {
            continue;
        }
        rows.push(ProviderRow {
            id: free_id(config, choice),
            label: choice.label().into(),
            method: choice.method().into(),
            choice: Some(choice),
            state: ProviderState::NotAdded,
            detail: offer_detail(choice),
            is_default: false,
            takes_key: choice.needs_key(),
        });
    }
    rows
}

/// The provider new chats use: the config's default, else the first configured one.
pub fn default_provider(config: &Config) -> Option<String> {
    config
        .default
        .as_ref()
        .map(|target| target.provider.clone())
        .filter(|id| config.providers.contains_key(id))
        .or_else(|| config.providers.keys().next().cloned())
}

fn free_id(config: &Config, choice: ProviderChoice) -> String {
    let base = choice.default_id();
    if !config.providers.contains_key(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !config.providers.contains_key(id))
        .unwrap()
}

fn offer_detail(choice: ProviderChoice) -> String {
    let signed_in = match choice {
        ProviderChoice::ClaudeCode => auth::detect_claude_code(),
        ProviderChoice::CodexCli => auth::detect_codex_cli(),
        ProviderChoice::ChatGpt => auth::detect_codex_oauth(choice.default_id()),
        ProviderChoice::Cursor => auth::detect_cursor_cli(),
        ProviderChoice::Anthropic | ProviderChoice::OpenAiCompatible => AuthStatus::Unconfigured,
    };
    match signed_in {
        AuthStatus::Ready { account } => match account {
            Some(account) => format!("Already signed in as {account}. Add it to use it."),
            None => "Already signed in. Add it to use it.".into(),
        },
        _ if choice.needs_key() => "Add it with your API key.".into(),
        _ => "Add it, then sign in.".into(),
    }
}

fn configured_row(id: &str, provider: &ProviderConfig, is_default: bool) -> ProviderRow {
    let choice = ProviderChoice::of(provider);
    let (state, detail) = match provider {
        ProviderConfig::ClaudeCode { .. } => from_auth(auth::detect_claude_code()),
        ProviderConfig::CodexCli { .. } => from_auth(auth::detect_codex_cli()),
        ProviderConfig::CursorAcp { .. } => from_auth(auth::detect_cursor_cli()),
        ProviderConfig::CodexOAuth { credential, .. } => from_auth(auth::detect_codex_oauth(
            credential
                .as_deref()
                .filter(|c| !c.is_empty())
                .unwrap_or(id),
        )),
        ProviderConfig::Anthropic { .. } | ProviderConfig::OpenaiCompatible { .. } => {
            key_state(provider)
        }
    };
    ProviderRow {
        id: id.to_string(),
        label: choice.label().to_string(),
        method: choice.method().to_string(),
        choice: Some(choice),
        state,
        detail,
        is_default,
        takes_key: choice.needs_key(),
    }
}

fn from_auth(status: AuthStatus) -> (ProviderState, String) {
    match status {
        AuthStatus::Ready { account } => (
            ProviderState::Ready,
            account
                .map(|a| format!("Signed in as {a}"))
                .unwrap_or_else(|| "Signed in".into()),
        ),
        AuthStatus::NotLoggedIn { .. } => (ProviderState::NeedsSignIn, "Not signed in".into()),
        AuthStatus::Unknown { .. } => (
            ProviderState::Unknown,
            "Zest cannot check this sign-in; turns may still work.".into(),
        ),
        AuthStatus::Unconfigured => (ProviderState::NeedsSignIn, "Not set up".into()),
    }
}

/// The credential-manager account and environment variable an API provider reads its key from.
fn key_sources(provider: &ProviderConfig) -> (Option<&str>, Option<&str>) {
    let credential = match provider {
        ProviderConfig::Anthropic { credential, .. }
        | ProviderConfig::OpenaiCompatible { credential, .. } => {
            credential.as_deref().filter(|c| !c.is_empty())
        }
        _ => None,
    };
    (credential, provider.key_env().filter(|v| !v.is_empty()))
}

fn key_state(provider: &ProviderConfig) -> (ProviderState, String) {
    let (credential, env) = key_sources(provider);
    if credential.is_some_and(|account| crate::credentials::present(account).unwrap_or(false)) {
        return (ProviderState::Ready, "API key saved".into());
    }
    if let Some(var) = env.filter(|var| std::env::var_os(var).is_some()) {
        return (ProviderState::Ready, format!("API key from {var}"));
    }
    let detail = match (credential, env) {
        (Some(_), _) => "No API key saved".to_string(),
        (None, Some(var)) => format!("Set {var}, or save a key here"),
        (None, None) => "No API key".to_string(),
    };
    (ProviderState::NeedsSignIn, detail)
}

/// What a settings screen submits to add a provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderForm {
    /// Blank for the choice's default id.
    pub id: String,
    /// Blank for the provider's default model. Required for an OpenAI-compatible endpoint.
    pub model: String,
    /// OpenAI-compatible endpoint URL.
    pub base_url: String,
    /// API key for the API kinds.
    pub key: String,
}

/// Adds a provider to the editable `zest.toml` (the project's when it has one) and, for the API
/// kinds, stores the key under the entry's credential account. Makes it the default when no
/// default is set. Returns the new id.
pub fn add(
    config_root: &Path,
    choice: ProviderChoice,
    form: ProviderForm,
) -> Result<String, String> {
    let config = Config::find(config_root).map_err(|e| e.to_string())?;
    let id = match form.id.trim() {
        "" => free_id(&config, choice),
        id => id.to_string(),
    };
    if config.providers.contains_key(&id) {
        return Err(format!("A provider named {id} already exists."));
    }
    let key = form.key.trim();
    if choice.needs_key() && key.is_empty() {
        return Err("Paste the API key first.".into());
    }
    let model = form.model.trim();
    let default_model =
        || crate::provider::descriptor_for_picker_id(choice.default_id()).default_model;
    let model = if model.is_empty() {
        default_model()
    } else {
        model.to_string()
    };
    let path = editable_config_path(config_root)?;
    match choice {
        ProviderChoice::ClaudeCode => config_edit::add_claude_code_provider(
            &path,
            &config_edit::ClaudeCodeProviderInput {
                id: id.clone(),
                command: "claude".into(),
                model,
                // No allow-list: the built-in catalogue applies and keeps up with new models.
                models: Vec::new(),
                allow_mcp: false,
                // `auto` approves routine work inside the CLI and refers anything it is unsure
                // about to Zest's approval card; `accept_edits` would skip the card for edits.
                permission_mode: crate::ClaudeCodePermissionMode::Auto,
                timeout_secs: 900,
            },
        )?,
        ProviderChoice::CodexCli => config_edit::add_codex_cli_provider(
            &path,
            &config_edit::CodexCliProviderInput {
                id: id.clone(),
                command: "codex".into(),
                model,
                models: Vec::new(),
                efforts: Vec::new(),
                allow_mcp: false,
                timeout_secs: 900,
            },
        )?,
        ProviderChoice::ChatGpt => config_edit::add_codex_oauth_provider(
            &path,
            &config_edit::CodexOAuthProviderInput {
                id: id.clone(),
                model,
                credential: id.clone(),
            },
        )?,
        ProviderChoice::Cursor => config_edit::add_cursor_provider(
            &path,
            &config_edit::CursorProviderInput {
                id: id.clone(),
                command: "cursor-agent".into(),
                model,
                // Empty on purpose: a written list is an allow-list that hides the account's
                // other models.
                models: Vec::new(),
                allow_mcp: false,
                // Cursor never asks before editing a file; set `mode = "plan"` in zest.toml for
                // a read-only Cursor.
                mode: crate::CursorMode::Agent,
                timeout_secs: 900,
            },
        )?,
        ProviderChoice::Anthropic => {
            config_edit::add_anthropic_provider(
                &path,
                &AnthropicProviderInput {
                    id: id.clone(),
                    model,
                    credential: id.clone(),
                },
            )?;
            crate::credentials::set(&id, key)?;
        }
        ProviderChoice::OpenAiCompatible => {
            if form.model.trim().is_empty() {
                return Err("Name the model the endpoint serves.".into());
            }
            let models = if form.base_url.contains("api.deepseek.com") {
                vec![
                    "deepseek-flash".into(),
                    "deepseek-v4.1-flash".into(),
                    "deepseek-v4-pro".into(),
                ]
            } else {
                Vec::new()
            };
            config_edit::add_openai_provider(
                &path,
                &OpenAiProviderInput {
                    id: id.clone(),
                    base_url: form.base_url,
                    model,
                    models,
                    credential: id.clone(),
                },
            )?;
            crate::credentials::set(&id, key)?;
        }
    }
    if config.default.is_none() {
        config_edit::set_default_provider(&path, &id)?;
    }
    Ok(id)
}

/// Stores (or replaces) the API key of a configured API provider.
pub fn save_key(config: &Config, id: &str, key: &str) -> Result<(), String> {
    let provider = config
        .providers
        .get(id)
        .ok_or_else(|| format!("{id} is not configured."))?;
    if !matches!(
        provider,
        ProviderConfig::Anthropic { .. } | ProviderConfig::OpenaiCompatible { .. }
    ) {
        return Err("This provider does not take an API key.".into());
    }
    let (credential, env) = key_sources(provider);
    let Some(account) = credential else {
        return Err(match env {
            Some(var) => format!("{id} reads its key from {var}. Set that variable instead."),
            None => format!("{id} has no credential account in zest.toml."),
        });
    };
    let key = key.trim();
    if key.is_empty() {
        return Err("Paste the API key first.".into());
    }
    crate::credentials::set(account, key)
}

/// Makes `id` the provider new chats use.
pub fn make_default(config_root: &Path, id: &str) -> Result<(), String> {
    let config = Config::find(config_root).map_err(|e| e.to_string())?;
    if !config.providers.contains_key(id) {
        return Err(format!("{id} is not configured."));
    }
    config_edit::set_default_provider(&editable_config_path(config_root)?, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(toml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::config::CONFIG_FILE), toml).unwrap();
        dir
    }

    fn config(dir: &tempfile::TempDir) -> Config {
        Config::find(dir.path()).unwrap()
    }

    #[test]
    fn an_empty_project_offers_every_standard_provider() {
        let dir = project("");
        let rows = rows(&config(&dir));
        let choices: Vec<_> = rows.iter().map(|r| r.choice).collect();
        assert_eq!(choices, ProviderChoice::ALL.map(Some));
        assert!(rows
            .iter()
            .all(|r| r.state == ProviderState::NotAdded && !r.is_default));
        assert!(rows.iter().filter(|r| r.takes_key).count() == 2);
    }

    #[test]
    fn configured_providers_come_first_with_the_default_on_top() {
        let dir = project(
            "[default]\nprovider = \"work\"\n\n[providers.claude]\nkind = \"claude_code\"\n\n[providers.work]\nkind = \"codex_cli\"\n",
        );
        let rows = rows(&config(&dir));
        assert_eq!(rows[0].id, "work");
        assert!(rows[0].is_default);
        assert_eq!(rows[1].id, "claude");
        assert_eq!(rows[1].choice, Some(ProviderChoice::ClaudeCode));
        // Claude Code and Codex CLI are configured, so only the other four are offered.
        let offered: Vec<_> = rows
            .iter()
            .filter(|r| r.state == ProviderState::NotAdded)
            .map(|r| r.choice.unwrap())
            .collect();
        assert_eq!(
            offered,
            [
                ProviderChoice::ChatGpt,
                ProviderChoice::Cursor,
                ProviderChoice::Anthropic,
                ProviderChoice::OpenAiCompatible
            ]
        );
    }

    #[test]
    fn the_default_falls_back_to_the_first_configured_provider() {
        let dir =
            project("[providers.b]\nkind = \"claude_code\"\n[providers.a]\nkind = \"codex_cli\"\n");
        assert_eq!(default_provider(&config(&dir)).as_deref(), Some("a"));
        assert_eq!(default_provider(&config(&project(""))), None);
    }

    #[test]
    fn adding_a_cli_provider_writes_it_and_makes_it_the_default() {
        let dir = project("");
        let id = add(
            dir.path(),
            ProviderChoice::ClaudeCode,
            ProviderForm::default(),
        )
        .unwrap();
        assert_eq!(id, "claude");
        let config = config(&dir);
        assert!(matches!(
            config.providers["claude"],
            ProviderConfig::ClaudeCode { .. }
        ));
        assert_eq!(default_provider(&config).as_deref(), Some("claude"));
        assert_eq!(
            config.default.as_ref().map(|t| t.provider.as_str()),
            Some("claude")
        );

        let second = add(
            dir.path(),
            ProviderChoice::CodexCli,
            ProviderForm::default(),
        )
        .unwrap();
        assert_eq!(second, "codex");
        assert_eq!(
            config_default(&dir),
            Some("claude".into()),
            "an existing default stays"
        );
        make_default(dir.path(), "codex").unwrap();
        assert_eq!(config_default(&dir), Some("codex".into()));
        assert!(make_default(dir.path(), "missing").is_err());
    }

    fn config_default(dir: &tempfile::TempDir) -> Option<String> {
        config(dir).default.map(|t| t.provider)
    }

    #[test]
    fn a_taken_id_gets_a_number_and_a_duplicate_name_is_refused() {
        let dir = project("[providers.claude]\nkind = \"codex_cli\"\n");
        assert_eq!(
            free_id(&config(&dir), ProviderChoice::ClaudeCode),
            "claude-2"
        );
        let form = ProviderForm {
            id: "claude".into(),
            ..Default::default()
        };
        let err = add(dir.path(), ProviderChoice::ClaudeCode, form).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn api_providers_need_a_key_and_an_endpoint_needs_a_model() {
        let dir = project("");
        let err = add(
            dir.path(),
            ProviderChoice::Anthropic,
            ProviderForm::default(),
        )
        .unwrap_err();
        assert!(err.contains("API key"), "{err}");
        let form = ProviderForm {
            key: "sk-test".into(),
            base_url: "https://example.com/v1".into(),
            ..Default::default()
        };
        let err = add(dir.path(), ProviderChoice::OpenAiCompatible, form).unwrap_err();
        assert!(err.contains("model"), "{err}");
        assert!(config(&dir).providers.is_empty(), "nothing is written");
    }

    #[test]
    fn saving_a_key_needs_an_api_provider_with_a_credential_account() {
        let dir = project("[providers.cli]\nkind = \"claude_code\"\n[providers.env]\nkind = \"anthropic\"\napi_key_env = \"MY_KEY\"\n");
        let config = config(&dir);
        assert!(save_key(&config, "cli", "x")
            .unwrap_err()
            .contains("does not take"));
        assert!(save_key(&config, "env", "x")
            .unwrap_err()
            .contains("MY_KEY"));
        assert!(save_key(&config, "nope", "x")
            .unwrap_err()
            .contains("not configured"));
    }
}
