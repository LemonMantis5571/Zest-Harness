//! Secret storage for API-key providers.
//!
//! The provider config contains only a stable credential reference. Secret
//! values are kept in the platform credential manager and are never serialized
//! into provider views or configuration files.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[cfg_attr(target_os = "macos", allow(dead_code))]
const SERVICE: &str = "zest";
type CachedCredential = Result<Option<String>, String>;
type CredentialCache = HashMap<String, CachedCredential>;

// A denied or failed OS keychain lookup is re-asked by the UI's status poll
// every couple of seconds; without this, that turns one "Deny" click into a
// prompt repeating for as long as the poll runs. Cache the outcome for the
// life of the process: a denial (or a success) sticks until the caller
// explicitly changes it via `set`/`delete`, or the app restarts.
fn cache() -> &'static Mutex<CredentialCache> {
    static CACHE: OnceLock<Mutex<CredentialCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn invalidate(account: &str) {
    cache().lock().unwrap().remove(account);
}

pub fn get(account: &str) -> Result<Option<String>, String> {
    // Held across `fetch`, not just the cache read/write: two concurrent
    // callers for the same not-yet-cached account (e.g. the quota widget and
    // the provider list both querying on startup) would otherwise each run
    // their own `fetch` — on Windows/Linux that's each its own credential-
    // manager prompt for the same item. Serializing means the second caller
    // waits and gets the first caller's cached result instead of prompting
    // again.
    let mut guard = cache().lock().unwrap();
    if let Some(cached) = guard.get(account) {
        return cached.clone();
    }

    let result = fetch(account);
    guard.insert(account.to_string(), result.clone());
    result
}

#[cfg(not(target_os = "macos"))]
fn fetch(account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set(account: &str, secret: &str) -> Result<(), String> {
    if secret.trim().is_empty() {
        return Err("API key cannot be empty".into());
    }
    invalidate(account);
    keyring::Entry::new(SERVICE, account)
        .map_err(|e| e.to_string())?
        .set_password(secret)
        .map_err(|e| e.to_string())?;

    // Read it back through a *fresh* entry, which is how every later lookup
    // reaches it. Without a backend feature, `keyring` falls back to a mock
    // store that is per-Entry and in-memory: the write above returns Ok and the
    // secret is simply gone. That shipped once — keys looked saved and every
    // provider then reported itself unconfigured — so the write is not trusted
    // until a separate read confirms it.
    invalidate(account);
    match get(account) {
        Ok(Some(stored)) if stored == secret => Ok(()),
        Ok(_) => Err(
            "the key did not persist — this build has no OS credential store \
             (keyring needs a platform backend feature)"
                .into(),
        ),
        Err(err) => Err(format!("the key could not be read back: {err}")),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn delete(account: &str) -> Result<(), String> {
    invalidate(account);
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

// macOS: an unsigned/ad-hoc dev build has no stable code-signing identity, so
// the Keychain cannot durably trust it — every single lookup re-prompts for
// the login password (sometimes twice: unlock, then re-confirm access) with
// no way to make "Always Allow" stick between runs. Rather than fight that,
// keys are kept in a plain JSON file under the app's data directory, owner-only
// (0600) on disk. That is a deliberate, user-accepted trade of "OS-managed
// secret store" for "no interactive prompt on every launch" — not a fix for
// the signing problem, a different storage backend entirely.
#[cfg(target_os = "macos")]
fn store_path() -> Result<std::path::PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join("zest").join("credentials.json"))
        .ok_or_else(|| "could not locate the app data directory".to_string())
}

#[cfg(target_os = "macos")]
fn read_store() -> Result<HashMap<String, String>, String> {
    let path = store_path()?;
    match std::fs::read(&path) {
        Ok(raw) => {
            serde_json::from_slice(&raw).map_err(|e| format!("credentials file is corrupt: {e}"))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(target_os = "macos")]
fn write_store(store: &HashMap<String, String>) -> Result<(), String> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())?;

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn fetch(account: &str) -> Result<Option<String>, String> {
    Ok(read_store()?.get(account).cloned())
}

#[cfg(target_os = "macos")]
pub fn set(account: &str, secret: &str) -> Result<(), String> {
    if secret.trim().is_empty() {
        return Err("API key cannot be empty".into());
    }
    invalidate(account);
    let mut store = read_store()?;
    store.insert(account.to_string(), secret.to_string());
    write_store(&store)
}

#[cfg(target_os = "macos")]
pub fn delete(account: &str) -> Result<(), String> {
    invalidate(account);
    let mut store = read_store()?;
    if store.remove(account).is_some() {
        write_store(&store)?;
    }
    Ok(())
}

pub fn present(account: &str) -> Result<bool, String> {
    Ok(get(account)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_not_provider_secret() {
        assert_eq!(SERVICE, "zest");
    }

    /// A real credential store is compiled in.
    ///
    /// This is the regression that let API keys vanish: `keyring` with no
    /// backend feature uses an in-memory mock, so a round trip through two
    /// separate `Entry` values loses the secret. Touches the real OS store,
    /// under its own account name, and removes itself.
    #[test]
    fn a_secret_survives_a_round_trip_through_a_new_entry() {
        let account = format!("__zest_selftest_{}", std::process::id());
        let secret = "round-trip-canary";

        if let Err(err) = set(&account, secret) {
            // A headless Linux box has no secret service; that is an absent
            // store rather than a wrong one, so do not fail the suite on it.
            if cfg!(target_os = "linux") {
                eprintln!("no usable credential store here: {err}");
                return;
            }
            panic!("could not store a secret: {err}");
        }

        let read_back = get(&account).expect("read back");
        let _ = delete(&account);
        assert_eq!(read_back.as_deref(), Some(secret));
        assert_eq!(get(&account).expect("after delete"), None);
    }

    #[test]
    fn an_empty_secret_is_refused_before_it_reaches_the_store() {
        assert!(set("__zest_selftest_empty", "   ").is_err());
    }
}
