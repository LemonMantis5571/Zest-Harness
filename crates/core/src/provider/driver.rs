//! One driver per provider kind, and one place that maps a kind to its driver.
//!
//! Construction and capability dispatch through **one** match, [`driver_for`].
//! Before this, construction lived in `registry::build` and the picker catalogue
//! lived in `provider::descriptor_from_config`, and the two had already drifted — an
//! `anthropic` entry with `model = "claude-haiku-5"` offered one model in the
//! picker and accepted two at runtime, because the live path *prepended* the
//! configured model to a catalogue that already held `DEFAULT_MODEL`. Two
//! independent matches over the same variants disagreeing is the argument for
//! this module; each driver now answers both questions from one helper.
//!
//! `quota.rs` still matches on [`ProviderConfig`] and that is deliberate. It
//! produces a value nothing else has to agree with, and being exhaustive it
//! already fails to compile when a kind is added — so it carries none of the
//! drift risk that a *pair* of matches does.
//!
//! Deliberately **not** here: a `DriverCapabilities` struct. `owns_agent_loop`,
//! `supports_prompt_cache`, and `resume_support` are already on [`Provider`], and
//! every caller holds a constructed provider — so declaring them again on the
//! driver would add a second source of truth with no reader, which is the exact
//! failure this module exists to remove.

use std::path::Path;
use std::sync::Arc;

use super::anthropic::AnthropicProvider;
use super::claude_code::ClaudeCodeProvider;
use super::codex_app_server::CodexAppServerProvider;
use super::codex_oauth::CodexOAuthProvider;
use super::openai_compatible::OpenAiCompatibleProvider;
use crate::codex_oauth::SESSION_ENV;
use super::{catalogue, EffortPolicy, ModelSpec, Provider, ProviderDescriptor, CODEX_KNOWN_MODELS};
use crate::anthropic::types::DEFAULT_MODEL;
use crate::config::{ProviderConfig, DEFAULT_CLAUDE_CODE_MODEL};

/// The `kind = "…"` string this driver owns. Equal to the serde tag, and checked
/// against it by `driver_kinds_round_trip_through_the_config_tag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverKind(pub &'static str);

/// What must be true about credentials for this entry to be usable.
///
/// These three policies existed before, unnamed, as differently-shaped error
/// handling in each arm of `registry::build`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPolicy {
    /// The vendor runtime owns the session. Zest never reads or holds a key, and
    /// a missing sign-in is the CLI's problem to report, not a load failure.
    VendorOwned,
    /// No key means the provider cannot be built at all, and the reason has to
    /// name what to set.
    RequiredToLoad,
    /// A key is used when present. Absence is legitimate — a local server on
    /// loopback needs none — so it changes the provider, not whether it loads.
    OptionalToLoad,
}

/// Where this entry's key may be found, and whether it has to be there.
///
/// The driver states *where*; [`resolve`] does the reading. Splitting it this way
/// is what lets one resolver serve every kind: a driver cannot forget to check
/// the credential manager before the environment, because it never looks itself.
#[derive(Debug, Clone, Copy)]
pub struct CredentialRequest<'a> {
    /// OS credential-manager account, checked first.
    pub account: Option<&'a str>,
    /// Environment variable name, checked when the store holds nothing.
    pub env: Option<&'a str>,
    pub policy: CredentialPolicy,
}

impl CredentialRequest<'_> {
    const VENDOR_OWNED: Self = Self {
        account: None,
        env: None,
        policy: CredentialPolicy::VendorOwned,
    };
}

/// Everything a driver needs that does not come from the config entry.
pub struct DriverContext<'a> {
    /// The `[providers.<id>]` name. The usage ledger and every config lookup key
    /// on this, so it is the provider's identity — never the kind or the vendor.
    pub id: &'a str,
    /// Active project root, for kinds that spawn a runtime inside the workspace.
    pub root: &'a Path,
    /// Resolved by [`resolve`] under this driver's own [`CredentialPolicy`].
    pub key: Option<String>,
}

pub trait ProviderDriver: Send + Sync {
    fn kind(&self) -> DriverKind;

    /// Human name for this kind, for UI labels and error text.
    fn display_name(&self) -> &'static str;

    fn credentials<'a>(&self, config: &'a ProviderConfig) -> CredentialRequest<'a>;

    /// The catalogue without loading credentials or spawning anything.
    ///
    /// Must agree with what [`ProviderDriver::create`] produces; every
    /// implementation below derives both from one private helper, and
    /// `the_picker_catalogue_matches_the_live_provider_catalogue` checks it.
    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor;

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String>;
}

