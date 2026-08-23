//! `zest.toml` — which providers exist, which one starts a chat, and which
//! external workers Zest may invoke through ACP or a headless CLI.
//!
//! Two principles shape this file:
//!
//! 1. **A missing config is not an error.** With no `zest.toml`, Zest falls back
//!    to a single Anthropic provider from the environment, which is exactly how
//!    it behaved before config existed.
//! 2. **An unusable provider is skipped, not fatal.** The whole premise is that
//!    some providers are available and some are not. One missing key must not
//!    stop the others from loading — it becomes a warning the picker can show.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{HarnessError, Result};

pub const CONFIG_FILE: &str = "zest.toml";
pub const DEFAULT_CLAUDE_CODE_MODEL: &str = "sonnet";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-sol";

/// Safe starter config embedded in every build. It contains provider metadata,
/// never a credential, so a fresh install can bootstrap user-global config
/// without asking the user to copy files out of the source checkout.
pub const DEFAULT_USER_CONFIG: &str = include_str!("../../../zest.toml.example");

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Optional external coding agents invoked through a non-interactive CLI
    /// or Agent Client Protocol (ACP) stdio session. These are deliberately
    /// separate from providers: they are workers, not the identity of the
    /// parent conversation.
    #[serde(default)]
    pub agents: BTreeMap<String, ExternalAgentConfig>,
    /// MCP servers Zest itself connects to, keyed by id.
    ///
    /// Deliberately not the same thing as `allow_mcp` on a CLI provider or
    /// worker: those servers belong to that CLI's configuration, and Zest
    /// neither sees nor approves the calls. These are Zest's own, and they
    /// exist because a native provider — an Anthropic key, or an
    /// OpenAI-compatible endpoint such as DeepSeek — has no CLI harness to
    /// borrow MCP servers from.
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServerConfig>,
    /// Provider used when a front-end does not choose one explicitly.
    ///
    /// This is intentionally separate from external workers: ACP agents are
    /// selected by the model through `delegate_external`, not by a provider
    /// provider-routing policy.
    #[serde(default)]
    pub default: Option<Target>,
    /// Read-only compatibility for configurations written before ACP became
    /// the only delegation path. The old routing rules are parsed so an
    /// existing config does not prevent Zest from starting, but they are never
    /// executed; `default` is migrated through `default_target` below.
    #[serde(default, rename = "routing")]
    legacy_routing: Option<LegacyRouting>,
    #[serde(default)]
    pub tools: ToolsConfig,
    /// Set by [`Config::parse`] when it had to rewrite a legacy document.
    /// Never read from or written to TOML — the file on disk is left alone.
    #[serde(skip)]
    migrations: Vec<String>,
    /// Providers a migration could not map onto a surviving kind.
    #[serde(skip)]
    unsupported: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRouting {
    #[serde(default)]
    default: Option<Target>,
    #[serde(default)]
    delegation: bool,
    /// The old rules are intentionally opaque. They are accepted only so an
    /// installed user config can be opened and replaced from the UI without a
    /// hard startup failure.
    #[serde(default)]
    rules: Vec<toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    #[serde(default)]
    pub bash: BashConfig,
    /// Bytes a tool result may occupy in the model's context before the full text
    /// is stored under `.zest/spill` and the model gets a bounded preview plus a
    /// locator it can read or grep.
    ///
    /// `0` keeps every result inline however large — the old behavior, where the
    /// bytes past a tool's own cap were simply gone.
    #[serde(default = "default_max_result_bytes")]
    pub max_result_bytes: usize,
}

/// Hand-written rather than derived: a derived `usize` default is `0`, which this
/// field reads as "disabled" — the one value that must not be what an absent
/// `[tools]` section means.
impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            bash: BashConfig::default(),
            max_result_bytes: default_max_result_bytes(),
        }
    }
}

/// 32 KiB — roughly 8k tokens, already a heavy result, and just above `bash`'s
/// own 30 KiB output cap so the one tool whose truncation markers are pinned by
/// tests is never also carrying a spill notice.
fn default_max_result_bytes() -> usize {
    32 * 1024
}

/// `[tools.bash]`. Absent means the defaults below, which is a working setup —
/// the tool ships on with only read-only commands running unattended.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Extra command prefixes that may run without approval, each given as its
    /// own token list (`[["just", "lint"]]`). Still subject to the shell
    /// metacharacter rule — an entry here cannot opt out of that.
    #[serde(default)]
    pub extra_allowlist: Vec<Vec<String>>,
    /// Substrings that force approval even for an otherwise allowlisted
    /// command. Checked first, so this always wins.
    #[serde(default)]
    pub denylist: Vec<String>,
    #[serde(default = "default_bash_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_allowlist: Vec::new(),
            denylist: Vec::new(),
            timeout_ms: default_bash_timeout_ms(),
        }
    }
}

