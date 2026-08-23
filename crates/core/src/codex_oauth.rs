//! ChatGPT Codex session helpers used when the Codex CLI is not installed.
//!
//! Tokens are stored in the OS credential manager as one JSON object. This
//! module never reads `~/.codex/auth.json`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::CODEX_OAUTH_CALLBACK_PORT;
use crate::credentials;

pub const SESSION_ENV: &str = "ZEST_CODEX_OAUTH_SESSION";
pub const ORIGINATOR: &str = "zest";
pub const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const BACKEND_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// Public Codex CLI OAuth client id (not a secret).
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexOAuthSession {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    pub account_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOAuthPoll {
    Running,
    Succeeded,
    Failed(String),
}

pub struct CodexOAuthLogin {
    account: String,
    state: Arc<Mutex<CodexOAuthPoll>>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

impl CodexOAuthSession {
    pub fn parse_json(raw: &str) -> Result<Self, String> {
        serde_json::from_str(raw).map_err(|error| format!("invalid ChatGPT session: {error}"))
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("could not encode ChatGPT session: {error}"))
    }

    pub fn needs_refresh(&self, now: i64) -> bool {
        self.expires_at > 0 && self.expires_at <= now + 60
    }
}

pub fn default_account(provider_id: &str) -> &str {
    if provider_id.trim().is_empty() {
        "codex"
    } else {
        provider_id
    }
}

pub fn session_present(account: &str) -> Result<bool, String> {
    if credentials::present(account)? {
        return Ok(true);
    }
    Ok(std::env::var(SESSION_ENV)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false))
}

pub fn load_session(account: &str) -> Result<Option<CodexOAuthSession>, String> {
    if let Some(raw) = credentials::get(account)? {
        return Ok(Some(CodexOAuthSession::parse_json(&raw)?));
    }
    match std::env::var(SESSION_ENV) {
        Ok(raw) if !raw.trim().is_empty() => Ok(Some(CodexOAuthSession::parse_json(&raw)?)),
        _ => Ok(None),
    }
}

pub fn store_session(account: &str, session: &CodexOAuthSession) -> Result<(), String> {
    credentials::set(account, &session.to_json()?)
}

pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn random_pkce_pair() -> Result<(String, String), String> {
    let mut verifier_bytes = [0u8; 48];
    getrandom::fill(&mut verifier_bytes).map_err(|error| error.to_string())?;
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = pkce_challenge(&verifier);
    Ok((verifier, challenge))
}

pub fn random_state() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn authorization_url(challenge: &str, state: &str) -> String {
    format!(
        "{AUTH_URL}?response_type=code&client_id={CLIENT_ID}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true",
        urlencoding(REDIRECT_URI),
        urlencoding("openid profile email offline_access"),
        urlencoding(challenge),
        urlencoding(state),
    )
}

pub fn extract_account_id(id_token: &str) -> Result<String, String> {
    let payload = jwt_payload(id_token)?;
    payload
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "ChatGPT authorization did not include an account id".into())
}

pub fn parse_callback(target: &str, expected_state: &str) -> Result<String, String> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/auth/callback" {
        return Err("invalid callback".into());
    }
    let mut code = None;
    let mut state = None;
    let mut error = false;
    for part in query.split('&') {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        match key {
            "code" => code = Some(form_decode(value)),
            "state" => state = Some(form_decode(value)),
            "error" => error = true,
            _ => {}
        }
    }
    if error {
        return Err("ChatGPT authorization failed.".into());
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() && state == expected_state => Ok(code),
        _ => Err("ChatGPT authorization failed or returned an invalid callback.".into()),
    }
}

pub fn open_https_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" {
        return Err("only https URLs can be opened".into());
    }
    open_url(url)
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub fn start_login(account: &str) -> Result<CodexOAuthLogin, String> {
    let listener = TcpListener::bind(("127.0.0.1", CODEX_OAUTH_CALLBACK_PORT)).map_err(|_| {
        "Another ChatGPT sign-in is already open. Close it and try again.".to_string()
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let (verifier, challenge) = random_pkce_pair()?;
    let state = random_state()?;
    let url = authorization_url(&challenge, &state);
    if let Err(error) = open_https_url(&url) {
        eprintln!("Could not open a browser for ChatGPT sign-in. ({error})");
    }
    let login = CodexOAuthLogin {
        account: account.to_string(),
        state: Arc::new(Mutex::new(CodexOAuthPoll::Running)),
    };
    let reported = login.state.clone();
    let account = account.to_string();
    thread::spawn(move || {
        let outcome = complete_login(listener, &verifier, &state, &account);
        if let Ok(mut guard) = reported.lock() {
            *guard = match outcome {
                Ok(()) => CodexOAuthPoll::Succeeded,
                Err(detail) => CodexOAuthPoll::Failed(detail),
            };
        }
    });
    Ok(login)
}

impl CodexOAuthLogin {
    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn poll(&self) -> CodexOAuthPoll {
        self.state
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| CodexOAuthPoll::Failed("sign-in state lock poisoned".into()))
    }

    pub fn cancel(&self) {
        if let Ok(mut guard) = self.state.lock() {
            if matches!(*guard, CodexOAuthPoll::Running) {
                *guard = CodexOAuthPoll::Failed("Authentication cancelled.".into());
            }
        }
    }
}

fn complete_login(
    listener: TcpListener,
    verifier: &str,
    state: &str,
    account: &str,
) -> Result<(), String> {
    let deadline = SystemTime::now() + CALLBACK_TIMEOUT;
    let code = loop {
        if SystemTime::now() > deadline {
            return Err("Timed out waiting for ChatGPT authorization.".into());
        }
        match listener.accept() {
            Ok((stream, _)) => break read_callback(stream, state)?,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let session = runtime.block_on(exchange_code(verifier, &code))?;
    store_session(account, &session)
}

fn read_callback(mut stream: TcpStream, expected_state: &str) -> Result<String, String> {
    let mut buf = [0u8; 8192];
    let read = stream.read(&mut buf).map_err(|error| error.to_string())?;
    let request = String::from_utf8_lossy(&buf[..read]);
    let line = request.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let parsed = if method == "GET" {
        parse_callback(target, expected_state)
    } else {
        Err("invalid callback".into())
    };
    let ok = parsed.is_ok();
    let body = if ok {
        "Authentication complete. You can return to the application."
    } else {
        "Authentication failed. You can return to the application."
    };
    let _ = writeln!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        if ok { 200 } else { 400 },
        body.len()
    );
    parsed
}

pub async fn exchange_code(verifier: &str, code: &str) -> Result<CodexOAuthSession, String> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={CLIENT_ID}&code_verifier={}",
        urlencoding(code),
        urlencoding(REDIRECT_URI),
        urlencoding(verifier)
    );
    let json = token_request(&body, "application/x-www-form-urlencoded").await?;
    session_from_token(json, None, None, true)
}