/// Read the key this entry asks for: credential manager first, environment second.
///
/// Promoted from `quota.rs`, which already filtered blank account names — the
/// registry did not, so a provider with `credential = ""` asked the OS store for
/// an empty account and reported its failure as a missing key.
pub fn resolve(request: CredentialRequest<'_>) -> std::result::Result<Option<String>, String> {
    let stored = request
        .account
        .filter(|account| !account.trim().is_empty())
        .map(crate::credentials::get)
        .transpose()
        .map_err(|error| format!("could not read the saved API key: {error}"))?
        .flatten();
    Ok(stored.or_else(|| {
        request
            .env
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
    }))
}

/// Credential request with the provider id filled in when the entry omitted it.
///
/// ChatGPT Codex stores its session under the provider id by default. The
/// driver cannot see that id from the config struct alone.
pub fn credentials_for<'a>(id: &'a str, config: &'a ProviderConfig) -> CredentialRequest<'a> {
    let driver = driver_for(config);
    let mut request = driver.credentials(config);
    if driver.kind().0 == "codex_oauth" {
        let blank = request
            .account
            .map(|account| account.trim().is_empty())
            .unwrap_or(true);
        if blank {
            request.account = Some(id);
        }
    }
    request
}

/// Resolve and enforce in one step, so no caller can do the first without the second.
pub fn resolve_required(
    request: CredentialRequest<'_>,
) -> std::result::Result<Option<String>, String> {
    let key = resolve(request)?;
    if key.is_none() && request.policy == CredentialPolicy::RequiredToLoad {
        return Err(match (request.account, request.env) {
            (Some(account), _) if !account.trim().is_empty() => {
                format!("API key for credential `{account}` is not set")
            }
            (_, Some(env)) => format!("{env} is not set in the environment"),
            _ => "no credential is configured for this provider".to_string(),
        });
    }
    Ok(key)
}

// ---------------------------------------------------------------- anthropic

struct AnthropicDriver;

impl AnthropicDriver {
    /// The single source both `descriptor` and `create` read.
    fn catalogue(config: &ProviderConfig) -> (String, Vec<ModelSpec>) {
        let ProviderConfig::Anthropic { model, .. } = config else {
            unreachable!("driver_for routes only Anthropic entries here");
        };
        let default_model = model.clone().unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let models = catalogue(&default_model, &[], &[], EffortPolicy::Standard(&[]));
        (default_model, models)
    }
}

impl ProviderDriver for AnthropicDriver {
    fn kind(&self) -> DriverKind {
        DriverKind("anthropic")
    }

    fn display_name(&self) -> &'static str {
        "Anthropic"
    }

    fn credentials<'a>(&self, config: &'a ProviderConfig) -> CredentialRequest<'a> {
        let ProviderConfig::Anthropic {
            api_key_env,
            credential,
            ..
        } = config
        else {
            unreachable!("driver_for routes only Anthropic entries here");
        };
        CredentialRequest {
            account: credential.as_deref(),
            env: Some(api_key_env),
            policy: CredentialPolicy::RequiredToLoad,
        }
    }

    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor {
        let (default_model, models) = Self::catalogue(config);
        ProviderDescriptor {
            id: id.to_string(),
            default_model,
            models,
        }
    }

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String> {
        let (default_model, models) = Self::catalogue(config);
        let provider = AnthropicProvider::new(
            ctx.id.to_string(),
            ctx.key.unwrap_or_default(),
            default_model,
            models,
        )
        .map_err(|error| format!("could not build client: {error}"))?;
        Ok(Arc::new(provider))
    }
}

// -------------------------------------------------------------- claude_code

struct ClaudeCodeDriver;

impl ClaudeCodeDriver {
    fn catalogue(config: &ProviderConfig) -> (String, Vec<ModelSpec>) {
        let ProviderConfig::ClaudeCode { model, models, .. } = config else {
            unreachable!("driver_for routes only ClaudeCode entries here");
        };
        let default_model = model
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLAUDE_CODE_MODEL.to_string());
        let catalogue = catalogue(
            &default_model,
            models,
            super::claude_code::BUILTIN_MODELS,
            EffortPolicy::Unsupported,
        );
        (default_model, catalogue)
    }
}