impl BashConfig {
    pub fn settings(&self) -> crate::tools::bash::BashSettings {
        crate::tools::bash::BashSettings {
            extra_allowlist: self.extra_allowlist.clone(),
            denylist: self.denylist.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_bash_timeout_ms() -> u64 {
    crate::tools::bash::DEFAULT_TIMEOUT_MS
}

/// How to reach one provider.
///
/// `kind` discriminates, and it is the only thing that does: transport,
/// credentials, and capabilities are all decided from this variant and never
/// inferred from a provider id. Two of the five kinds spawn a vendor runtime
/// that owns its own agent loop; `codex_oauth` uses Zest's loop. See
/// `Provider::owns_agent_loop`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderConfig {
    Anthropic {
        /// Environment variable holding the key. The key itself is never
        /// written in config — this file is meant to be committed. A desktop
        /// setup may instead use `credential` below.
        #[serde(default = "default_anthropic_key_env")]
        api_key_env: String,
        #[serde(default)]
        model: Option<String>,
        /// OS credential-manager account for a key entered through the desktop.
        /// When present, it takes precedence over `api_key_env`.
        #[serde(default)]
        credential: Option<String>,
    },
    /// Claude Code owns the authenticated subscription session and its
    /// built-in coding tools. Unlike an external agent, this provider is the
    /// identity of the parent conversation.
    ClaudeCode {
        /// Executable name or absolute path. No shell is involved.
        #[serde(default = "default_claude_code_command")]
        command: String,
        /// Optional CLI-owned model alias or model id. Omitted uses 'sonnet'.
        #[serde(default)]
        model: Option<String>,
        /// Optional allow-list for the Claude Code model picker.
        #[serde(default)]
        models: Vec<String>,
        /// Let Claude Code load MCP servers from its own configuration.
        #[serde(default)]
        allow_mcp: bool,
        /// Permission mode passed to Claude Code's non-interactive runtime.
        #[serde(default)]
        permission_mode: ClaudeCodePermissionMode,
        /// Parent process limit, capped at the same bound as delegated workers.
        #[serde(default = "default_external_timeout_secs")]
        timeout_secs: u64,
    },
    /// Native Codex app-server. Authentication and account state belong to
    /// the Codex CLI (`codex login`), never to zest.toml.
    CodexCli {
        /// Executable name or absolute path. Spawned directly, without a shell.
        #[serde(default = "default_codex_command")]
        command: String,
        /// Default model sent to `thread/start` / `turn/start`.
        #[serde(default = "default_codex_model")]
        model: String,
        /// Optional allow-list for the model picker. Empty uses the built-in
        /// conservative catalogue until a successful `model/list` is cached.
        #[serde(default)]
        models: Vec<String>,
        /// Optional effort allow-list for every listed model. When empty, the
        /// standard set (`low`…`max`) is used. Carried over from the migrated
        /// `gateway` kind, which was the only place this could be expressed.
        #[serde(default)]
        efforts: Vec<String>,
        /// MCP is opt-in because the app-server can execute server-owned work
        /// outside Zest's approval boundary.
        #[serde(default)]
        allow_mcp: bool,
        #[serde(default = "default_external_timeout_secs")]
        timeout_secs: u64,
    },
    /// ChatGPT subscription via Zest-owned OAuth. Available even when the
    /// Codex CLI is installed; the two kinds use different ids. Tokens live
    /// in the credential manager, not here.
    #[serde(rename = "codex_oauth")]
    CodexOAuth {
        #[serde(default = "default_codex_model")]
        model: String,
        #[serde(default)]
        models: Vec<String>,
        #[serde(default)]
        efforts: Vec<String>,
        /// OS credential-manager account. Defaults to the provider id.
        #[serde(default)]
        credential: Option<String>,
    },
    OpenaiCompatible {
        /// API root, for example `https://api.openai.com/v1` or
        /// `https://api.deepseek.com`. The client appends `/chat/completions`.
        base_url: String,
        /// The model used when the default provider does not choose one
        /// explicitly.
        model: String,
        /// Optional allow-list. Empty means only `model` is accepted.
        #[serde(default)]
        models: Vec<String>,
        /// Reserved for future provider-specific effort support. The v1
        /// OpenAI-compatible adapter ignores this field and does not expose an
        /// effort selector until a wire mapping is implemented.
        #[serde(default)]
        efforts: Vec<String>,
        /// OS credential-manager account name. Defaults to the provider id.
        #[serde(default)]
        credential: Option<String>,
        /// Headless/CI fallback. Never written by Zest's setup UI.
        #[serde(default)]
        api_key_env: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCodePermissionMode {
    /// Let Claude Code apply its own interactive/default permission policy.
    #[default]
    Default,
    /// Allow file edits while retaining Claude Code's command safeguards.
    AcceptEdits,
    /// Keep the parent session read-only and plan-oriented.
    Plan,
    /// Disable Claude Code permission prompts. Use only in a throwaway tree.
    BypassPermissions,
}

impl ClaudeCodePermissionMode {
    pub fn cli_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
        }
    }
}

/// An external coding agent Zest may invoke as an explicit delegated worker.
///
/// `command` and `args` are passed directly to the operating system process
/// API; Zest never constructs a shell command. Put `{prompt}` in `args` when a
/// CLI needs the prompt at a particular position. Without it, headless mode
/// appends the prompt as the final argument. `{model}` is expanded when a model
/// is configured, and is left alone otherwise so a missing model fails clearly
/// in the child CLI rather than silently selecting a different one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAgentConfig {
    /// `headless` consumes newline-delimited JSON from stdout. `acp` speaks
    /// JSON-RPC over stdio and lets Zest proxy the worker workspace boundary.
    #[serde(default)]
    pub mode: ExternalAgentMode,
    /// Executable name or absolute path. No shell is involved.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Let the child CLI use MCP servers from its own configuration. This is
    /// opt-in because Zest cannot inspect or approve individual MCP calls.
    #[serde(default)]
    pub allow_mcp: bool,
    /// Optional label/model shown in the delegation result and available as
    /// the `{model}` argument placeholder.
    #[serde(default)]
    pub model: Option<String>,
    /// Isolated Git worktree by default. `current` is an explicit escape hatch
    /// for read-only/non-Git projects and is never selected implicitly.
    #[serde(default)]
    pub workspace: ExternalWorkspace,
    /// Child process limit. Capped by the runner to avoid a config typo making
    /// a turn wait indefinitely.
    #[serde(default = "default_external_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAgentMode {
    #[default]
    Headless,
    Acp,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalWorkspace {
    #[default]
    Isolated,
    Current,
}

fn default_external_timeout_secs() -> u64 {
    900
}

/// One MCP server Zest starts and calls itself.
///
/// `command` and `args` go straight to the operating system process API; no
/// shell is involved, and there is no placeholder expansion — an MCP server is
/// a long-lived stdio process, not a per-prompt invocation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Executable name or absolute path.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Secret-looking host environment variables this server may keep, by
    /// name.
    ///
    /// Names only, never values. Zest scrubs anything that looks like a
    /// credential from the child environment; a server that genuinely needs a
    /// token names the variable here, and the value stays in the machine's own
    /// environment where `zest.toml` can still be committed.
    #[serde(default)]
    pub env_vars: Vec<String>,
    /// Configured servers stay in the file when switched off, so turning one
    /// off is not the same as losing how it was set up.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Per-request limit. Clamped by the client, so a config typo cannot make
    /// a turn wait indefinitely.
    #[serde(default = "default_mcp_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_mcp_timeout_secs() -> u64 {
    120
}

impl ProviderConfig {
    pub fn key_env(&self) -> Option<&str> {
        match self {
            ProviderConfig::Anthropic { api_key_env, .. } => Some(api_key_env),
            ProviderConfig::ClaudeCode { .. }
            | ProviderConfig::CodexCli { .. }
            | ProviderConfig::CodexOAuth { .. } => None,
            ProviderConfig::OpenaiCompatible { api_key_env, .. } => api_key_env.as_deref(),
        }
    }

