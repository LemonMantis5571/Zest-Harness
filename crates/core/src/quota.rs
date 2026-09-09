//! Live provider balance checks.
//!
//! A usage ledger can only say what Zest sent. This module handles the small
//! set of supported provider checks that can say something more: Codex's local
//! app-server limits and DeepSeek's documented API balance endpoint. Providers
//! without a supported read-only check are returned as unavailable rather than
//! guessed.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::join_all;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::timeout;

use crate::codex_oauth::{refresh_and_store, ORIGINATOR, USAGE_URL};
use crate::config::{Config, ProviderConfig};
use crate::provider::driver::{credentials_for, resolve};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const CODEX_QUERY_TIMEOUT: Duration = Duration::from_secs(8);
const CODEX_COMMAND_ENV: &str = "ZEST_CODEX_COMMAND";
const CLAUDE_DESKTOP_MAX_CACHE_AGE_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaSnapshot {
    pub checked_at: u64,
    pub providers: Vec<ProviderQuotaView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaView {
    pub provider_id: String,
    pub kind: ProviderQuotaKind,
    pub detail: String,
    pub available: Option<bool>,
    pub balances: Vec<ProviderBalanceView>,
    #[serde(default)]
    pub windows: Vec<ProviderQuotaWindowView>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub spend_limit: Option<ProviderSpendLimitView>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderQuotaKind {
    Balance,
    RateLimit,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalanceView {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaWindowView {
    pub label: String,
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSpendLimitView {
    pub used: String,
    pub limit: String,
    pub remaining_percent: f64,
    pub resets_at: Option<u64>,
}

/// Query the official quota/balance endpoints that match the configured
/// provider. This never sends a request to an arbitrary configured gateway:
/// only DeepSeek's official host is eligible for the balance call.
pub async fn fetch_provider_quotas(config: &Config) -> ProviderQuotaSnapshot {
    let checked_at = now_secs();
    let (client, client_error) = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => (Some(client), None),
        Err(error) => (
            None,
            Some(format!("Could not start the balance check: {error}")),
        ),
    };

    // `join_all` polls every provider future together and returns values in
    // iterator order. The BTreeMap iteration order therefore remains the
    // public, deterministic order even though slow providers do not block
    // checks for the providers next to them.
    let providers = join_all(config.providers.iter().map(|(provider_id, provider)| {
        fetch_provider_quota(
            provider_id,
            provider,
            client.as_ref(),
            client_error.as_deref(),
        )
    }))
    .await;

    ProviderQuotaSnapshot {
        checked_at,
        providers,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexQuotaSource {
    AppServer,
    Wham,
}

fn quota_source(provider: &ProviderConfig) -> Option<CodexQuotaSource> {
    match provider {
        ProviderConfig::CodexCli { .. } => Some(CodexQuotaSource::AppServer),
        ProviderConfig::CodexOAuth { .. } => Some(CodexQuotaSource::Wham),
        _ => None,
    }
}

async fn fetch_provider_quota(
    provider_id: &str,
    provider: &ProviderConfig,
    client: Option<&reqwest::Client>,
    client_error: Option<&str>,
) -> ProviderQuotaView {
    match quota_source(provider) {
        Some(CodexQuotaSource::AppServer) => return fetch_codex_rate_limits(provider_id).await,
        Some(CodexQuotaSource::Wham) => {
            return fetch_codex_oauth_usage(provider_id, provider, client, client_error).await
        }
        None => {}
    }

    if is_claude_subscription_provider(provider) {
        return fetch_claude_desktop_quota(provider_id).await;
    }

    match provider {
        ProviderConfig::OpenaiCompatible {
            base_url,
            credential,
            api_key_env,
            ..
        } if deepseek_balance_url(base_url).is_some() => match client {
            Some(client) => {
                fetch_deepseek_balance(
                    client,
                    provider_id,
                    base_url,
                    credential.as_deref(),
                    api_key_env.as_deref(),
                )
                .await
            }
            None => error_view(
                provider_id,
                client_error.unwrap_or("Could not start the balance check."),
            ),
        },
        ProviderConfig::Anthropic { .. } => unavailable_view(
            provider_id,
            "Anthropic exposes rate limits after a request, not a plan balance here.",
        ),
        ProviderConfig::ClaudeCode { .. } => fetch_claude_desktop_quota(provider_id).await,
        ProviderConfig::CodexCli { .. } | ProviderConfig::CodexOAuth { .. } => {
            unreachable!("Codex quota is dispatched by kind before this match")
        }
        // Cursor reports plan usage on its dashboard, behind an authenticated
        // web session Zest does not hold. ACP carries no token accounting
        // either, so there is nothing here to read without inventing it.
        ProviderConfig::CursorAcp { .. } => unavailable_view(
            provider_id,
            "Cursor reports plan usage on its own dashboard, not over ACP.",
        ),
        ProviderConfig::OpenaiCompatible { .. } => {
            unavailable_view(provider_id, "This API has no standard balance endpoint.")
        }
    }
}

/// Only the Claude Code runtime spends the Claude.ai subscription allowance.
///
/// This used to also accept a provider *named* `claude` behind a gateway. It is
/// a pure kind decision now: the id is a label, not a capability.
fn is_claude_subscription_provider(provider: &ProviderConfig) -> bool {
    matches!(provider, ProviderConfig::ClaudeCode { .. })
}

/// Read Claude Desktop's last provider-reported usage sample.
///
/// Claude Desktop and Claude Code use the same Claude.ai account allowance.
/// Desktop keeps a small local history with the 5-hour and 7-day percentages.
/// This is deliberately a read-only cache adapter: it does not read OAuth
/// credentials or call Anthropic's private usage endpoints. The sample is
/// considered stale after a day so an old desktop value cannot look live.
async fn fetch_claude_desktop_quota(provider_id: &str) -> ProviderQuotaView {
    let Some(path) =
        dirs::config_dir().map(|dir| dir.join("Claude").join("plan-usage-history.json"))
    else {
        return unavailable_view(
            provider_id,
            "Claude Desktop usage data is not available on this system.",
        );
    };

    let raw = match tokio::fs::read(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return unavailable_view(
                provider_id,
                "Open Claude Desktop once to create its shared usage snapshot.",
            );
        }
        Err(_) => {
            return error_view(provider_id, "Claude Desktop usage data could not be read.");
        }
    };
    let history = match serde_json::from_slice::<ClaudeDesktopUsageHistory>(&raw) {
        Ok(history) => history,
        Err(_) => {
            return error_view(
                provider_id,
                "Claude Desktop usage data has an unknown format.",
            );
        }
    };
    claude_desktop_quota_view(provider_id, &history, now_secs())
}

fn claude_desktop_quota_view(
    provider_id: &str,
    history: &ClaudeDesktopUsageHistory,
    now: u64,
) -> ProviderQuotaView {
    let Some(sample) = history.samples.iter().max_by_key(|sample| sample.timestamp) else {
        return unavailable_view(
            provider_id,
            "Claude Desktop has not reported a usage snapshot yet.",
        );
    };
    let sample_secs = timestamp_to_secs(sample.timestamp);
    let age = now.saturating_sub(sample_secs);
    if age > CLAUDE_DESKTOP_MAX_CACHE_AGE_SECS {
        return unavailable_view(
            provider_id,
            format!(
                "Claude Desktop usage is stale (last update {} ago).",
                format_elapsed(age)
            ),
        );
    }

    let mut windows = Vec::new();
    if let Some(used_percent) = sample.usage.five_hour {
        windows.push(ProviderQuotaWindowView {
            label: "5-hour".into(),
            used_percent: clamp_percent(used_percent),
            window_minutes: Some(5 * 60),
            resets_at: None,
        });
    }
    if let Some(used_percent) = sample.usage.seven_day {
        windows.push(ProviderQuotaWindowView {
            label: "7-day".into(),
            used_percent: clamp_percent(used_percent),
            window_minutes: Some(7 * 24 * 60),
            resets_at: None,
        });
    }
    if windows.is_empty() {
        return unavailable_view(
            provider_id,
            "Claude Desktop has not reported an active usage window.",
        );
    }

    let available = windows.iter().all(|window| window.used_percent < 100.0);
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::RateLimit,
        detail: format!(
            "Shared with Claude Desktop and Claude Code; updated {} ago. Reset times are not in the desktop cache.",
            format_elapsed(age)
        ),
        available: Some(available),
        balances: Vec::new(),
        windows,
        plan: None,
        spend_limit: None,
    }
}

fn timestamp_to_secs(timestamp: u64) -> u64 {
    if timestamp >= 10_000_000_000 {
        timestamp / 1_000
    } else {
        timestamp
    }
}

fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h", seconds / (60 * 60))
    } else {
        format!("{}d", seconds / (24 * 60 * 60))
    }
}

async fn fetch_deepseek_balance(
    client: &reqwest::Client,
    provider_id: &str,
    base_url: &str,
    credential: Option<&str>,
    api_key_env: Option<&str>,
) -> ProviderQuotaView {
    let Some(url) = deepseek_balance_url(base_url) else {
        return unavailable_view(provider_id, "This is not the official DeepSeek API host.");
    };

    let key = match resolve_key(credential, api_key_env) {
        Ok(Some(key)) => key,
        Ok(None) => return unavailable_view(provider_id, "No API key is configured."),
        Err(error) => return error_view(provider_id, error),
    };

    let response = match client.get(url).bearer_auth(key).send().await {
        Ok(response) => response,
        Err(error) => {
            return error_view(provider_id, format!("Could not reach DeepSeek: {error}"));
        }
    };
    let status = response.status();
    if !status.is_success() {
        return error_view(
            provider_id,
            format!("DeepSeek returned HTTP {}.", status.as_u16()),
        );
    }

    let payload = match response.json::<DeepSeekBalanceResponse>().await {
        Ok(payload) => payload,
        Err(error) => {
            return error_view(
                provider_id,
                format!("DeepSeek returned an unreadable balance: {error}"),
            );
        }
    };

    let balances = payload
        .balance_infos
        .into_iter()
        .map(|balance| ProviderBalanceView {
            currency: balance.currency,
            total_balance: balance.total_balance,
            granted_balance: balance.granted_balance,
            topped_up_balance: balance.topped_up_balance,
        })
        .collect();
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::Balance,
        detail: if payload.is_available {
            "Balance reported by DeepSeek.".into()
        } else {
            "DeepSeek reports that this balance cannot be used right now.".into()
        },
        available: Some(payload.is_available),
        balances,
        windows: Vec::new(),
        plan: None,
        spend_limit: None,
    }
}

/// Ask the installed Codex CLI for the account limits it already knows about.
///
/// This uses the supported local app-server protocol instead of reading
/// `auth.json` or calling a private ChatGPT web endpoint. The CLI owns the
/// stored session and performs the authenticated request itself.
async fn fetch_codex_rate_limits(provider_id: &str) -> ProviderQuotaView {
    let command = std::env::var(CODEX_COMMAND_ENV).unwrap_or_else(|_| "codex".into());
    // Resolve against PATH/PATHEXT and the user's current PATH, the same way
    // the provider spawn does. Without this the CLI is invisible on Windows,
    // where npm installs it as a `.cmd` shim.
    let mut builder = Command::new(crate::tools::external_agent::resolve_program(&command));
    builder
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    crate::tools::external_agent::prepare_external_command(&mut builder);
    let mut child = match builder.spawn() {
        Ok(child) => child,
        Err(error) => {
            let detail = if command == "codex" && error.kind() == std::io::ErrorKind::NotFound {
                codex_cli_unavailable_detail()
            } else {
                format!("Codex app-server is not available: {error}")
            };
            return unavailable_view(provider_id, detail);
        }
    };

    let result = run_codex_rate_limit_request(&mut child).await;
    let _ = child.kill().await;

    match result {
        Ok(result) => codex_quota_view(provider_id, result),
        Err(detail) => unavailable_view(provider_id, detail),
    }
}

async fn fetch_codex_oauth_usage(
    provider_id: &str,
    provider: &ProviderConfig,
    client: Option<&reqwest::Client>,
    client_error: Option<&str>,
) -> ProviderQuotaView {
    let Some(client) = client else {
        return error_view(
            provider_id,
            client_error.unwrap_or("Could not start the usage check."),
        );
    };
    let request = credentials_for(provider_id, provider);
    let raw = match resolve(request) {
        Ok(Some(raw)) => raw,
        Ok(None) => return unavailable_view(provider_id, "Sign in again to refresh Codex limits."),
        Err(error) => return error_view(provider_id, error),
    };
    let session = match crate::codex_oauth::CodexOAuthSession::parse_json(&raw) {
        Ok(session) => session,
        Err(_) => return error_view(provider_id, "Sign in again to refresh Codex limits."),
    };
    let account = request
        .account
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(provider_id);
    let session = match refresh_and_store(account, session).await {
        Ok(session) => session,
        Err(_) => return error_view(provider_id, "Sign in again to refresh Codex limits."),
    };

    let response = match client
        .get(USAGE_URL)
        .header("authorization", format!("Bearer {}", session.access_token))
        .header("ChatGPT-Account-ID", &session.account_id)
        .header("accept", "application/json")
        .header("originator", ORIGINATOR)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return error_view(provider_id, format!("Could not read Codex limits: {error}"))
        }
    };
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    chatgpt_usage_from_http(provider_id, status, &body)
}

