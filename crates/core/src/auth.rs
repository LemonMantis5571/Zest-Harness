//! Detecting which providers are already signed in, and starting a login.
//!
//! Vendor CLIs still own their own sessions. Codex is the exception: when the
//! `codex` binary is missing, Zest can run a ChatGPT sign-in itself and store
//! its own session in the credential manager. That path still never reads
//! tokens out of `~/.codex/auth.json`.
//!
//! Two rules this module holds to:
//!
//! 1. **Never read or surface a secret.** Detection checks that a credential
//!    store exists and is well-formed. It does not extract tokens, and no value
//!    from a credential file is ever logged or returned.
//! 2. **Never claim "not logged in" when the real answer is "can't tell".** Some
//!    providers keep credentials somewhere we cannot inspect — an OS keychain, an
//!    encrypted blob. Reporting those as logged-out would push the user to
//!    re-authenticate for no reason, so they get `Unknown` instead.
//!
//! The desktop exposes native shells for the Zest-managed Codex sign-in and the
//! first-class Claude Code parent sign-in. Claude Code and Gemini CLI workers
//! still authenticate in their own tools; Zest never exchanges those tokens or
//! presents a worker session as the parent.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::tools::external_agent::resolve_program;

pub const CODEX_OAUTH_CALLBACK_PORT: u16 = 1455;

/// Copy on the waiting screen for a vendor CLI login. Those flows need a real
/// console: they open a browser only when stdin looks like a terminal, then
/// wait for Enter after "Login successful".
const CLI_LOGIN_BODY: &str =
    "A sign-in window should open. Finish in the browser, then press Enter there if it asks.";

/// Windows `CREATE_NEW_CONSOLE`. A hidden process with nulled stdio is why
/// Connect used to sit on "Waiting for browser sign-in" forever: `claude login`
/// never got a TTY, so it never opened a browser.
#[cfg(windows)]
const LOGIN_CREATION_FLAGS: u32 = 0x0000_0010;

/// What a provider's sign-in looks like from the outside.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthStatus {
    /// A credential store was found and is well-formed.
    ///
    /// `account` is a display label (an email, a plan name) when the provider
    /// exposes one in a non-secret field, `None` when it does not. It is never a
    /// token or any part of one.
    Ready { account: Option<String> },

    /// The credential store is absent. `fix` is the command that creates it.
    NotLoggedIn { fix: String },

    /// The provider is installed but its credentials are somewhere we cannot
    /// inspect. Offer it and let the request fail with a real error rather than
    /// pre-emptively greying it out.
    Unknown { reason: String },

    /// Bring-your-own-key with no key supplied yet.
    Unconfigured,
}

impl AuthStatus {
    /// Whether the UI should let the user pick this provider.
    ///
    /// `Unknown` counts as selectable on purpose — see the module note.
    pub fn selectable(&self) -> bool {
        matches!(self, AuthStatus::Ready { .. } | AuthStatus::Unknown { .. })
    }
}

/// One row in the launch picker.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderSlot {
    /// Stable id used by config and the usage ledger.
    pub id: &'static str,
    pub label: &'static str,
    /// How this provider is authenticated, in words the picker can show.
    pub method: &'static str,
    pub status: AuthStatus,
}

/// Resolved spawn plan for a Connect action. Owned paths so gateway binaries
/// under `tools/` work without being on PATH.
#[derive(Debug, Clone)]
pub struct LoginSpawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Short title for the waiting screen ("Sign in with ChatGPT").
    pub browser_title: &'static str,
    /// Body copy while the system browser completes OAuth.
    pub browser_body: &'static str,
}

/// A login helper that Zest can observe after it has been launched.
pub struct LoginProcess {
    pub spawn: LoginSpawn,
    child: Option<Child>,
    oauth: Option<crate::codex_oauth::CodexOAuthLogin>,
    /// Vendor store as it looked when Connect started. Presence and mtime only;
    /// the file is never opened. A rewrite means login finished even if the
    /// CLI is still sitting on "Press Enter to continue".
    store: Option<StoreSnapshot>,
}

impl LoginProcess {
    /// Account used for an in-process ChatGPT sign-in. `None` for CLI logins.
    pub fn oauth_account(&self) -> Option<&str> {
        self.oauth.as_ref().map(|login| login.account())
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        if self.oauth.is_some() {
            return Ok(None);
        }
        self.child
            .as_mut()
            .expect("login process has a child or an in-process sign-in")
            .try_wait()
    }

