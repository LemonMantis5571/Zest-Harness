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

/// Codex readiness for Zest's default path.
///
/// When a local CLIProxyAPI install is present, Ready means the gateway's own
/// credential store under `~/.cli-proxy-api` looks complete. This reports
/// credential state only; gateway process supervision is a selected-provider
/// decision made from the resolved configuration. Otherwise fall back to the
/// Codex CLI's `auth.json`.
pub fn detect_codex() -> AuthStatus {
    if cliproxy_exe().is_some() {
        return match gateway_auth_state("codex") {
            GatewayAuthState::Present => AuthStatus::Ready { account: None },
            GatewayAuthState::Incomplete => AuthStatus::NotLoggedIn {
                fix: "Connect in Zest (ChatGPT sign-in) — session file looks incomplete".into(),
            },
            GatewayAuthState::Absent => AuthStatus::NotLoggedIn {
                fix: "Connect in Zest (ChatGPT sign-in)".into(),
            },
        };
    }

    detect_codex_cli()
}

/// Readiness for the native Codex CLI, deliberately ignoring CLIProxyAPI.
/// Native app-server providers and gateway providers have separate auth
/// lifecycles even when both are called `codex` in the picker.
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

/// True when `~/.cli-proxy-api` has at least one well-formed JSON file.
///
/// Presence and parseability only — file contents are never returned or logged.
pub fn gateway_auth_present() -> bool {
    let Some(dir) = home_dir().map(|h| h.join(".cli-proxy-api")) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if well_formed_json(&path) == Some(true) {
            return true;
        }
    }
    false
}

/// Claude readiness for Zest's default path.
///
/// When a local CLIProxyAPI install is present, Ready means a Claude credential
/// file under `~/.cli-proxy-api` looks complete. Tiny stubs left after a failed
/// or cooled-down OAuth still parse as JSON and used to show "Signed in" while
/// every turn returned 503 `auth_unavailable`. Desktop probes before chat.
/// Otherwise fall back to the Claude Code CLI store.
pub fn detect_claude() -> AuthStatus {
    if cliproxy_exe().is_some() {
        return match gateway_auth_state("claude") {
            GatewayAuthState::Present => AuthStatus::Ready { account: None },
            GatewayAuthState::Incomplete => AuthStatus::NotLoggedIn {
                fix: "Connect in Zest (Claude sign-in) — session file looks incomplete".into(),
            },
            GatewayAuthState::Absent => AuthStatus::NotLoggedIn {
                fix: "Connect in Zest (Claude sign-in)".into(),
            },
        };
    }

    detect_claude_code()
}

/// Readiness for the Claude Code CLI itself, deliberately ignoring any local
/// CLIProxyAPI installation. A Claude Code parent provider must use the
/// subscription session owned by the CLI, not silently switch to a gateway.
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

/// Outcome of looking at gateway auth files for one provider prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayAuthState {
    Absent,
    /// JSON exists but is far too small to be a real OAuth blob (common after a
    /// failed Claude Connect that still wrote a stub).
    Incomplete,
    Present,
}

/// Reject empty/`{}` stubs. Claude gateway OAuth files are often ~400 bytes;
/// Codex ones are multi-KB. Size alone cannot prove the account works — desktop
/// still probes before chat — but a near-empty file is never a finished login.
const GATEWAY_AUTH_MIN_BYTES: u64 = 200;

fn gateway_auth_state(prefix: &str) -> GatewayAuthState {
    let Some(dir) = home_dir().map(|h| h.join(".cli-proxy-api")) else {
        return GatewayAuthState::Absent;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return GatewayAuthState::Absent;
    };
    let needle = format!("{prefix}-");
    let mut saw_incomplete = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.to_ascii_lowercase().starts_with(&needle) {
            continue;
        }
        if well_formed_json(&path) != Some(true) {
            continue;
        }
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if gateway_auth_file_looks_complete(len) {
            return GatewayAuthState::Present;
        }
        saw_incomplete = true;
    }
    if saw_incomplete {
        GatewayAuthState::Incomplete
    } else {
        GatewayAuthState::Absent
    }
}