fn chatgpt_usage_from_http(provider_id: &str, status: u16, body: &str) -> ProviderQuotaView {
    if status == 401 || status == 403 {
        return error_view(provider_id, "Sign in again to refresh Codex limits.");
    }
    if !(200..300).contains(&status) {
        return unavailable_view(provider_id, format!("Codex usage check failed ({status})."));
    }
    match serde_json::from_str::<ChatgptWhamUsage>(body) {
        Ok(parsed) => chatgpt_quota_view(provider_id, parsed),
        Err(_) => unavailable_view(provider_id, "Codex returned an unreadable usage report."),
    }
}

fn chatgpt_quota_view(provider_id: &str, usage: ChatgptWhamUsage) -> ProviderQuotaView {
    let Some(rate_limit) = usage.rate_limit else {
        return unavailable_view(provider_id, "Codex did not report account limits.");
    };
    let mut windows = Vec::new();
    for (label, window) in [
        ("Primary", rate_limit.primary_window),
        ("Secondary", rate_limit.secondary_window),
    ] {
        if let Some(window) = window {
            match chatgpt_window_view(label, window) {
                Some(view) => windows.push(view),
                None => {
                    return unavailable_view(
                        provider_id,
                        "Codex returned an unreadable usage report.",
                    )
                }
            }
        }
    }
    if windows.is_empty() {
        return unavailable_view(provider_id, "Codex did not report an active quota window.");
    }
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::RateLimit,
        detail: "Limits reported by ChatGPT.".to_string(),
        available: Some(!rate_limit.limit_reached.unwrap_or(false)),
        balances: Vec::new(),
        windows,
        plan: usage.plan_type.filter(|value| !value.is_empty()),
        spend_limit: None,
    }
}