    pub fn poll_status(&mut self) -> std::io::Result<LoginPoll> {
        if let Some(oauth) = &self.oauth {
            return Ok(match oauth.poll() {
                crate::codex_oauth::CodexOAuthPoll::Running => LoginPoll::Running,
                crate::codex_oauth::CodexOAuthPoll::Succeeded => LoginPoll::Succeeded,
                crate::codex_oauth::CodexOAuthPoll::Failed(detail) => LoginPoll::Failed(detail),
            });
        }
        if self.store.as_ref().is_some_and(store_rewritten) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Ok(LoginPoll::Succeeded);
        }
        Ok(match self.try_wait()? {
            None => LoginPoll::Running,
            Some(status) => login_poll_from_cli_exit(status),
        })
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        if let Some(oauth) = &self.oauth {
            oauth.cancel();
            return Ok(());
        }
        let child = self
            .child
            .as_mut()
            .expect("login process has a child or an in-process sign-in");
        child.kill()?;
        let _ = child.wait();
        Ok(())
    }
}

fn login_poll_from_cli_exit(status: std::process::ExitStatus) -> LoginPoll {
    if status.success() {
        LoginPoll::Succeeded
    } else {
        LoginPoll::Failed("The sign-in did not finish. Try again.".into())
    }
}

/// Outcome of an in-flight Connect, including ChatGPT sign-in success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPoll {
    Running,
    Succeeded,
    Failed(String),
}

/// Every provider Zest knows how to look for, in display order.
pub fn detect_all() -> Vec<ProviderSlot> {
    vec![
        ProviderSlot {
            id: "codex",
            label: "Codex",
            method: "ChatGPT sign-in",
            status: detect_codex(),
        },
        ProviderSlot {
            id: "claude",
            label: "Claude",
            method: "Claude sign-in",
            status: detect_claude(),
        },
        ProviderSlot {
            id: "cursor",
            label: "Cursor",
            method: "Cursor subscription",
            status: detect_cursor_cli(),
        },
        ProviderSlot {
            id: "antigravity",
            label: "Antigravity",
            method: "Google sign-in",
            status: detect_antigravity(),
        },
        ProviderSlot {
            id: "byok",
            label: "API key",
            method: "Bring your own key",
            status: detect_byok(),
        },
    ]
}

/// Codex readiness for the picker row: a Zest-held ChatGPT session **or**
/// the Codex CLI's own `auth.json`. Detection never extracts tokens.
pub fn detect_codex() -> AuthStatus {
    match detect_codex_oauth("codex") {
        AuthStatus::Ready { account } => return AuthStatus::Ready { account },
        AuthStatus::Unknown { reason } => return AuthStatus::Unknown { reason },
        _ => {}
    }
    let cli = detect_codex_cli();
    if matches!(cli, AuthStatus::Ready { .. } | AuthStatus::Unknown { .. }) {
        return cli;
    }
    if codex_cli_on_path() {
        cli
    } else {
        AuthStatus::NotLoggedIn {
            fix: "Connect with ChatGPT".into(),
        }
    }
}

/// Whether the credential manager (or `ZEST_CODEX_OAUTH_SESSION`) holds a
/// Zest ChatGPT session. Presence only — the JSON is not returned.
pub fn detect_codex_oauth(account: &str) -> AuthStatus {
    match crate::codex_oauth::session_present(account) {
        Ok(true) => AuthStatus::Ready { account: None },
        Ok(false) => AuthStatus::NotLoggedIn {
            fix: "Connect with ChatGPT".into(),
        },
        Err(_) => AuthStatus::Unknown {
            reason: "Zest could not verify this sign-in.".into(),
        },
    }
}

/// `codex` / `codex.exe` / `codex.cmd` on PATH. Does not execute the binary.
pub fn codex_cli_on_path() -> bool {
    command_on_path("codex")
}