impl ProviderDriver for ClaudeCodeDriver {
    fn kind(&self) -> DriverKind {
        DriverKind("claude_code")
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn credentials<'a>(&self, _config: &'a ProviderConfig) -> CredentialRequest<'a> {
        CredentialRequest::VENDOR_OWNED
    }

    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor {
        let (default_model, models) = Self::catalogue(config);
        ProviderDescriptor {
            id: id.to_string(),
            default_model,
            models,
        }
    }

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String> {
        let ProviderConfig::ClaudeCode {
            command,
            model,
            models,
            allow_mcp,
            permission_mode,
            timeout_secs,
        } = config
        else {
            unreachable!("driver_for routes only ClaudeCode entries here");
        };
        let provider = ClaudeCodeProvider::new(
            ctx.id.to_string(),
            ctx.root,
            command.clone(),
            model.clone(),
            models.clone(),
            *allow_mcp,
            *permission_mode,
            *timeout_secs,
        )
        .map_err(|error| format!("could not build Claude Code provider: {error}"))?;
        Ok(Arc::new(provider))
    }
}

// ---------------------------------------------------------------- codex_cli

struct CodexCliDriver;

impl CodexCliDriver {
    fn catalogue(config: &ProviderConfig) -> (String, Vec<ModelSpec>) {
        let ProviderConfig::CodexCli {
            model,
            models,
            efforts,
            ..
        } = config
        else {
            unreachable!("driver_for routes only CodexCli entries here");
        };
        // The built-in list is the *kind's*, not the id's. Under the old
        // `provider_id == "codex"` match a `codex_cli` entry named anything else
        // silently lost it.
        let catalogue = catalogue(
            model,
            models,
            CODEX_KNOWN_MODELS,
            EffortPolicy::Standard(efforts),
        );
        (model.clone(), catalogue)
    }
}

impl ProviderDriver for CodexCliDriver {
    fn kind(&self) -> DriverKind {
        DriverKind("codex_cli")
    }

    fn display_name(&self) -> &'static str {
        "Codex CLI"
    }

    fn credentials<'a>(&self, _config: &'a ProviderConfig) -> CredentialRequest<'a> {
        CredentialRequest::VENDOR_OWNED
    }

    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor {
        let (default_model, models) = Self::catalogue(config);
        ProviderDescriptor {
            id: id.to_string(),
            default_model,
            models,
        }
    }

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String> {
        let ProviderConfig::CodexCli {
            command,
            model,
            models,
            efforts,
            allow_mcp,
            timeout_secs,
        } = config
        else {
            unreachable!("driver_for routes only CodexCli entries here");
        };
        let provider = CodexAppServerProvider::new(
            ctx.id.to_string(),
            ctx.root,
            command.clone(),
            model.clone(),
            models.clone(),
            efforts.clone(),
            *allow_mcp,
            *timeout_secs,
        )
        .map_err(|error| format!("could not build Codex CLI provider: {error}"))?;
        Ok(Arc::new(provider))
    }
}

// -------------------------------------------------------------- codex_oauth

struct CodexOAuthDriver;

impl CodexOAuthDriver {
    fn catalogue(config: &ProviderConfig) -> (String, Vec<ModelSpec>) {
        let ProviderConfig::CodexOAuth {
            model,
            models,
            efforts,
            ..
        } = config
        else {
            unreachable!("driver_for routes only CodexOAuth entries here");
        };
        let catalogue = catalogue(
            model,
            models,
            CODEX_KNOWN_MODELS,
            EffortPolicy::Standard(efforts),
        );
        (model.clone(), catalogue)
    }
}

impl ProviderDriver for CodexOAuthDriver {
    fn kind(&self) -> DriverKind {
        DriverKind("codex_oauth")
    }

    fn display_name(&self) -> &'static str {
        "ChatGPT Codex"
    }

    fn credentials<'a>(&self, config: &'a ProviderConfig) -> CredentialRequest<'a> {
        let ProviderConfig::CodexOAuth { credential, .. } = config else {
            unreachable!("driver_for routes only CodexOAuth entries here");
        };
        CredentialRequest {
            account: credential.as_deref(),
            env: Some(SESSION_ENV),
            policy: CredentialPolicy::RequiredToLoad,
        }
    }

    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor {
        let (default_model, models) = Self::catalogue(config);
        ProviderDescriptor {
            id: id.to_string(),
            default_model,
            models,
        }
    }

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String> {
        let ProviderConfig::CodexOAuth {
            model,
            credential,
            ..
        } = config
        else {
            unreachable!("driver_for routes only CodexOAuth entries here");
        };
        let (_, models) = Self::catalogue(config);
        let account = credential
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(ctx.id);
        let provider = CodexOAuthProvider::from_key(ctx.id, account, ctx.key, model.clone(), models)?;
        Ok(Arc::new(provider))
    }
}

// -------------------------------------------------------- openai_compatible