fn gateway_auth_file_looks_complete(len: u64) -> bool {
    len >= GATEWAY_AUTH_MIN_BYTES
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

/// Shell command the vendor CLI expects for sign-in, as `"program", ["args"…]`.
///
/// For Codex this is the *fallback* (`codex login`). Prefer [`resolve_login`],
/// which may point at CLIProxyAPI instead.
pub fn login_command(provider_id: &str) -> Option<(&'static str, &'static [&'static str])> {
    match provider_id {
        "codex" => Some(("codex", &["login"])),
        "claude" => Some(("claude", &["login"])),
        "antigravity" | "byok" => None,
        _ => None,
    }
}

/// Resolve what Connect should spawn. Codex and Claude prefer a local CLIProxyAPI
/// binary when `tools/CLIProxyAPI` (or `ZEST_CLIPROXY_PATH`) is available.
pub fn resolve_login(provider_id: &str) -> Option<LoginSpawn> {
    match provider_id {
        "codex" => {
            if let Some(spawn) = cliproxy_login(
                "-codex-login",
                "Sign in with ChatGPT",
                "Finish in your browser. This window will update when you’re done.",
            ) {
                return Some(spawn);
            }
            Some(LoginSpawn {
                program: PathBuf::from("codex"),
                args: vec!["login".into()],
                browser_title: "Sign in with ChatGPT",
                browser_body: "Finish in your browser. This window will update when you’re done.",
            })
        }
        "claude" => {
            if let Some(spawn) = cliproxy_login(
                "-claude-login",
                "Sign in with Claude",
                "Finish in your browser. This window will update when you’re done.",
            ) {
                return Some(spawn);
            }
            Some(LoginSpawn {
                program: PathBuf::from("claude"),
                args: vec!["login".into()],
                browser_title: "Sign in with Claude",
                browser_body: "Finish in your browser. This window will update when you’re done.",
            })
        }
        _ => None,
    }
}

/// Resolve the direct Claude Code CLI login used by the first-class parent
/// provider. This deliberately bypasses CLIProxyAPI: the parent provider must
/// authenticate the same subscription session that the `claude` executable
/// will later use.
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

fn cliproxy_login(
    flag: &str,
    browser_title: &'static str,
    browser_body: &'static str,
) -> Option<LoginSpawn> {
    // Login is an explicit gateway operation. Discovering the bundled sidecar
    // here is allowed; provisioning and process startup remain turn-scoped.
    let _ = adopt_bundled_gateway();
    // Same resolver the serving process uses. Signing in through a different
    // config would write credentials to an `auth-dir` the gateway never reads.
    let (exe, config) = crate::gateway::runtime().ok().flatten()?;
    Some(LoginSpawn {
        program: exe,
        args: vec![
            "-config".into(),
            config.to_string_lossy().into_owned(),
            flag.into(),
        ],
        browser_title,
        browser_body,
    })
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

/// Start a managed long-lived process with no inherited console handles.
///
/// The returned child handle is intentionally retained by the gateway lease so
/// the front-end can terminate only a process it started. A detached console is
/// still useful on Windows, but detachment no longer means that the child
/// outlives Zest.
pub(crate) fn spawn_managed(program: &Path, args: &[String]) -> std::io::Result<Child> {
    // DETACHED_PROCESS: no console at all, rather than an invisible one.
    // Supersedes CREATE_NO_WINDOW, which Windows ignores when this is set.
    #[cfg(windows)]
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    #[cfg(windows)]
    return spawn_with_flags(program, args, DETACHED_PROCESS);
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

/// Where CLIProxyAPI is installed, as `(executable, config)`.
///
/// Only reports an install whose config sits beside the binary — a
/// hand-installed one. A bundled gateway has no writable directory next to it,
/// so its config comes from [`crate::gateway::provision`] instead; use
/// [`cliproxy_exe`] when the question is just "is a binary available".
pub fn cliproxy_install() -> Option<(PathBuf, PathBuf)> {
    find_cliproxy()
}

/// Locate the CLIProxyAPI **executable**, with or without a config beside it.
///
/// `ZEST_CLIPROXY_PATH` first — that is how the desktop points core at a bundled
/// sidecar — then walk up from cwd for a development checkout.
pub fn cliproxy_exe() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("ZEST_CLIPROXY_PATH") {
        let exe = PathBuf::from(raw.trim());
        if exe.is_file() {
            return Some(exe);
        }
    }

    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..8 {
        let exe = dir
            .join("tools")
            .join("CLIProxyAPI")
            .join(cliproxy_bin_name());
        if exe.is_file() {
            return Some(exe);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Adopt a bundled gateway placed beside the current Zest executable.
///
/// Desktop and CLI builds use the same portable layout. Keeping this resolver
/// in core prevents one front-end from finding the sidecar while the other
/// falls back to a development-only checkout search.
pub fn adopt_bundled_gateway() -> bool {
    if let Ok(raw) = std::env::var("ZEST_CLIPROXY_PATH") {
        if PathBuf::from(raw.trim()).is_file() {
            return true;
        }
    }

    // A hand-installed gateway still wins, as the README promises. Pointing the
    // variable at the bundled binary here would pre-empt the walk-up search and
    // quietly strand a tuned `tools/CLIProxyAPI` checkout — and, because the two
    // configs list different `api-keys`, hand-starting the one Zest no longer
    // uses is how `401 Invalid API key` happens.
    if find_cliproxy().is_some() {
        return true;
    }

    let Some(candidate) = bundled_gateway_candidates()
        .into_iter()
        .find(|p| p.is_file())
    else {
        return false;
    };
    std::env::set_var("ZEST_CLIPROXY_PATH", candidate);
    true
}

/// Where a bundled gateway might be, best first.
fn bundled_gateway_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Development: the sidecar *source*, deliberately not the copy that Tauri
    // places beside the executable. `tauri-build` rewrites that copy on every
    // build, and Windows refuses to overwrite a running `.exe` — so spawning it
    // makes the gateway lock the next rebuild with a PermissionDenied panic in
    // the build script. The source file is only ever read, never rewritten.
    #[cfg(debug_assertions)]
    if let Some(dir) = dev_sidecar_dir() {
        candidates.push(dir.join(format!(
            "cli-proxy-api-{}{}",
            current_target_triple(),
            std::env::consts::EXE_SUFFIX
        )));
    }

    // Installed: Tauri places the sidecar next to the main executable and strips
    // the target-triple suffix, so the bundled path is predictable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(cliproxy_bin_name()));
        }
    }

    candidates
}

/// `crates/desktop/binaries` in a development checkout, found by walking up from
/// the running binary rather than from cwd — the CLI and the desktop app are
/// launched from wherever the user happens to be.
#[cfg(debug_assertions)]
fn dev_sidecar_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..8 {
        let candidate = dir.join("crates").join("desktop").join("binaries");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// The target triple this build was compiled for, as the sidecar filename spells
/// it. `TAURI_ENV_TARGET_TRIPLE` is set by `tauri-build`; the CLI has no build
/// script, so it falls back to composing one from the compile-time target.
#[cfg(debug_assertions)]
fn current_target_triple() -> String {
    match option_env!("TAURI_ENV_TARGET_TRIPLE") {
        Some(triple) => triple.to_string(),
        None => format!(
            "{}-{}-{}",
            std::env::consts::ARCH,
            if cfg!(windows) {
                "pc"
            } else if cfg!(target_os = "macos") {
                "apple"
            } else {
                "unknown"
            },
            if cfg!(windows) {
                "windows-msvc"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux-gnu"
            }
        ),
    }
}

#[cfg(all(test, debug_assertions))]
mod bundled_gateway_tests {
    use super::*;

    /// The dev candidate must be the sidecar *source*, never the copy beside the
    /// running executable. Spawning that copy locks it, and `tauri-build`
    /// rewrites it on the next build — which failed with PermissionDenied.
    #[test]
    fn dev_prefers_the_sidecar_source_over_the_build_copy() {
        let Some(dir) = dev_sidecar_dir() else {
            eprintln!("not a development checkout - nothing to assert");
            return;
        };
        assert!(
            dir.ends_with(std::path::Path::new("crates/desktop/binaries")),
            "{dir:?}"
        );

        let candidates = bundled_gateway_candidates();
        let first = candidates.first().expect("a dev candidate");
        assert!(
            first.starts_with(&dir),
            "dev candidate should be the source: {first:?}"
        );

        // And it must not be whatever sits next to the test binary.
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(cliproxy_bin_name())));
        assert_ne!(Some(first.clone()), beside_exe);
    }

    #[test]
    fn the_dev_candidate_matches_the_name_fetch_gateway_writes() {
        let triple = current_target_triple();
        // scripts/fetch-gateway.ps1 writes `cli-proxy-api-<triple>[.exe]`.
        assert!(triple.contains('-'), "{triple}");
        if cfg!(windows) {
            assert!(triple.ends_with("-pc-windows-msvc"), "{triple}");
        }
    }
}

/// A hand-installed CLIProxyAPI: an executable with its own `config.yaml`.
fn find_cliproxy() -> Option<(PathBuf, PathBuf)> {
    let exe = cliproxy_exe()?;
    let config = exe.parent()?.join("config.yaml");
    config.is_file().then_some((exe, config))
}

fn cliproxy_bin_name() -> &'static str {
    if cfg!(windows) {
        "cli-proxy-api.exe"
    } else {
        "cli-proxy-api"
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
        assert_eq!(ids, vec!["codex", "claude", "antigravity", "byok"]);
    }

    #[test]
    fn login_command_covers_cli_backed_providers_only() {
        assert_eq!(login_command("codex"), Some(("codex", &["login"][..])));
        assert_eq!(login_command("claude"), Some(("claude", &["login"][..])));
        assert_eq!(login_command("antigravity"), None);
        assert_eq!(login_command("byok"), None);
        assert_eq!(login_command("unknown"), None);
    }

    #[test]
    fn resolve_login_covers_cli_backed_providers() {
        assert!(resolve_login("claude").is_some());
        assert!(resolve_login("codex").is_some());
        assert!(resolve_login("antigravity").is_none());
        assert!(resolve_login("byok").is_none());
    }

    #[test]
    fn gateway_auth_rejects_tiny_stub_files() {
        // Empty / near-empty stubs only — Claude OAuth files are often ~424 bytes.
        assert!(!gateway_auth_file_looks_complete(2));
        assert!(!gateway_auth_file_looks_complete(
            GATEWAY_AUTH_MIN_BYTES - 1
        ));
        assert!(gateway_auth_file_looks_complete(GATEWAY_AUTH_MIN_BYTES));
        assert!(gateway_auth_file_looks_complete(424));
        assert!(gateway_auth_file_looks_complete(4309));
    }

    #[test]
    fn start_login_rejects_providers_without_a_cli() {
        assert!(start_login("byok").is_err());
        assert!(start_login("antigravity").is_err());
    }

    #[test]
    fn gateway_auth_absent_dir_is_false() {
        // Home may or may not have a gateway store; the helper must not panic.
        let _ = gateway_auth_present();
    }
}