fn command_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path) {
        if dir.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                if dir.join(format!("{name}.{ext}")).is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// Readiness for the native Codex CLI: whether its own `auth.json` holds a
/// usable session.
pub fn detect_codex_cli() -> AuthStatus {
    let home = match std::env::var("CODEX_HOME") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => match home_dir() {
            Some(h) => h.join(".codex"),
            None => {
                return AuthStatus::Unknown {
                    reason: "no home directory".into(),
                }
            }
        },
    };

    match well_formed_json(&home.join("auth.json")) {
        Some(true) => AuthStatus::Ready { account: None },
        Some(false) => AuthStatus::Unknown {
            reason: "auth.json is present but unreadable".into(),
        },
        None => AuthStatus::NotLoggedIn {
            fix: "codex login".into(),
        },
    }
}

/// Claude readiness for Zest's default path: the Claude Code CLI's own store.
///
/// Kept as a distinct name from [`detect_claude_code`] because the picker asks
/// about the *account* while the provider asks about the runtime. They report
/// the same thing now that the only Claude path is the CLI. A credentials file
/// is still not a working session — the desktop probes before opening a chat.
pub fn detect_claude() -> AuthStatus {
    detect_claude_code()
}

/// Readiness for the Claude Code CLI itself. A Claude Code parent provider must
/// use the subscription session owned by the `claude` executable it will spawn.
pub fn detect_claude_code() -> AuthStatus {
    let Some(dir) = home_dir().map(|h| h.join(".claude")) else {
        return AuthStatus::Unknown {
            reason: "no home directory".into(),
        };
    };

    if !dir.exists() {
        return AuthStatus::NotLoggedIn {
            fix: "claude login".into(),
        };
    }

    // Only trust an explicit credentials file. Its absence means "installed, but
    // credentials live somewhere we can't see" — not "logged out".
    match well_formed_json(&dir.join(".credentials.json")) {
        Some(true) => AuthStatus::Ready { account: None },
        _ => AuthStatus::Unknown {
            reason: "Zest could not verify this sign-in.".into(),
        },
    }
}

/// Readiness for the Cursor CLI, which signs in with `cursor-agent login`.
///
/// `~/.cursor/cli-config.json` is settings, not a credential store: the tokens
/// live elsewhere and this file only names who is signed in. That is exactly
/// what makes it safe to read — `authInfo.email` is a display label, and no
/// part of it is a secret. Its absence is the honest "logged out".
pub fn detect_cursor_cli() -> AuthStatus {
    let Some(dir) = home_dir().map(|h| h.join(".cursor")) else {
        return AuthStatus::Unknown {
            reason: "no home directory".into(),
        };
    };

    let config = dir.join("cli-config.json");
    if !config.is_file() {
        return AuthStatus::NotLoggedIn {
            fix: "cursor-agent login".into(),
        };
    }
    let Ok(raw) = std::fs::read_to_string(&config) else {
        return AuthStatus::Unknown {
            reason: "cli-config.json is present but unreadable".into(),
        };
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return AuthStatus::Unknown {
            reason: "cli-config.json is present but not JSON".into(),
        };
    };
    match value
        .get("authInfo")
        .and_then(|info| info.get("email"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|email| !email.is_empty())
    {
        Some(email) => AuthStatus::Ready {
            account: Some(email.to_string()),
        },
        None => AuthStatus::NotLoggedIn {
            fix: "cursor-agent login".into(),
        },
    }
}

/// Antigravity keeps a data directory under `~/.gemini/antigravity`. The Gemini
/// CLI writes `~/.gemini/oauth_creds.json`; Antigravity itself does not, so a
/// present data directory alone is not proof of a session.
pub fn detect_antigravity() -> AuthStatus {
    let Some(gemini) = home_dir().map(|h| h.join(".gemini")) else {
        return AuthStatus::Unknown {
            reason: "no home directory".into(),
        };
    };

    if let Some(true) = well_formed_json(&gemini.join("oauth_creds.json")) {
        return AuthStatus::Ready { account: None };
    }

    if gemini.join("antigravity").is_dir() {
        return AuthStatus::Unknown {
            reason: "Zest could not verify this sign-in.".into(),
        };
    }

    AuthStatus::NotLoggedIn {
        fix: "sign in to Antigravity".into(),
    }
}

/// Environment variable names that count as a bring-your-own key.
///
/// Presence only — values are never read here. `DEEPSEEK_API_KEY` belongs on
/// this list because the shipped OpenAI-compatible DeepSeek entry is configured
/// from that variable, and omitting it made `zest auth` report no key when the
/// runtime could already serve DeepSeek.
pub const BYOK_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "DEEPSEEK_API_KEY",
];