struct OpenAiCompatibleDriver;

impl OpenAiCompatibleDriver {
    fn catalogue(config: &ProviderConfig) -> (String, Vec<ModelSpec>) {
        let ProviderConfig::OpenaiCompatible { model, models, .. } = config else {
            unreachable!("driver_for routes only OpenaiCompatible entries here");
        };
        // The v1 adapter sends no reasoning field, so a selector here would look
        // authoritative and change nothing.
        let catalogue = catalogue(model, models, &[], EffortPolicy::Unsupported);
        (model.clone(), catalogue)
    }
}

impl ProviderDriver for OpenAiCompatibleDriver {
    fn kind(&self) -> DriverKind {
        DriverKind("openai_compatible")
    }

    fn display_name(&self) -> &'static str {
        "OpenAI-compatible"
    }

    fn credentials<'a>(&self, config: &'a ProviderConfig) -> CredentialRequest<'a> {
        let ProviderConfig::OpenaiCompatible {
            credential,
            api_key_env,
            ..
        } = config
        else {
            unreachable!("driver_for routes only OpenaiCompatible entries here");
        };
        CredentialRequest {
            account: credential.as_deref(),
            env: api_key_env.as_deref(),
            policy: CredentialPolicy::OptionalToLoad,
        }
    }

    fn descriptor(&self, id: &str, config: &ProviderConfig) -> ProviderDescriptor {
        let (default_model, models) = Self::catalogue(config);
        ProviderDescriptor {
            id: id.to_string(),
            default_model,
            models,
        }
    }

    fn create(
        &self,
        ctx: DriverContext<'_>,
        config: &ProviderConfig,
    ) -> std::result::Result<Arc<dyn Provider>, String> {
        let ProviderConfig::OpenaiCompatible {
            base_url,
            model,
            credential,
            api_key_env,
            ..
        } = config
        else {
            unreachable!("driver_for routes only OpenaiCompatible entries here");
        };
        let (_, catalogue) = Self::catalogue(config);
        let mut provider = OpenAiCompatibleProvider::new(
            ctx.id.to_string(),
            ctx.key.unwrap_or_default(),
            base_url.clone(),
            model.clone(),
        )
        .map_err(|error| format!("could not build client: {error}"))?
        .with_models(catalogue);
        // Declaring a secret source is what makes a missing key a user-visible
        // problem; a keyless local server must not be reported as unconfigured.
        if credential.is_some() || api_key_env.is_some() {
            provider = provider.with_key_requirement();
        } else {
            provider = provider.without_key_requirement();
        }
        Ok(Arc::new(provider))
    }
}

// ------------------------------------------------------------------- table

pub const BUILT_IN_DRIVERS: &[&(dyn ProviderDriver + Sync)] = &[
    &AnthropicDriver,
    &ClaudeCodeDriver,
    &CodexCliDriver,
    &CodexOAuthDriver,
    &OpenAiCompatibleDriver,
];