pub async fn refresh_session(session: &CodexOAuthSession) -> Result<CodexOAuthSession, String> {
    let body = serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": session.refresh_token,
    })
    .to_string();
    let json = token_request(&body, "application/json").await?;
    session_from_token(
        json,
        Some(session.refresh_token.clone()),
        Some(session.account_id.clone()),
        false,
    )
}

pub async fn refresh_and_store(
    account: &str,
    session: CodexOAuthSession,
) -> Result<CodexOAuthSession, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    if !session.needs_refresh(now) {
        return Ok(session);
    }
    let refreshed = refresh_session(&session).await?;
    store_session(account, &refreshed)?;
    Ok(refreshed)
}

async fn token_request(body: &str, content_type: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(TOKEN_URL)
        .header("content-type", content_type)
        .header("accept", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err("Could not exchange the ChatGPT authorization code.".into());
    }
    response
        .json()
        .await
        .map_err(|error| format!("ChatGPT authorization response was unreadable: {error}"))
}

fn session_from_token(
    json: serde_json::Value,
    preserve_refresh: Option<String>,
    previous_account: Option<String>,
    require_id: bool,
) -> Result<CodexOAuthSession, String> {
    let parsed: TokenResponse = serde_json::from_value(json).map_err(|_| {
        "ChatGPT authorization response did not include usable Codex account credentials."
            .to_string()
    })?;
    let refresh = parsed
        .refresh_token
        .or(preserve_refresh)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "ChatGPT authorization response did not include usable Codex account credentials."
                .to_string()
        })?;
    let id_token = parsed.id_token.filter(|value| !value.is_empty());
    if require_id && id_token.is_none() {
        return Err(
            "ChatGPT authorization response did not include usable Codex account credentials."
                .into(),
        );
    }
    let account_id = match id_token.as_deref().map(extract_account_id).transpose()? {
        Some(account) => account,
        None => previous_account.ok_or_else(|| {
            "ChatGPT authorization response did not include usable Codex account credentials."
                .to_string()
        })?,
    };
    let expires_in = parsed.expires_in.filter(|value| *value > 0).unwrap_or(3600);
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
        + expires_in;
    Ok(CodexOAuthSession {
        access_token: parsed.access_token,
        refresh_token: refresh,
        id_token,
        account_id,
        expires_at,
    })
}

fn jwt_payload(token: &str) -> Result<serde_json::Value, String> {
    let mut parts = token.split('.');
    let _header = parts.next();
    let payload = parts
        .next()
        .ok_or_else(|| "id token is not a JWT".to_string())?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .map_err(|_| "id token payload is not valid".to_string())?;
    serde_json::from_slice(&decoded).map_err(|_| "id token payload is not valid".into())
}

fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_decode(value: &str) -> String {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &value[i + 1..i + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_contains_pkce_and_simplified_flow() {
        let url = authorization_url("challenge", "state-1");
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("state=state-1"));
    }

    #[test]
    fn pkce_challenge_is_s256_base64url() {
        // RFC 7636 appendix B.
        let challenge = pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn reads_chatgpt_account_id_from_id_token() {
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test"}}"#);
        let token = format!("aaa.{payload}.sig");
        assert_eq!(extract_account_id(&token).unwrap(), "acct_test");
    }

    #[test]
    fn session_json_round_trips() {
        let session = CodexOAuthSession {
            access_token: "a".into(),
            refresh_token: "r".into(),
            id_token: Some("i".into()),
            account_id: "acct".into(),
            expires_at: 10,
        };
        let parsed = CodexOAuthSession::parse_json(&session.to_json().unwrap()).unwrap();
        assert_eq!(parsed, session);
    }

    #[test]
    fn callback_rejects_wrong_state() {
        assert!(parse_callback("/auth/callback?code=abc&state=nope", "expected").is_err());
        assert_eq!(
            parse_callback("/auth/callback?code=abc&state=expected", "expected").unwrap(),
            "abc"
        );
        assert!(parse_callback("/other?code=abc&state=expected", "expected").is_err());
    }

    #[test]
    fn open_https_url_rejects_http() {
        assert!(open_https_url("http://example.com").is_err());
        assert!(open_https_url("file:///tmp/x").is_err());
    }
}