    /// The serde `kind` tag for this entry. Capability comes from this, never
    /// from the provider id.
    pub fn kind_tag(&self) -> &'static str {
        match self {
            ProviderConfig::Anthropic { .. } => "anthropic",
            ProviderConfig::ClaudeCode { .. } => "claude_code",
            ProviderConfig::CodexCli { .. } => "codex_cli",
            ProviderConfig::CodexOAuth { .. } => "codex_oauth",
            ProviderConfig::OpenaiCompatible { .. } => "openai_compatible",
        }
    }
}

fn default_anthropic_key_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}

fn default_claude_code_command() -> String {
    "claude".to_string()
}

fn default_codex_command() -> String {
    "codex".to_string()
}

fn default_codex_model() -> String {
    DEFAULT_CODEX_MODEL.to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Provider id from `[providers.<id>]`.
    pub provider: String,
    /// Omitted means the provider's own default.
    #[serde(default)]
    pub model: Option<String>,
    /// Omitted means `high`.
    #[serde(default)]
    pub effort: Option<String>,
}

/// Load `.env` from the project (searching upward), then `~/.zest/.env`.
///
/// The second one is the point: a key like `ANTHROPIC_API_KEY` belongs to the
/// machine for the same reason the provider list does. With only the upward
/// search, opening a folder outside the Zest checkout finds no `.env` at all
/// and a correctly-configured provider fails for want of a credential.
///
/// dotenv semantics are first-wins and never clobber a variable already in the
/// environment, so a project `.env` still overrides the user one, and a real
/// environment variable overrides both.
pub fn load_env() {
    let _ = dotenvy::dotenv();
    if let Some(home) = dirs::home_dir() {
        let _ = dotenvy::from_path(home.join(".zest").join(".env"));
    }
}

