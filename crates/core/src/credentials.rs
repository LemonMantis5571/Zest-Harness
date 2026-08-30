//! Secret storage for API-key providers.
//!
//! The provider config contains only a stable credential reference. Secret
//! values are kept in the platform credential manager and are never serialized
//! into provider views or configuration files.
//!
//! Windows Credential Manager stores a password as a UTF-16 blob of at most
//! 2560 bytes (1280 characters). A ChatGPT session JSON is routinely larger,
//! so oversized secrets are split across numbered entries and reassembled.

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

/// Windows CredWriteW stores a UTF-16 blob of at most 2560 bytes
/// (`CRED_MAX_CREDENTIAL_BLOB_SIZE`). That is 1280 BMP characters. The crate
/// error still says "2560 chars", which is the byte cap, not the character cap.
#[cfg(any(not(target_os = "macos"), test))]
const WINDOWS_UTF16_LIMIT: usize = 1280;
/// Stay under the blob cap after UTF-16 encoding.
#[cfg(any(not(target_os = "macos"), test))]
const CHUNK_UTF16_LIMIT: usize = 1100;
#[cfg(any(not(target_os = "macos"), test))]
const CHUNKED_PREFIX: &str = "zest-chunked:";

#[cfg(any(not(target_os = "macos"), test))]
fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

#[cfg(any(not(target_os = "macos"), test))]
fn chunk_account(account: &str, index: usize) -> String {
    format!("{account}#zest-chunk-{index}")
}

#[cfg(any(not(target_os = "macos"), test))]
fn chunk_count(manifest: &str) -> Option<usize> {
    let count = manifest.strip_prefix(CHUNKED_PREFIX)?;
    let parsed = count.parse::<usize>().ok()?;
    (parsed > 0).then_some(parsed)
}

#[cfg(any(not(target_os = "macos"), test))]
fn split_for_credential_chunks(secret: &str, max_utf16: usize) -> Vec<String> {
    debug_assert!(max_utf16 > 0);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut units = 0;
    for ch in secret.chars() {
        let added = ch.len_utf16();
        if units + added > max_utf16 && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += added;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(any(not(target_os = "macos"), test))]
fn looks_like_windows_length_limit(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("2560") || lower.contains("longer than platform limit")
}

#[cfg(not(target_os = "macos"))]
fn fetch_plain(account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
fn store_plain(account: &str, secret: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, account)
        .map_err(|e| e.to_string())?
        .set_password(secret)
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn delete_plain(account: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_chunks(account: &str, count: usize) {
    for index in 0..count {
        let _ = delete_plain(&chunk_account(account, index));
    }
}

#[cfg(not(target_os = "macos"))]
fn assemble_chunks(account: &str, count: usize) -> Result<Option<String>, String> {
    let mut assembled = String::new();
    for index in 0..count {
        match fetch_plain(&chunk_account(account, index))? {
            Some(piece) => assembled.push_str(&piece),
            None => {
                return Err(format!(
                    "stored ChatGPT session is incomplete (missing piece {})",
                    index + 1
                ));
            }
        }
    }
    if assembled.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(assembled))
    }
}

#[cfg(not(target_os = "macos"))]
fn write_chunked(
    account: &str,
    secret: &str,
    previous_chunks: Option<usize>,
) -> Result<(), String> {
    let chunks = split_for_credential_chunks(secret, CHUNK_UTF16_LIMIT);
    if chunks.is_empty() {
        return Err("API key cannot be empty".into());
    }
    store_plain(account, &format!("{CHUNKED_PREFIX}{}", chunks.len()))?;
    for (index, chunk) in chunks.iter().enumerate() {
        store_plain(&chunk_account(account, index), chunk)?;
    }
    if let Some(previous) = previous_chunks {
        for index in chunks.len()..previous {
            let _ = delete_plain(&chunk_account(account, index));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn fetch(account: &str) -> Result<Option<String>, String> {
    match fetch_plain(account)? {
        None => Ok(None),
        Some(value) => {
            if let Some(count) = chunk_count(&value) {
                return assemble_chunks(account, count);
            }
            Ok(Some(value))
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set(account: &str, secret: &str) -> Result<(), String> {
    if secret.trim().is_empty() {
        return Err("API key cannot be empty".into());
    }
    let previous_chunks = fetch_plain(account)
        .ok()
        .flatten()
        .as_deref()
        .and_then(chunk_count);
    invalidate(account);
    let too_long_for_windows = cfg!(windows) && utf16_len(secret) >= WINDOWS_UTF16_LIMIT;
    if too_long_for_windows {
        write_chunked(account, secret, previous_chunks)?;
    } else if let Err(err) = store_plain(account, secret) {
        if looks_like_windows_length_limit(&err) {
            write_chunked(account, secret, previous_chunks)?;
        } else {
            return Err(err);
        }
    } else if let Some(count) = previous_chunks {
        delete_chunks(account, count);
    }

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
    let previous_chunks = fetch_plain(account)
        .ok()
        .flatten()
        .as_deref()
        .and_then(chunk_count);
    invalidate(account);
    delete_plain(account)?;
    if let Some(count) = previous_chunks {
        delete_chunks(account, count);
    }
    Ok(())
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

    #[test]
    fn chunk_manifest_is_only_a_count() {
        assert_eq!(chunk_count("zest-chunked:3"), Some(3));
        assert_eq!(chunk_count("zest-chunked:0"), None);
        assert_eq!(chunk_count("not-chunked"), None);
        assert_eq!(chunk_account("codex", 2), "codex#zest-chunk-2");
    }

    #[test]
    fn a_chatgpt_sized_secret_splits_under_the_windows_cap() {
        let secret = "é".repeat(3000);
        assert!(utf16_len(&secret) > WINDOWS_UTF16_LIMIT);
        let chunks = split_for_credential_chunks(&secret, CHUNK_UTF16_LIMIT);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks.concat(), secret);
        for chunk in &chunks {
            assert!(utf16_len(chunk) <= CHUNK_UTF16_LIMIT);
        }
    }

    #[test]
    fn the_windows_length_error_is_recognized() {
        assert!(looks_like_windows_length_limit(
            "Attribute 'password encoded as UTF-16' is longer than platform limit of 2560 chars"
        ));
    }

    /// ChatGPT session JSON is larger than Windows will store in one item.
    /// The write has to survive that cap and still read back as one secret.
    #[test]
    fn a_secret_over_the_windows_limit_survives_a_round_trip() {
        let account = format!("__zest_selftest_long_{}", std::process::id());
        let secret = format!("{{\"access_token\":\"{}\"}}", "a".repeat(3000));
        assert!(utf16_len(&secret) >= WINDOWS_UTF16_LIMIT);

        if let Err(err) = set(&account, &secret) {
            if cfg!(target_os = "linux") {
                eprintln!("no usable credential store here: {err}");
                return;
            }
            panic!("could not store a long secret: {err}");
        }

        let read_back = get(&account).expect("read back");
        let _ = delete(&account);
        assert_eq!(read_back.as_deref(), Some(secret.as_str()));
        assert_eq!(get(&account).expect("after delete"), None);
    }
}
