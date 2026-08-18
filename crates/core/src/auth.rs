//! Detecting which providers are already signed in.
//!
//! Zest does **not** implement OAuth. Each vendor CLI (or local gateway) already
//! performs its own login and writes credentials to disk; Zest reads whether
//! that happened and nothing more. Implementing three vendor OAuth flows would
//! be the most fragile code in the project, and it would break without notice.
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
use std::process::{Child, Command, Stdio};

const CODEX_OAUTH_CALLBACK_PORT: u16 = 1455;

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
    child: Child,
}

impl LoginProcess {
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()?;
        let _ = self.child.wait();
        Ok(())
    }
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

/// Codex readiness for Zest's default path: the Codex CLI's own `auth.json`.
///
/// Kept as a distinct name from [`detect_codex_cli`] because the picker asks
/// about the *account* while the provider asks about the runtime. They report
/// the same thing now that the only Codex path is the CLI.
pub fn detect_codex() -> AuthStatus {
    detect_codex_cli()
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

/// A key in the environment. Deliberately checks presence only — the value is
/// never inspected, compared, or reported.
pub fn detect_byok() -> AuthStatus {
    let present = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"]
        .iter()
        .any(|k| {
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
            browser_body: "Finish in your browser. This window will update when you’re done.",
        }),
        "claude" => Some(LoginSpawn {
            program: PathBuf::from("claude"),
            args: vec!["login".into()],
            browser_title: "Sign in with Claude",
            browser_body: "Finish in your browser. This window will update when you’re done.",
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
        args: vec!["login".into()],
        browser_title: "Sign in with Claude",
        browser_body: "Finish in your browser. This window will update when you’re done.",
    }
}

/// Resolve the direct Codex CLI login used by the native app-server provider.
pub fn resolve_codex_cli_login() -> LoginSpawn {
    LoginSpawn {
        program: PathBuf::from("codex"),
        args: vec!["login".into()],
        browser_title: "Sign in with ChatGPT",
        browser_body: "Finish in your browser. This window will update when you’re done.",
    }
}

/// Spawn the vendor/gateway login flow with no console window. Credentials stay
/// with the vendor — Zest only starts the process and later re-detects whether
/// a store appeared.
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

    let child = spawn_silent(&spawn.program, &spawn.args).map_err(|e| {
        format!(
            "could not start `{} {}` — is it installed? ({e})",
            spawn.program.display(),
            spawn.args.join(" ")
        )
    })?;

    Ok(LoginProcess { spawn, child })
}

/// Start the direct Claude Code subscription login without routing through a
/// gateway-owned authentication store.
pub fn start_claude_code_login() -> std::result::Result<LoginProcess, String> {
    let spawn = resolve_claude_code_login();
    let child = spawn_silent(&spawn.program, &spawn.args)
        .map_err(|e| format!("could not start Claude Code login: {e}"))?;
    Ok(LoginProcess { spawn, child })
}

/// Start the direct Codex CLI subscription login without using a gateway store.
pub fn start_codex_cli_login() -> std::result::Result<LoginProcess, String> {
    if !codex_callback_port_available() {
        return Err(format!(
            "Codex sign-in cannot start because localhost:{CODEX_OAUTH_CALLBACK_PORT} is already in use. Close any other Codex/Zest sign-in window and try again."
        ));
    }
    let spawn = resolve_codex_cli_login();
    let child = spawn_silent(&spawn.program, &spawn.args)
        .map_err(|e| format!("could not start Codex CLI login: {e}"))?;
    Ok(LoginProcess { spawn, child })
}

fn codex_callback_port_available() -> bool {
    TcpListener::bind(("127.0.0.1", CODEX_OAUTH_CALLBACK_PORT)).is_ok()
}

fn spawn_silent(program: &Path, args: &[String]) -> std::io::Result<Child> {
    // Hide the console entirely on Windows so Connect feels like Zest, not a
    // terminal handoff. The system browser still opens for OAuth.
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    #[cfg(windows)]
    return spawn_with_flags(program, args, CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    spawn_with_flags(program, args, 0)
}

fn spawn_with_flags(program: &Path, args: &[String], flags: u32) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = flags;

    cmd.spawn()
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
        assert_eq!(ids, vec!["codex", "claude", "antigravity", "byok"]);
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
}