/// A key in the environment. Deliberately checks presence only — the value is
/// never inspected, compared, or reported.
pub fn detect_byok() -> AuthStatus {
    let present = BYOK_ENV_VARS.iter().any(|k| {
        std::env::var(k)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    });

    if present {
        AuthStatus::Ready { account: None }
    } else {
        AuthStatus::Unconfigured
    }
}

/// Whether Connect can launch a login for this provider.
pub fn can_start_login(provider_id: &str) -> bool {
    resolve_login(provider_id).is_some()
}

/// Resolve what Connect should spawn: each vendor's own CLI login flow.
pub fn resolve_login(provider_id: &str) -> Option<LoginSpawn> {
    match provider_id {
        "codex" => Some(LoginSpawn {
            program: PathBuf::from("codex"),
            args: vec!["login".into()],
            browser_title: "Sign in with ChatGPT",
            browser_body: CLI_LOGIN_BODY,
        }),
        "claude" => Some(LoginSpawn {
            program: PathBuf::from("claude"),
            args: vec!["auth".into(), "login".into()],
            browser_title: "Sign in with Claude",
            browser_body: CLI_LOGIN_BODY,
        }),
        _ => None,
    }
}

/// Resolve the direct Claude Code CLI login used by the first-class parent
/// provider: it must authenticate the same subscription session that the
/// `claude` executable will later use.
pub fn resolve_claude_code_login() -> LoginSpawn {
    LoginSpawn {
        program: PathBuf::from("claude"),
        args: vec!["auth".into(), "login".into()],
        browser_title: "Sign in with Claude",
        browser_body: CLI_LOGIN_BODY,
    }
}

/// Resolve the direct Codex CLI login used by the native app-server provider.
pub fn resolve_codex_cli_login() -> LoginSpawn {
    LoginSpawn {
        program: PathBuf::from("codex"),
        args: vec!["login".into()],
        browser_title: "Sign in with ChatGPT",
        browser_body: CLI_LOGIN_BODY,
    }
}

/// Spawn the vendor CLI login in its own console. Credentials stay with the
/// vendor. Zest only starts the process and later re-detects whether a store
/// appeared.
pub fn start_login(provider_id: &str) -> std::result::Result<LoginProcess, String> {
    if provider_id == "codex" && !codex_callback_port_available() {
        return Err(format!(
            "Codex sign-in cannot start because localhost:{CODEX_OAUTH_CALLBACK_PORT} is already in use. Close any other Codex/Zest sign-in window and try again."
        ));
    }

    let spawn = resolve_login(provider_id).ok_or_else(|| match provider_id {
        "antigravity" => {
            "Antigravity has no CLI login Zest can launch — sign in from the Antigravity app".into()
        }
        "byok" => "API key providers are configured via environment variables, not a login".into(),
        other => format!("no login command for provider `{other}`"),
    })?;

    start_cli_login(spawn)
}

/// Start the direct Claude Code subscription login without routing through a
/// gateway-owned authentication store.
pub fn start_claude_code_login() -> std::result::Result<LoginProcess, String> {
    start_cli_login(resolve_claude_code_login())
}

/// Start the direct Codex CLI subscription login without using a gateway store.
pub fn start_codex_cli_login() -> std::result::Result<LoginProcess, String> {
    if !codex_callback_port_available() {
        return Err(format!(
            "Codex sign-in cannot start because localhost:{CODEX_OAUTH_CALLBACK_PORT} is already in use. Close any other Codex/Zest sign-in window and try again."
        ));
    }
    start_cli_login(resolve_codex_cli_login())
}

/// Start an in-process ChatGPT sign-in and store the session under `account`.
pub fn start_codex_oauth_login(account: &str) -> std::result::Result<LoginProcess, String> {
    if !codex_callback_port_available() {
        return Err("Another ChatGPT sign-in is already open. Close it and try again.".into());
    }
    let spawn = resolve_codex_cli_login();
    let oauth = crate::codex_oauth::start_login(account)?;
    Ok(LoginProcess {
        spawn,
        child: None,
        oauth: Some(oauth),
        store: None,
    })
}

fn codex_callback_port_available() -> bool {
    TcpListener::bind(("127.0.0.1", CODEX_OAUTH_CALLBACK_PORT)).is_ok()
}