fn chatgpt_window_view(label: &str, window: ChatgptWhamWindow) -> Option<ProviderQuotaWindowView> {
    let used_percent = window.used_percent?;
    let window_minutes = window.limit_window_seconds.map(|seconds| seconds / 60);
    Some(ProviderQuotaWindowView {
        label: format!(
            "{label}{}",
            window_minutes
                .map(|minutes| format!(" ({})", format_window_duration(minutes)))
                .unwrap_or_default()
        ),
        used_percent,
        window_minutes,
        resets_at: window.reset_at.and_then(non_negative_timestamp),
    })
}

fn codex_cli_unavailable_detail() -> String {
    if cfg!(target_os = "macos") {
        "Codex CLI is not installed. The ChatGPT macOS app does not expose codex app-server to Zest; install the Codex CLI to read account limits.".into()
    } else {
        "Codex CLI is not installed or is not available in PATH. Install it to read account limits."
            .into()
    }
}

async fn run_codex_rate_limit_request(child: &mut Child) -> Result<Value, String> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server did not open its input.".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server did not open its output.".to_string())?;

    for message in [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "zest",
                    "title": "Zest",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        json!({"method": "initialized"}),
        json!({"method": "account/rateLimits/read", "id": 2}),
    ] {
        let mut line = serde_json::to_vec(&message)
            .map_err(|error| format!("Could not prepare the Codex quota check: {error}"))?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|error| format!("Could not contact Codex app-server: {error}"))?;
    }
    stdin
        .flush()
        .await
        .map_err(|error| format!("Could not start the Codex quota check: {error}"))?;

    let lines = BufReader::new(stdout).lines();
    let response = timeout(CODEX_QUERY_TIMEOUT, read_codex_response(lines))
        .await
        .map_err(|_| "Codex quota check timed out.".to_string())??;
    if let Some(error) = response.get("error") {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex did not return account limits.");
        return Err(codex_error_detail(detail));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex did not return account limits.".to_string())
}