/// User-global config: `~/.zest/zest.toml`.
///
/// Which accounts you are signed into is a property of the machine, not of a
/// repository. Without this, opening any folder that happens not to contain a
/// `zest.toml` would drop you back to the bare Anthropic-from-env fallback and
/// fail with "provider `codex` could not be loaded" — even though nothing about
/// your Codex login changed by opening a different directory.
pub fn user_config_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".zest").join(CONFIG_FILE))
}

/// Create the machine-level config on first launch, preserving anything the
/// user already has. A project config still takes precedence when one exists.
pub fn ensure_user_config() -> Result<Option<PathBuf>> {
    // Fail during development/build verification if the committed starter
    // config ever becomes invalid, instead of writing a broken first-run file.
    Config::parse(DEFAULT_USER_CONFIG)?;

    let Some(path) = user_config_path() else {
        return Ok(None);
    };
    if ensure_config_file(&path, DEFAULT_USER_CONFIG)? {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

fn ensure_config_file(path: &Path, contents: &str) -> Result<bool> {
    if path.is_file() {
        return Ok(false);
    }
    let parent = path.parent().ok_or_else(|| {
        HarnessError::Other(format!("config path has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| HarnessError::Other(format!("cannot create {}: {e}", parent.display())))?;

    // create_new is deliberate: two Zest processes racing on first launch
    // cannot replace a config that appeared between the existence check and
    // this open.
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => {
            return Err(HarnessError::Other(format!(
                "cannot create {}: {e}",
                path.display()
            )))
        }
    };

    if let Err(e) = (|| -> std::io::Result<()> {
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()
    })() {
        let _ = std::fs::remove_file(path);
        return Err(HarnessError::Other(format!(
            "cannot write {}: {e}",
            path.display()
        )));
    }
    Ok(true)
}

impl Config {
    /// Look for `zest.toml` in `dir`, then `~/.zest/zest.toml`. Absent is not an
    /// error — see module note.
    ///
    /// Project config **replaces** user config rather than merging into it.
    /// Merging two provider tables would make it genuinely hard to answer
    /// "which account is this about to spend", and that question has to stay
    /// easy.
    pub fn find(dir: impl AsRef<Path>) -> Result<Self> {
        let path = dir.as_ref().join(CONFIG_FILE);
        if path.is_file() {
            return Self::load_from(path);
        }
        if let Some(user) = user_config_path().filter(|p| p.is_file()) {
            return Self::load_from(user);
        }
        Ok(Self::env_fallback())
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .map_err(|e| HarnessError::Other(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&raw)
    }

    /// Return the names of provider environment variables that must not be
    /// inherited by an external worker. This intentionally exposes names only;
    /// credential values remain in the parent process or credential manager.
    pub fn provider_key_env_names(&self) -> Vec<String> {
        let mut names = self
            .providers
            .values()
            .filter_map(ProviderConfig::key_env)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        names.sort_unstable_by_key(|name| name.to_ascii_lowercase());
        names.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        names
    }

    /// Parse `zest.toml`, migrating a legacy `gateway` provider if that is what
    /// made it unparseable.
    ///
    /// Strict first, deliberately. A two-stage parse for everyone would lose
    /// serde's span information, so an ordinary typo would degrade into a vague
    /// message for the sake of a legacy path. If the migrated document still
    /// fails, the *original* error is what the user sees — the rewrite is not a
    /// suspect worth naming when the real problem is elsewhere.
    pub fn parse(raw: &str) -> Result<Self> {
        let strict = match toml::from_str::<Self>(raw) {
            Ok(config) => return Ok(config),
            Err(strict) => strict,
        };

        let invalid = || HarnessError::Other(format!("{CONFIG_FILE} is invalid: {strict}"));
        let Some((migrated, report)) = crate::config_migrate::migrate(raw) else {
            return Err(invalid());
        };
        let mut config: Self = toml::from_str(&migrated).map_err(|_| invalid())?;
        config.migrations = report.migrations;
        config.unsupported = report.unsupported;
        Ok(config)
    }

    /// Notices for providers this load rewrote in memory. Empty for a config
    /// that parsed strictly, which is every config Zest writes today.
    pub fn migrations(&self) -> &[String] {
        &self.migrations
    }

    /// Providers dropped by a migration, as `(id, reason)`. The registry turns
    /// these into `Skipped` entries so the picker can explain the absence.
    pub fn unsupported(&self) -> &[(String, String)] {
        &self.unsupported
    }

    /// The zero-config shape: one Anthropic provider keyed off the environment.
    pub fn env_fallback() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig::Anthropic {
                api_key_env: default_anthropic_key_env(),
                model: None,
                credential: None,
            },
        );
        Config {
            providers,
            agents: BTreeMap::new(),
            mcp: BTreeMap::new(),
            default: Some(Target {
                provider: "anthropic".to_string(),
                model: None,
                effort: None,
            }),
            legacy_routing: None,
            tools: ToolsConfig::default(),
            migrations: Vec::new(),
            unsupported: Vec::new(),
        }
    }

    /// Which provider a new session uses when the front-end did not choose one.
    ///
    /// Falls back to a legacy `[routing].default` for existing installations,
    /// then to the only configured provider. A single-provider config needs no
    /// default section at all.
    pub fn default_target(&self) -> Option<Target> {
        if let Some(target) = &self.default {
            return Some(target.clone());
        }
        if let Some(target) = self
            .legacy_routing
            .as_ref()
            .and_then(|routing| routing.default.as_ref())
        {
            return Some(target.clone());
        }
        if self.providers.len() == 1 {
            return self.providers.keys().next().map(|id| Target {
                provider: id.clone(),
                model: None,
                effort: None,
            });
        }
        None
    }

    /// Config problems worth showing the user that are not parse errors —
    /// dangling references that would otherwise fail much later, at dispatch.
    pub fn lint(&self) -> Vec<String> {
        // Migration notices come first: they explain a document the user did not
        // write, and every other issue below may be a consequence of one.
        let mut issues = self.migrations.clone();

        if let Some(target) = self.default.as_ref().or_else(|| {
            self.legacy_routing
                .as_ref()
                .and_then(|r| r.default.as_ref())
        }) {
            if !self.providers.contains_key(&target.provider) {
                issues.push(format!(
                    "default provider points at unknown provider `{}`",
                    target.provider
                ));
            }
        }
        if let Some(legacy) = &self.legacy_routing {
            if legacy.delegation || !legacy.rules.is_empty() {
                issues.push(
                    "legacy [routing] delegation is ignored; configure ACP workers under [agents.*]"
                        .into(),
                );
            }
        }
        // A cap this small leaves no room for a preview after the locator notice
        // is paid for, so every oversized result would come back as notice-only —
        // or stay inline, if even the notice does not fit.
        if self.tools.max_result_bytes > 0 && self.tools.max_result_bytes < 4_096 {
            issues.push(format!(
                "[tools] max_result_bytes = {} is too small to leave room for a preview; use 0 to keep results inline or at least 4096",
                self.tools.max_result_bytes
            ));
        }
        for (id, agent) in &self.agents {
            if agent.command.trim().is_empty() {
                issues.push(format!("external agent `{id}` has an empty command"));
            }
            if agent.timeout_secs == 0 || agent.timeout_secs > 3_600 {
                issues.push(format!(
                    "external agent `{id}` timeout_secs must be between 1 and 3600"
                ));
            }
        }
        for (id, provider) in &self.providers {
            match provider {
                ProviderConfig::ClaudeCode {
                    command,
                    timeout_secs,
                    ..
                } => {
                    if command.trim().is_empty() {
                        issues.push(format!("Claude Code provider {id} has an empty command"));
                    }
                    if *timeout_secs == 0 || *timeout_secs > 3_600 {
                        issues.push(format!(
                            "Claude Code provider {id} timeout_secs must be between 1 and 3600"
                        ));
                    }
                }
                ProviderConfig::CodexCli {
                    command,
                    timeout_secs,
                    ..
                } => {
                    if command.trim().is_empty() {
                        issues.push(format!("Codex CLI provider {id} has an empty command"));
                    }
                    if *timeout_secs == 0 || *timeout_secs > 3_600 {
                        issues.push(format!(
                            "Codex CLI provider {id} timeout_secs must be between 1 and 3600"
                        ));
                    }
                }
                _ => {}
            }
        }
        issues
    }
}

/// Where the config was found, for error messages.
pub fn config_path(dir: impl AsRef<Path>) -> PathBuf {
    dir.as_ref().join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
[providers.anthropic]
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"

[providers.codex]
kind = "codex_cli"
model = "gpt-5.3-codex"

[default]
provider = "anthropic"
model = "claude-opus-5"
"#;

    #[test]
    fn parses_providers_and_default() {
        let config = Config::parse(FULL).expect("valid");

        assert_eq!(config.providers.len(), 2);
        assert!(matches!(
            config.providers["anthropic"],
            ProviderConfig::Anthropic { .. }
        ));
        match &config.providers["codex"] {
            ProviderConfig::CodexCli {
                command,
                model,
                models,
                efforts,
                ..
            } => {
                assert_eq!(command, "codex");
                assert_eq!(model, "gpt-5.3-codex");
                assert!(models.is_empty());
                assert!(efforts.is_empty());
            }
            other => panic!("expected the Codex CLI, got {other:?}"),
        }

        let target = config.default_target().expect("default");
        assert_eq!(target.provider, "anthropic");
        assert_eq!(target.model.as_deref(), Some("claude-opus-5"));

        assert_eq!(config.default.as_ref().unwrap().provider, "anthropic");
    }

    #[test]
    fn accepts_legacy_routing_without_executing_it() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"

[routing]
default = { provider = "anthropic" }
delegation = true

[[routing.rules]]
kind = "mechanical"
provider = "anthropic"
"#,
        )
        .expect("legacy config remains readable");

        assert_eq!(config.default_target().unwrap().provider, "anthropic");
        let warnings = config.lint();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("ACP workers")));
    }

    #[test]
    fn parses_openai_compatible_provider_without_a_secret() {
        let config = Config::parse(
            r#"
[providers.deepseek]
kind = "openai_compatible"
base_url = "https://api.deepseek.com"
model = "deepseek-v4-flash"
models = ["deepseek-v4-flash", "deepseek-v4-pro"]
credential = "deepseek"
"#,
        )
        .expect("valid OpenAI-compatible config");
        match &config.providers["deepseek"] {
            ProviderConfig::OpenaiCompatible {
                base_url,
                model,
                credential,
                ..
            } => {
                assert_eq!(base_url, "https://api.deepseek.com");
                assert_eq!(model, "deepseek-v4-flash");
                assert_eq!(credential.as_deref(), Some("deepseek"));
            }
            other => panic!("expected OpenAI-compatible provider, got {other:?}"),
        }
    }

    #[test]
    fn parses_claude_code_parent_defaults() {
        let config = Config::parse(
            r#"
[providers.claude]
kind = "claude_code"
model = "opus"
permission_mode = "accept_edits"
"#,
        )
        .expect("valid Claude Code provider config");

        match &config.providers["claude"] {
            ProviderConfig::ClaudeCode {
                command,
                model,
                models,
                allow_mcp,
                permission_mode,
                timeout_secs,
            } => {
                assert_eq!(command, "claude");
                assert_eq!(model.as_deref(), Some("opus"));
                assert!(models.is_empty());
                assert!(!allow_mcp);
                assert_eq!(*permission_mode, ClaudeCodePermissionMode::AcceptEdits);
                assert_eq!(*timeout_secs, 900);
            }
            other => panic!("expected Claude Code provider, got {other:?}"),
        }
    }

    /// Transport, credentials, and capabilities are decided from `kind` alone.
    /// A provider id is a label — naming one `codex` must not change how it is
    /// reached.
    #[test]
    fn every_kind_parses_to_exactly_its_own_variant() {
        let config = Config::parse(
            r#"
[providers.claude]
kind = "claude_code"

[providers.codex]
kind = "codex_cli"

[providers.anthropic]
kind = "anthropic"

[providers.local]
kind = "openai_compatible"
base_url = "http://127.0.0.1:11434/v1"
model = "local"

[providers.chatgpt]
kind = "codex_oauth"
"#,
        )
        .expect("valid provider-kind config");

        assert!(matches!(
            config.providers["claude"],
            ProviderConfig::ClaudeCode { .. }
        ));
        assert!(matches!(
            config.providers["codex"],
            ProviderConfig::CodexCli { .. }
        ));
        assert!(matches!(
            config.providers["anthropic"],
            ProviderConfig::Anthropic { .. }
        ));
        assert!(matches!(
            config.providers["local"],
            ProviderConfig::OpenaiCompatible { .. }
        ));
        match &config.providers["chatgpt"] {
            ProviderConfig::CodexOAuth {
                model, credential, ..
            } => {
                assert_eq!(model, DEFAULT_CODEX_MODEL);
                assert!(credential.is_none());
            }
            other => panic!("expected ChatGPT Codex, got {other:?}"),
        }
    }

    #[test]
    fn parses_external_headless_and_acp_agents_without_provider_changes() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"

[agents.claude]
mode = "headless"
command = "claude"
allow_mcp = true
args = [
    "--print",
    "--output-format", "stream-json",
    "{prompt}",
]
workspace = "isolated"

[agents.gemini]
mode = "acp"
command = "gemini"
args = ["--acp"]
workspace = "current"
timeout_secs = 120
"#,
        )
        .expect("valid external agent config");

        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.agents["claude"].mode, ExternalAgentMode::Headless);
        assert!(config.agents["claude"].allow_mcp);
        assert_eq!(
            config.agents["claude"].workspace,
            ExternalWorkspace::Isolated
        );
        assert_eq!(config.agents["gemini"].mode, ExternalAgentMode::Acp);
        assert_eq!(config.agents["gemini"].timeout_secs, 120);
    }

    #[test]
    fn collects_configured_provider_key_environment_names_without_values() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"