/// The one exhaustive match over [`ProviderConfig`].
///
/// Adding a provider means adding a variant and one arm here; the round-trip
/// test below is what ties the two together, since Rust cannot.
pub fn driver_for(config: &ProviderConfig) -> &'static (dyn ProviderDriver + Sync) {
    match config {
        ProviderConfig::Anthropic { .. } => &AnthropicDriver,
        ProviderConfig::ClaudeCode { .. } => &ClaudeCodeDriver,
        ProviderConfig::CodexCli { .. } => &CodexCliDriver,
        ProviderConfig::CodexOAuth { .. } => &CodexOAuthDriver,
        ProviderConfig::OpenaiCompatible { .. } => &OpenAiCompatibleDriver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn entry(toml: &str) -> ProviderConfig {
        let config = Config::parse(toml).expect("valid test config");
        config.providers.into_values().next().expect("one provider")
    }

    /// The type system cannot tie [`BUILT_IN_DRIVERS`] to [`ProviderConfig`], so
    /// this does: every kind string in the table routes back to the driver that
    /// claims it, and the table has no duplicates and no strays.
    #[test]
    fn driver_kinds_round_trip_through_the_config_tag() {
        let samples = [
            ("anthropic", "[providers.p]\nkind = \"anthropic\"\n"),
            ("claude_code", "[providers.p]\nkind = \"claude_code\"\n"),
            ("codex_cli", "[providers.p]\nkind = \"codex_cli\"\n"),
            ("codex_oauth", "[providers.p]\nkind = \"codex_oauth\"\n"),
            (
                "openai_compatible",
                "[providers.p]\nkind = \"openai_compatible\"\nbase_url = \"http://x/v1\"\nmodel = \"m\"\n",
            ),
        ];
        assert_eq!(
            samples.len(),
            BUILT_IN_DRIVERS.len(),
            "every driver needs a sample, and every sample a driver"
        );

        for (kind, toml) in samples {
            let config = entry(toml);
            assert_eq!(
                driver_for(&config).kind(),
                DriverKind(kind),
                "`kind = \"{kind}\"` must route to the driver that claims it"
            );
            assert_eq!(
                BUILT_IN_DRIVERS
                    .iter()
                    .filter(|driver| driver.kind() == DriverKind(kind))
                    .count(),
                1,
                "exactly one driver may claim `{kind}`"
            );
        }
    }

    /// A `codex_cli` entry keeps the built-in catalogue under any id. The old
    /// `provider_id == "codex"` match meant a second Codex account under a
    /// different name could select only its configured default.
    #[test]
    fn the_codex_catalogue_follows_the_kind_not_the_provider_id() {
        let config = entry("[providers.work-codex]\nkind = \"codex_cli\"\n");
        let descriptor = driver_for(&config).descriptor("work-codex", &config);
        assert!(
            descriptor.models.iter().any(|m| m.id == "gpt-5.6-luna"),
            "expected the built-in catalogue, got {:?}",
            descriptor.models.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
    }

    /// A key that is required must be named in the failure, because the name *is*
    /// the fix. An optional one must not fail at all.
    #[test]
    fn a_required_key_names_itself_and_an_optional_one_is_allowed_to_be_absent() {
        std::env::remove_var("ZEST_TEST_DRIVER_ABSENT");
        let required = entry(
            "[providers.p]\nkind = \"anthropic\"\napi_key_env = \"ZEST_TEST_DRIVER_ABSENT\"\n",
        );
        let error = resolve_required(driver_for(&required).credentials(&required))
            .expect_err("a required key must fail loudly");
        assert!(error.contains("ZEST_TEST_DRIVER_ABSENT"), "{error}");

        let optional = entry(
            "[providers.p]\nkind = \"openai_compatible\"\nbase_url = \"http://127.0.0.1:11434/v1\"\nmodel = \"m\"\n",
        );
        assert_eq!(
            resolve_required(driver_for(&optional).credentials(&optional)),
            Ok(None),
            "a loopback server with no key configured still loads"
        );
    }

    /// A vendor-owned kind must never be asked for a key: a missing `claude
    /// login` is the CLI's to report, not a reason the provider fails to load.
    #[test]
    fn a_vendor_owned_kind_is_never_asked_for_a_key() {
        for toml in [
            "[providers.p]\nkind = \"claude_code\"\n",
            "[providers.p]\nkind = \"codex_cli\"\n",
        ] {
            let config = entry(toml);
            let request = driver_for(&config).credentials(&config);
            assert_eq!(request.policy, CredentialPolicy::VendorOwned);
            assert!(request.account.is_none() && request.env.is_none());
            assert_eq!(resolve_required(request), Ok(None));
        }
    }

    #[test]
    fn codex_oauth_is_required_to_load_and_does_not_read_vendor_files() {
        let config = entry("[providers.p]\nkind = \"codex_oauth\"\n");
        let request = credentials_for("p", &config);
        assert_eq!(request.policy, CredentialPolicy::RequiredToLoad);
        assert_eq!(request.account, Some("p"));
        assert_eq!(request.env, Some(SESSION_ENV));
        std::env::remove_var(SESSION_ENV);
        let error = resolve_required(request).expect_err("a missing ChatGPT session must fail");
        assert!(
            error.contains("p") || error.contains(SESSION_ENV),
            "{error}"
        );
        assert!(
            !error.contains("auth.json"),
            "must not mention the vendor CLI store: {error}"
        );
    }

    /// A blank account name is not a credential-manager lookup.
    ///
    /// `quota.rs` filtered this and the registry did not, so `credential = ""`
    /// asked the OS store for an empty account and reported the failure as a
    /// missing key.
    #[test]
    fn a_blank_credential_account_falls_through_to_the_environment() {
        std::env::set_var("ZEST_TEST_DRIVER_ENV", "present");
        let key = resolve(CredentialRequest {
            account: Some("   "),
            env: Some("ZEST_TEST_DRIVER_ENV"),
            policy: CredentialPolicy::OptionalToLoad,
        });
        std::env::remove_var("ZEST_TEST_DRIVER_ENV");
        assert_eq!(key, Ok(Some("present".to_string())));
    }
}