async fn read_codex_response(mut lines: Lines<BufReader<ChildStdout>>) -> Result<Value, String> {
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Could not read Codex quota data: {error}"))?
    {
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("id").and_then(Value::as_i64) == Some(2) {
            return Ok(value);
        }
    }
    Err("Codex app-server closed before returning account limits.".to_string())
}

fn codex_quota_view(provider_id: &str, value: Value) -> ProviderQuotaView {
    let response = match serde_json::from_value::<CodexRateLimitResponse>(value) {
        Ok(response) => response,
        Err(error) => {
            return error_view(
                provider_id,
                format!("Codex returned an unreadable quota: {error}"),
            )
        }
    };
    let Some(rate_limits) = response.rate_limits.or_else(|| {
        response
            .rate_limits_by_limit_id
            .as_ref()
            .and_then(|limits| limits.values().next().cloned())
    }) else {
        return unavailable_view(provider_id, "Codex did not report account limits.");
    };

    let mut windows = Vec::new();
    if let Some(window) = rate_limits.primary {
        windows.push(codex_window_view("Primary", window));
    }
    if let Some(window) = rate_limits.secondary {
        windows.push(codex_window_view("Secondary", window));
    }
    let spend_limit = rate_limits
        .individual_limit
        .map(|limit| ProviderSpendLimitView {
            used: limit.used,
            limit: limit.limit,
            remaining_percent: limit.remaining_percent,
            resets_at: non_negative_timestamp(limit.resets_at),
        });
    if windows.is_empty() && spend_limit.is_none() {
        return unavailable_view(provider_id, "Codex did not report an active quota window.");
    }

    let detail = rate_limits
        .rate_limit_reached_type
        .filter(|value| !value.is_empty())
        .map(|value| format!("Codex reports: {value}."))
        .unwrap_or_else(|| "Limits reported by Codex.".to_string());
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::RateLimit,
        detail,
        available: Some(rate_limits.spend_control_reached != Some(true)),
        balances: Vec::new(),
        windows,
        plan: rate_limits.plan_type,
        spend_limit,
    }
}

