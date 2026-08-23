//! Building live providers from configuration.
//!
//! A provider that cannot be constructed is **skipped with a reason**, not
//! treated as fatal. Half the point of this harness is that some accounts are
//! available and some are not; one missing key must not prevent the rest from
//! loading. The reasons come back so the picker can show them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{driver, Provider};
use crate::config::{Config, ProviderConfig};

/// A provider that could not be built, and why — phrased for a user to act on.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub id: String,
    pub reason: String,
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    /// Build every provider the config declares.
    ///
    /// Returns the registry plus everything skipped. Never errors: an empty
    /// registry with reasons is more useful than a failed startup.
    pub fn from_config(config: &Config) -> (Self, Vec<Skipped>) {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::from_config_at(config, &root)
    }

    /// Build providers with the active project root available to providers
    /// whose vendor runtime operates on the workspace, such as Claude Code.
    pub fn from_config_at(config: &Config, root: &Path) -> (Self, Vec<Skipped>) {
        let mut providers: BTreeMap<String, Arc<dyn Provider>> = BTreeMap::new();
        let mut skipped = Vec::new();

        // A provider a migration had to drop is absent from `providers` entirely,
        // so without this it would look like it was never configured.
        for (id, reason) in config.unsupported() {
            skipped.push(Skipped {
                id: id.clone(),
                reason: reason.clone(),
            });
        }

        for (id, entry) in &config.providers {
            match build(id, entry, root) {
                Ok(provider) => {
                    providers.insert(id.clone(), provider);
                }
                Err(reason) => skipped.push(Skipped {
                    id: id.clone(),
                    reason,
                }),
            }
        }

        (Self { providers }, skipped)
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
    }

    /// Insert or replace a provider (tests and custom assembly).
    pub fn insert(&mut self, provider: Arc<dyn Provider>) {
        let id = provider.id().to_string();
        self.providers.insert(id, provider);
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// Build one provider from its config entry.
///
/// Every kind-specific decision — where the key lives, whether it is required,
/// what catalogue to offer, what to construct — belongs to the entry's driver.
/// This function's whole job is to resolve the credential the driver asks for
/// and enforce the policy it declared, so no driver can read a secret itself
/// and none can forget to check the credential manager before the environment.
fn build(
    id: &str,
    entry: &ProviderConfig,
    root: &Path,
) -> std::result::Result<Arc<dyn Provider>, String> {
    let driver = driver::driver_for(entry);
    let key = driver::resolve_required(driver::credentials_for(id, entry))?;
    driver.create(driver::DriverContext { id, root, key }, entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthStatus;

    #[test]
    fn one_missing_key_does_not_stop_the_others() {
        // Two providers; only the gateway's key is present.
        std::env::set_var("ZEST_TEST_GATEWAY_KEY", "present");
        std::env::remove_var("ZEST_TEST_ABSENT_KEY");

        let config = Config::parse(
            r#"
[providers.anthropic]
kind = "anthropic"
api_key_env = "ZEST_TEST_ABSENT_KEY"

[providers.codex]
kind = "gateway"
base_url = "http://127.0.0.1:8317"
api_key_env = "ZEST_TEST_GATEWAY_KEY"
model = "gpt-5.3-codex"
"#,
        )
        .expect("valid config");

        let (registry, skipped) = ProviderRegistry::from_config(&config);

        assert_eq!(registry.len(), 1, "the usable provider still loaded");
        assert!(registry.get("codex").is_some());
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].id, "anthropic");
        assert!(
            skipped[0].reason.contains("ZEST_TEST_ABSENT_KEY"),
            "the reason names the variable to set: {}",
            skipped[0].reason
        );

        std::env::remove_var("ZEST_TEST_GATEWAY_KEY");
    }

    #[test]
    fn a_provider_keeps_the_id_it_was_configured_under() {
        std::env::set_var("ZEST_TEST_KEY_2", "present");

        let config = Config::parse(
            r#"
[providers.house]
kind = "anthropic"
api_key_env = "ZEST_TEST_KEY_2"
model = "claude-haiku-5"
"#,
        )
        .unwrap();

        let (registry, _) = ProviderRegistry::from_config(&config);
        let provider = registry.get("house").expect("built");

        // The ledger and configuration key on this, so it must be the config
        // name rather than the vendor's or the kind's.
        assert_eq!(provider.id(), "house");
        assert_eq!(provider.default_model(), "claude-haiku-5");
        assert!(matches!(provider.auth_status(), AuthStatus::Ready { .. }));

        std::env::remove_var("ZEST_TEST_KEY_2");
    }

    #[test]
    fn an_openai_compatible_provider_with_no_key_env_is_allowed() {
        let config = Config::parse(
            r#"
[providers.local]
kind = "openai_compatible"
base_url = "http://127.0.0.1:11434/v1"
model = "llama"
"#,
        )
        .unwrap();

        let (registry, skipped) = ProviderRegistry::from_config(&config);
        assert_eq!(registry.len(), 1, "no key declared means no key required");
        assert!(skipped.is_empty());
    }
}