api_key_env = "CUSTOM_AUTH"

[providers.remote]
kind = "openai_compatible"
base_url = "http://127.0.0.1:8317/v1"
api_key_env = "REMOTE_AUTH"
model = "model"

[providers.local]
kind = "openai_compatible"
base_url = "http://127.0.0.1:11434/v1"
api_key_env = "custom_auth"
model = "local"
"#,
        )
        .expect("valid provider env config");

        assert_eq!(
            config.provider_key_env_names(),
            vec!["CUSTOM_AUTH".to_string(), "REMOTE_AUTH".to_string()]
        );
    }

    #[test]
    fn external_agent_defaults_are_safe() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"

[agents.claude]
command = "claude"
"#,
        )
        .unwrap();
        let agent = &config.agents["claude"];
        assert_eq!(agent.mode, ExternalAgentMode::Headless);
        assert!(!agent.allow_mcp);
        assert_eq!(agent.workspace, ExternalWorkspace::Isolated);
        assert_eq!(agent.timeout_secs, 900);
    }

    #[test]
    fn a_single_provider_needs_no_default_section() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"
"#,
        )
        .expect("valid");

        assert_eq!(config.default_target().unwrap().provider, "anthropic");
    }

    #[test]
    fn two_providers_without_a_default_is_ambiguous() {
        let config = Config::parse(
            r#"
[providers.a]
kind = "anthropic"

[providers.b]
kind = "codex_cli"
"#,
        )
        .expect("valid");

        // Guessing which of two accounts to spend would be the wrong kind of helpful.
        assert!(config.default_target().is_none());
    }

    #[test]
    fn lint_catches_a_default_provider_that_does_not_exist() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"