/// Resolve a bare CLI name the same way provider workers do, so Windows
/// `CreateProcessW` is not asked to spawn `claude` when only `claude.exe` or
/// `claude.cmd` exists.
fn resolve_login_program(program: &Path) -> PathBuf {
    match program.to_str() {
        Some(name) if program.components().count() == 1 => PathBuf::from(resolve_program(name)),
        _ => program.to_path_buf(),
    }
}

fn start_cli_login(mut spawn: LoginSpawn) -> std::result::Result<LoginProcess, String> {
    spawn.program = resolve_login_program(&spawn.program);
    let store = store_path_for_program(&spawn.program).map(snapshot_store);
    let child = spawn_login_cli(&spawn.program, &spawn.args).map_err(|e| {
        format!(
            "could not start `{} {}` — is it installed? ({e})",
            spawn.program.display(),
            spawn.args.join(" ")
        )
    })?;
    Ok(LoginProcess {
        spawn,
        child: Some(child),
        oauth: None,
        store,
    })
}

fn spawn_login_cli(program: &Path, args: &[String]) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    // Login helpers are not provider workers. They must not inherit the
    // serialized OAuth session used by Zest's in-process client.
    cmd.env_remove(crate::codex_oauth::SESSION_ENV);

    // Do not redirect stdio. CREATE_NEW_CONSOLE then attaches the child's
    // default handles to that console. Nulled handles plus CREATE_NO_WINDOW
    // made `claude login` a zombie: no browser, no prompt, Zest waiting forever.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(LOGIN_CREATION_FLAGS);
    }

    cmd.spawn()
}

/// Metadata of a vendor credential file. The file itself is never opened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreSnapshot {
    path: PathBuf,
    exists: bool,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn snapshot_store(path: PathBuf) -> StoreSnapshot {
    match std::fs::metadata(&path) {
        Ok(meta) => StoreSnapshot {
            path,
            exists: true,
            len: meta.len(),
            modified: meta.modified().ok(),
        },
        Err(_) => StoreSnapshot {
            path,
            exists: false,
            len: 0,
            modified: None,
        },
    }
}

fn store_rewritten(before: &StoreSnapshot) -> bool {
    let now = snapshot_store(before.path.clone());
    now.exists && (!before.exists || now.len != before.len || now.modified != before.modified)
}