fn codex_window_view(label: &str, window: CodexRateLimitWindow) -> ProviderQuotaWindowView {
    ProviderQuotaWindowView {
        label: format!(
            "{label}{}",
            window
                .window_duration_mins
                .map(|minutes| format!(" ({})", format_window_duration(minutes)))
                .unwrap_or_default()
        ),
        used_percent: window.used_percent,
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at.and_then(non_negative_timestamp),
    }
}

fn format_window_duration(minutes: u64) -> String {
    if minutes.is_multiple_of(24 * 60) {
        format!("{}d", minutes / (24 * 60))
    } else if minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

fn non_negative_timestamp(value: i64) -> Option<u64> {
    (value >= 0).then_some(value as u64)
}

fn codex_error_detail(message: &str) -> String {
    if message.to_ascii_lowercase().contains("authentication") {
        "Sign in to Codex to see account limits.".to_string()
    } else {
        "Codex could not read account limits.".to_string()
    }
}

fn resolve_key(
    credential: Option<&str>,
    api_key_env: Option<&str>,
) -> Result<Option<String>, String> {
    let stored = credential
        .filter(|account| !account.trim().is_empty())
        .map(crate::credentials::get)
        .transpose()
        .map_err(|error| format!("Could not read the saved API key: {error}"))?
        .flatten();
    Ok(stored.or_else(|| {
        api_key_env
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
    }))
}

fn deepseek_balance_url(base_url: &str) -> Option<Url> {
    let mut url = Url::parse(base_url).ok()?;
    if url.scheme() != "https" || !url.host_str()?.eq_ignore_ascii_case("api.deepseek.com") {
        return None;
    }
    url.set_path("/user/balance");
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

fn unavailable_view(provider_id: &str, detail: impl Into<String>) -> ProviderQuotaView {
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::Unavailable,
        detail: detail.into(),
        available: None,
        balances: Vec::new(),
        windows: Vec::new(),
        plan: None,
        spend_limit: None,
    }
}

fn error_view(provider_id: &str, detail: impl Into<String>) -> ProviderQuotaView {
    ProviderQuotaView {
        provider_id: provider_id.to_string(),
        kind: ProviderQuotaKind::Error,
        detail: detail.into(),
        available: None,
        balances: Vec::new(),
        windows: Vec::new(),
        plan: None,
        spend_limit: None,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct ClaudeDesktopUsageHistory {
    #[serde(default)]
    samples: Vec<ClaudeDesktopUsageSample>,
}

#[derive(Debug, Deserialize)]
struct ClaudeDesktopUsageSample {
    #[serde(rename = "t")]
    timestamp: u64,
    #[serde(rename = "u")]
    usage: ClaudeDesktopUsage,
}

#[derive(Debug, Deserialize)]
struct ClaudeDesktopUsage {
    #[serde(rename = "fh")]
    five_hour: Option<f64>,
    #[serde(rename = "sd")]
    seven_day: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekBalanceResponse {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalance>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekBalance {
    currency: String,
    total_balance: String,
    granted_balance: String,
    topped_up_balance: String,
}

#[derive(Debug, Deserialize)]
struct ChatgptWhamUsage {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<ChatgptWhamRateLimit>,
}

#[derive(Debug, Deserialize)]
struct ChatgptWhamRateLimit {
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default)]
    primary_window: Option<ChatgptWhamWindow>,
    #[serde(default)]
    secondary_window: Option<ChatgptWhamWindow>,
}

#[derive(Debug, Deserialize)]
struct ChatgptWhamWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<u64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitResponse {
    #[serde(default)]
    rate_limits: Option<CodexRateLimitSnapshot>,
    #[serde(default)]
    rate_limits_by_limit_id: Option<BTreeMap<String, CodexRateLimitSnapshot>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitSnapshot {
    #[serde(default)]
    primary: Option<CodexRateLimitWindow>,
    #[serde(default)]
    secondary: Option<CodexRateLimitWindow>,
    #[serde(default)]
    individual_limit: Option<CodexSpendLimit>,
    #[serde(default)]
    spend_control_reached: Option<bool>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitWindow {
    used_percent: f64,
    #[serde(default)]
    window_duration_mins: Option<u64>,
    #[serde(default)]
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexSpendLimit {
    used: String,
    limit: String,
    remaining_percent: f64,
    #[serde(default)]
    resets_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_official_deepseek_host_gets_a_balance_url() {
        assert_eq!(
            deepseek_balance_url("https://api.deepseek.com/v1")
                .unwrap()
                .as_str(),
            "https://api.deepseek.com/user/balance"
        );
        assert!(deepseek_balance_url("https://proxy.example.com").is_none());
        assert!(deepseek_balance_url("http://api.deepseek.com").is_none());
    }

    #[test]
    fn parses_codex_rate_limit_shape() {
        let value = serde_json::json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 42,
                    "windowDurationMins": 300,
                    "resetsAt": 1_800_000_000
                },
                "secondary": null,
                "individualLimit": {
                    "used": "8000",
                    "limit": "25000",
                    "remainingPercent": 68,
                    "resetsAt": 1_800_000_100
                },
                "spendControlReached": false,
                "planType": "pro",
                "rateLimitReachedType": null
            }
        });
        let view = codex_quota_view(
            "codex",
            value.get("rateLimits").cloned().map_or_else(
                || serde_json::json!({}),
                |limits| serde_json::json!({"rateLimits": limits}),
            ),
        );
        assert!(matches!(view.kind, ProviderQuotaKind::RateLimit));
        assert_eq!(view.windows[0].used_percent, 42.0);
        assert_eq!(view.spend_limit.unwrap().remaining_percent, 68.0);
    }

    #[test]
    fn parses_codex_rate_limit_map_shape() {
        let value = serde_json::json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "primary": {
                        "usedPercent": 12,
                        "windowDurationMins": 300,
                        "resetsAt": 1_800_000_000
                    }
                }
            }
        });
        let view = codex_quota_view("codex", value);
        assert!(matches!(view.kind, ProviderQuotaKind::RateLimit));
        assert_eq!(view.windows[0].used_percent, 12.0);
    }

    #[test]
    fn parses_documented_balance_shape() {
        let payload: DeepSeekBalanceResponse = serde_json::from_str(
            r#"{
                "is_available": true,
                "balance_infos": [{
                    "currency": "USD",
                    "total_balance": "12.50",
                    "granted_balance": "2.50",
                    "topped_up_balance": "10.00"
                }]
            }"#,
        )
        .unwrap();
        assert!(payload.is_available);
        assert_eq!(payload.balance_infos[0].total_balance, "12.50");
    }

    #[test]
    fn parses_claude_desktop_shared_usage_cache() {
        let history: ClaudeDesktopUsageHistory = serde_json::from_str(
            r#"{
                "version": 1,
                "samples": [
                    {"t": 1700000000000, "org": "ignored", "u": {"fh": 13, "sd": 42}}
                ]
            }"#,
        )
        .unwrap();
        let view = claude_desktop_quota_view("claude", &history, 1_700_000_000 + 60);
        assert!(matches!(view.kind, ProviderQuotaKind::RateLimit));
        assert_eq!(view.windows.len(), 2);
        assert_eq!(view.windows[0].label, "5-hour");
        assert_eq!(view.windows[0].used_percent, 13.0);
        assert_eq!(view.windows[1].used_percent, 42.0);
    }

    #[test]
    fn rejects_stale_claude_desktop_shared_usage_cache() {
        let history: ClaudeDesktopUsageHistory =
            serde_json::from_str(r#"{"samples":[{"t":1700000000000,"u":{"fh":13,"sd":42}}]}"#)
                .unwrap();
        let view = claude_desktop_quota_view("claude", &history, 1_700_000_000 + 86_401);
        assert!(matches!(view.kind, ProviderQuotaKind::Unavailable));
        assert!(view.detail.contains("stale"));
    }

    #[tokio::test]
    async fn keeps_provider_order_while_isolating_unavailable_checks() {
        let mut config = Config::default();
        config.providers.insert(
            "zeta".into(),
            ProviderConfig::CodexCli {
                command: "codex".into(),
                model: "model".into(),
                models: Vec::new(),
                efforts: Vec::new(),
                allow_mcp: false,
                timeout_secs: 900,
            },
        );
        config.providers.insert(
            "deepseek".into(),
            ProviderConfig::OpenaiCompatible {
                base_url: "https://api.deepseek.com".into(),
                model: "deepseek-chat".into(),
                models: Vec::new(),
                efforts: Vec::new(),
                credential: None,
                api_key_env: None,
            },
        );
        config.providers.insert(
            "alpha".into(),
            ProviderConfig::Anthropic {
                api_key_env: "ZEST_TEST_MISSING_KEY".into(),
                model: None,
                credential: None,
            },
        );

        let snapshot = fetch_provider_quotas(&config).await;
        let ids: Vec<_> = snapshot
            .providers
            .iter()
            .map(|provider| provider.provider_id.as_str())
            .collect();

        assert_eq!(ids, ["alpha", "deepseek", "zeta"]);
        assert!(matches!(
            snapshot.providers[1].kind,
            ProviderQuotaKind::Unavailable
        ));
        assert!(snapshot.providers[1].detail.contains("API key"));
        assert!(matches!(
            snapshot.providers[2].kind,
            ProviderQuotaKind::Unavailable | ProviderQuotaKind::RateLimit
        ));
    }

    fn oauth_config(model: &str, credential: Option<&str>) -> ProviderConfig {
        ProviderConfig::CodexOAuth {
            model: model.into(),
            models: Vec::new(),
            efforts: Vec::new(),
            credential: credential.map(str::to_string),
        }
    }

    fn cli_config() -> ProviderConfig {
        ProviderConfig::CodexCli {
            command: "codex".into(),
            model: "model".into(),
            models: Vec::new(),
            efforts: Vec::new(),
            allow_mcp: false,
            timeout_secs: 900,
        }
    }

    #[test]
    fn parses_chatgpt_wham_usage_windows() {
        let usage: ChatgptWhamUsage = serde_json::from_str(
            r#"{
              "plan_type": "plus",
              "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                  "used_percent": 34,
                  "limit_window_seconds": 18000,
                  "reset_at": 1778091218
                },
                "secondary_window": {
                  "used_percent": 37,
                  "limit_window_seconds": 604800,
                  "reset_at": 1778605571
                }
              }
            }"#,
        )
        .unwrap();
        let view = chatgpt_quota_view("codex", usage);
        assert!(matches!(view.kind, ProviderQuotaKind::RateLimit));
        assert_eq!(view.plan.as_deref(), Some("plus"));
        assert_eq!(view.windows.len(), 2);
        assert_eq!(view.windows[0].label, "Primary (5h)");
        assert_eq!(view.windows[0].used_percent, 34.0);
        assert_eq!(view.windows[0].window_minutes, Some(300));
        assert_eq!(view.windows[1].label, "Secondary (7d)");
        assert_eq!(view.windows[1].used_percent, 37.0);
        assert_eq!(view.windows[1].window_minutes, Some(10_080));
    }

    #[test]
    fn quota_source_is_app_server_for_every_codex_cli_id() {
        assert_eq!(
            quota_source(&cli_config()),
            Some(CodexQuotaSource::AppServer)
        );
    }

    #[test]
    fn quota_source_is_wham_for_codex_oauth_even_when_id_is_codex() {
        assert_eq!(
            quota_source(&oauth_config("gpt-5.6-sol", None)),
            Some(CodexQuotaSource::Wham)
        );
    }

    #[test]
    fn codex_oauth_named_codex_does_not_spawn_the_cli() {
        assert_ne!(
            quota_source(&oauth_config("gpt-5.6-sol", None)),
            Some(CodexQuotaSource::AppServer)
        );
    }

    #[tokio::test]
    async fn a_config_with_both_codex_kinds_emits_two_quota_views() {
        let mut config = Config::default();
        config.providers.insert(
            "codex".into(),
            oauth_config("gpt-5.6-sol", Some("missing-oauth")),
        );
        config.providers.insert("work-codex".into(), cli_config());
        let snapshot = fetch_provider_quotas(&config).await;
        assert_eq!(snapshot.providers.len(), 2);
        assert_eq!(snapshot.providers[0].provider_id, "codex");
        assert_eq!(snapshot.providers[1].provider_id, "work-codex");
        assert!(matches!(
            snapshot.providers[0].kind,
            ProviderQuotaKind::Unavailable | ProviderQuotaKind::Error
        ));
        assert!(
            snapshot.providers[0]
                .windows
                .iter()
                .all(|window| window.used_percent != 0.0)
                || snapshot.providers[0].windows.is_empty()
        );
    }

    #[test]
    fn wham_usage_401_is_not_zero_percent() {
        let view = chatgpt_usage_from_http("codex", 401, r#"{"error":"unauthorized"}"#);
        assert!(matches!(
            view.kind,
            ProviderQuotaKind::Error | ProviderQuotaKind::Unavailable
        ));
        assert!(view.windows.is_empty());
        assert!(view.detail.contains("Sign in again"), "{}", view.detail);
        assert!(!matches!(view.kind, ProviderQuotaKind::RateLimit));
    }
}