[default]
provider = "typo"
"#,
        )
        .expect("parses");

        let issues = config.lint();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("typo"));
    }

    #[test]
    fn the_result_cap_defaults_without_a_tools_section() {
        let config =
            Config::parse("[providers.anthropic]\nkind = \"anthropic\"\n").expect("parses");
        assert_eq!(config.tools.max_result_bytes, 32 * 1024);
        // And through the other constructor, which is where a derived `0` default
        // would have slipped in.
        assert_eq!(Config::default().tools.max_result_bytes, 32 * 1024);
        assert!(config.lint().is_empty(), "{:?}", config.lint());
    }

    #[test]
    fn the_result_cap_is_configurable_and_zero_is_left_alone() {
        let config = Config::parse("[tools]\nmax_result_bytes = 0\n").expect("parses");
        assert_eq!(config.tools.max_result_bytes, 0);
        assert!(
            config.lint().is_empty(),
            "0 is an explicit opt-out, not a mistake: {:?}",
            config.lint()
        );

        let config = Config::parse("[tools]\nmax_result_bytes = 65536\n").expect("parses");
        assert_eq!(config.tools.max_result_bytes, 65_536);
        assert!(config.lint().is_empty());
    }

    #[test]
    fn a_tiny_result_cap_is_linted() {
        let config = Config::parse("[tools]\nmax_result_bytes = 64\n").expect("parses");
        let issues = config.lint();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("max_result_bytes"), "{issues:?}");
    }

    #[test]
    fn an_openai_compatible_provider_without_a_model_is_rejected() {
        let err = Config::parse(
            r#"
[providers.remote]
kind = "openai_compatible"
base_url = "http://127.0.0.1:8317/v1"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("model"), "{err}");
    }

    #[test]
    fn codex_cli_may_list_supported_models_and_efforts() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "codex_cli"