fn store_path_for_program(program: &Path) -> Option<PathBuf> {
    let name = program.file_stem()?.to_str()?.to_ascii_lowercase();
    match name.as_str() {
        "claude" => home_dir().map(|home| home.join(".claude").join(".credentials.json")),
        "codex" => {
            let home = match std::env::var("CODEX_HOME") {
                Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
                _ => home_dir()?.join(".codex"),
            };
            Some(home.join("auth.json"))
        }
        _ => None,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// `Some(true)` = present and parses as JSON, `Some(false)` = present but not,
/// `None` = absent.
///
/// The parsed value is dropped immediately. Nothing inside a credential file is
/// read out, and the file's contents never leave this function.
fn well_formed_json(path: &Path) -> Option<bool> {
    if !path.is_file() {
        return None;
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Some(false);
    };
    Some(serde_json::from_str::<serde_json::Value>(&raw).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_selectable_but_logged_out_is_not() {
        // The whole point: "can't tell" must not be rendered as "logged out".
        assert!(AuthStatus::Unknown {
            reason: "keychain".into()
        }
        .selectable());
        assert!(AuthStatus::Ready { account: None }.selectable());
        assert!(!AuthStatus::NotLoggedIn {
            fix: "codex login".into()
        }
        .selectable());
        assert!(!AuthStatus::Unconfigured.selectable());
    }

    #[test]
    fn missing_credential_file_is_absent_not_malformed() {
        assert_eq!(
            well_formed_json(Path::new("./definitely-not-here.json")),
            None
        );
    }

    #[test]
    fn detect_all_covers_every_provider_slot() {
        let slots = detect_all();
        let ids: Vec<_> = slots.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec!["codex", "claude", "cursor", "antigravity", "byok"]
        );
    }

    #[test]
    fn byok_includes_the_deepseek_environment_variable() {
        assert!(BYOK_ENV_VARS.contains(&"DEEPSEEK_API_KEY"));
    }

    #[test]
    fn resolve_login_covers_cli_backed_providers() {
        assert!(resolve_login("claude").is_some());
        assert!(resolve_login("codex").is_some());
        assert!(resolve_login("antigravity").is_none());
        assert!(resolve_login("byok").is_none());
    }

    #[test]
    fn start_login_rejects_providers_without_a_cli() {
        assert!(start_login("byok").is_err());
        assert!(start_login("antigravity").is_err());
    }

    #[test]
    fn can_start_login_for_codex_without_the_cli() {
        assert!(can_start_login("codex"));
    }

    #[test]
    fn codex_cli_on_path_sees_a_temp_directory_entry() {
        let dir = tempfile::tempdir().unwrap();
        let dummy = if cfg!(windows) {
            dir.path().join("codex.exe")
        } else {
            dir.path().join("codex")
        };
        std::fs::write(&dummy, []).unwrap();
        let original = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref orig) = original {
            paths.extend(std::env::split_paths(orig));
        }
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let found = command_on_path("codex");
        match original {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        assert!(found, "PATH probe must see a dummy Codex CLI entry");
    }

    #[test]
    fn a_successful_cli_login_exit_is_success() {
        assert_eq!(
            login_poll_from_cli_exit(exit_status(true)),
            LoginPoll::Succeeded
        );
        assert!(matches!(
            login_poll_from_cli_exit(exit_status(false)),
            LoginPoll::Failed(_)
        ));
    }

    #[test]
    fn cli_login_copy_mentions_the_sign_in_window() {
        assert_eq!(resolve_claude_code_login().browser_body, CLI_LOGIN_BODY);
        assert_eq!(resolve_codex_cli_login().browser_body, CLI_LOGIN_BODY);
        assert!(CLI_LOGIN_BODY.contains("sign-in window"));
    }

    #[test]
    fn claude_connect_runs_auth_login_not_the_interactive_cli() {
        assert_eq!(
            resolve_claude_code_login().args,
            vec!["auth".to_string(), "login".to_string()]
        );
        assert_eq!(
            resolve_login("claude").map(|spawn| spawn.args),
            Some(vec!["auth".into(), "login".into()])
        );
    }

    #[test]
    fn a_rewritten_store_counts_as_login_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, "{\"before\":true}").unwrap();
        let before = snapshot_store(path.clone());
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "{\"after\":true}").unwrap();
        assert!(store_rewritten(&before));
    }

    #[test]
    fn an_untouched_store_is_not_login_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, "{\"same\":true}").unwrap();
        let before = snapshot_store(path);
        assert!(!store_rewritten(&before));
    }

    #[test]
    fn a_store_that_appears_counts_as_login_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        let before = snapshot_store(path.clone());
        assert!(!before.exists);
        std::fs::write(&path, "{}").unwrap();
        assert!(store_rewritten(&before));
    }

    #[test]
    fn a_bare_login_name_is_resolved_to_a_claude_binary() {
        let resolved = resolve_login_program(Path::new("claude"));
        let file = resolved
            .file_name()
            .and_then(|name| name.to_str())
            .expect("login program has a file name");
        assert!(
            file.eq_ignore_ascii_case("claude")
                || file.eq_ignore_ascii_case("claude.exe")
                || file.eq_ignore_ascii_case("claude.cmd")
                || file.eq_ignore_ascii_case("claude.bat"),
            "resolved {resolved:?}"
        );
    }

    #[test]
    fn an_explicit_login_path_is_kept() {
        let path = PathBuf::from("C:\\already\\resolved\\claude.exe");
        assert_eq!(resolve_login_program(&path), path);
    }

    #[test]
    fn claude_login_watches_the_cli_credentials_file() {
        let path = store_path_for_program(Path::new("claude.exe")).expect("claude store");
        assert!(path.ends_with(".credentials.json"));
    }

    #[cfg(windows)]
    #[test]
    fn cli_login_opens_a_console_instead_of_hiding_it() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        assert_eq!(LOGIN_CREATION_FLAGS, 0x0000_0010);
        assert_ne!(LOGIN_CREATION_FLAGS, CREATE_NO_WINDOW);
    }

    fn exit_status(ok: bool) -> std::process::ExitStatus {
        let code = if ok { "0" } else { "1" };
        if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", &format!("exit {code}")])
                .status()
                .expect("cmd exit")
        } else {
            std::process::Command::new("sh")
                .args(["-c", &format!("exit {code}")])
                .status()
                .expect("sh exit")
        }
    }
}