model = "gpt-5.6-sol"
models = ["gpt-5.6-sol", "gpt-5.6-terra"]
efforts = ["low", "high", "max"]
"#,
        )
        .expect("valid");
        match &config.providers["codex"] {
            ProviderConfig::CodexCli {
                models, efforts, ..
            } => {
                assert_eq!(
                    models,
                    &["gpt-5.6-sol".to_string(), "gpt-5.6-terra".to_string()]
                );
                assert_eq!(
                    efforts,
                    &["low".to_string(), "high".to_string(), "max".to_string()]
                );
            }
            other => panic!("expected the Codex CLI, got {other:?}"),
        }
    }

    #[test]
    fn native_codex_defaults_are_explicit() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "codex_cli"
"#,
        )
        .expect("valid");

        match &config.providers["codex"] {
            ProviderConfig::CodexCli {
                command,
                model,
                allow_mcp,
                timeout_secs,
                ..
            } => {
                assert_eq!(command, "codex");
                assert_eq!(model, DEFAULT_CODEX_MODEL);
                assert!(!allow_mcp);
                assert_eq!(*timeout_secs, 900);
            }
            other => panic!("expected native Codex provider, got {other:?}"),
        }
    }

    /// The whole migration, through the front door.
    ///
    /// The strict parse fails on `kind = "gateway"`, the document is rewritten in
    /// memory, and what comes back is a config the rest of Zest can use — with
    /// notices explaining a shape the user did not write.
    #[test]
    fn a_legacy_gateway_document_loads_through_parse_with_notices() {
        let config = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
api_key_env = "ZEST_GATEWAY_KEY"
model = "gpt-5.6-sol"
efforts = ["low", "high"]

[providers.claude]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "claude-opus-5"
models = ["claude-opus-5"]

[providers.gemini]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gemini-3.1-pro"

[default]
provider = "claude"
model = "claude-opus-5"
"#,
        )
        .expect("a legacy gateway document must still load");

        match &config.providers["codex"] {
            ProviderConfig::CodexCli {
                command,
                model,
                efforts,
                ..
            } => {
                assert_eq!(command, "codex");
                assert_eq!(model, "gpt-5.6-sol");
                assert_eq!(efforts, &["low".to_string(), "high".to_string()]);
            }
            other => panic!("expected the Codex CLI, got {other:?}"),
        }
        match &config.providers["claude"] {
            ProviderConfig::ClaudeCode {
                command, models, ..
            } => {
                assert_eq!(command, "claude");
                assert!(models.is_empty(), "API model ids are not CLI aliases");
            }
            other => panic!("expected Claude Code, got {other:?}"),
        }
        assert!(
            !config.providers.contains_key("gemini"),
            "an unmappable entry is dropped rather than guessed at"
        );

        let target = config.default_target().expect("a default survives");
        assert_eq!(target.provider, "claude");
        assert_eq!(
            target.model, None,
            "a pin the migrated provider can no longer offer would be a hard \
             startup error"
        );

        assert_eq!(config.migrations().len(), 2);
        assert_eq!(config.unsupported().len(), 1);
        assert_eq!(config.unsupported()[0].0, "gemini");
        // Every notice reaches a user: the front-ends already print lint output.
        let issues = config.lint();
        assert!(issues.iter().any(|i| i.contains("Codex CLI")), "{issues:?}");
        assert!(
            issues.iter().any(|i| i.contains("Claude Code")),
            "{issues:?}"
        );
    }

    /// A migration must not become a suspect for an unrelated mistake. The
    /// rewrite runs, the result still fails, and the *original* error is what
    /// the user sees.
    #[test]
    fn a_typo_still_reports_the_original_error_after_a_failed_migration() {
        let err = Config::parse(
            r#"
[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
model = "gpt-5.6-sol"

[nonsense]
what = 1
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("nonsense"),
            "the surviving problem must be named: {err}"
        );
        assert!(
            !err.contains("migrat"),
            "the rewrite is not the story here: {err}"
        );
    }

    /// A config Zest writes today parses strictly, so it must carry no notices.
    #[test]
    fn a_current_document_records_no_migration() {
        let config = Config::parse(FULL).expect("valid");
        assert!(config.migrations().is_empty());
        assert!(config.unsupported().is_empty());
    }

    #[test]
    fn an_unknown_kind_is_rejected_rather_than_ignored() {
        let err = Config::parse(
            r#"
[providers.mystery]
kind = "telepathy"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("telepathy"), "{err}");
    }

    #[test]
    fn a_typo_in_a_field_name_is_rejected() {
        // deny_unknown_fields: a silently ignored `base_urls` would send traffic
        // to the wrong place with no warning.
        let err = Config::parse(
            r#"
[providers.remote]
kind = "openai_compatible"
base_urls = "http://127.0.0.1:8317/v1"
model = "m"
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("base_urls"), "{err}");
    }

    #[test]
    fn bash_defaults_to_enabled_with_no_tools_section() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"
"#,
        )
        .expect("valid");
        assert!(config.tools.bash.enabled);
        assert!(config.tools.bash.extra_allowlist.is_empty());
        assert_eq!(
            config.tools.bash.timeout_ms,
            crate::tools::bash::DEFAULT_TIMEOUT_MS
        );
    }

    #[test]
    fn bash_section_parses_allowlist_and_denylist() {
        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"

[tools.bash]
enabled = false
extra_allowlist = [["just", "lint"], ["make", "test"]]
denylist = ["cargo publish"]
timeout_ms = 5000
"#,
        )
        .expect("valid");
        let bash = &config.tools.bash;
        assert!(!bash.enabled);
        assert_eq!(bash.extra_allowlist.len(), 2);
        assert_eq!(bash.extra_allowlist[0], vec!["just", "lint"]);
        assert_eq!(bash.denylist, vec!["cargo publish".to_string()]);
        assert_eq!(bash.timeout_ms, 5000);

        // The settings a tool actually receives carry the same values.
        let settings = bash.settings();
        assert_eq!(settings.timeout_ms, 5000);
        assert_eq!(settings.denylist, vec!["cargo publish".to_string()]);
    }

    #[test]
    fn the_repo_config_parses() {
        // The example is embedded into fresh installs; a typo in its committed
        // `[tools.bash]` block would break launch, not just this test.
        let raw = include_str!("../../../zest.toml.example");
        let config = Config::parse(raw).expect("committed zest.toml.example must parse");
        assert!(config.tools.bash.enabled);
        assert!(config.lint().is_empty(), "{:?}", config.lint());
    }

    #[test]
    fn first_run_config_is_valid_and_never_overwrites_user_config() {
        let dir = std::env::temp_dir().join(format!(
            "zest-config-bootstrap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);

        assert!(ensure_config_file(&path, DEFAULT_USER_CONFIG).unwrap());
        assert_eq!(
            Config::load_from(&path)
                .unwrap()
                .default_target()
                .unwrap()
                .provider,
            "codex"
        );
        assert!(!ensure_config_file(&path, "this must not replace the user's file").unwrap());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("[providers.codex]"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn env_fallback_is_a_working_single_provider_config() {
        let config = Config::env_fallback();
        assert_eq!(config.default_target().unwrap().provider, "anthropic");
        assert!(config.lint().is_empty());
    }
}
