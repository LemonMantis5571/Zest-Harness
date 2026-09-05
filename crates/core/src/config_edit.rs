//! Small, comment-preserving edits to the user/project `zest.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::config::{
    ClaudeCodePermissionMode, Config, CursorMode, ExternalAgentMode, ExternalWorkspace,
};
use crate::fsutil::atomic_write;

#[derive(Debug, Clone)]
pub struct OpenAiProviderInput {
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<String>,
    pub credential: String,
}

#[derive(Debug, Clone)]
pub struct AnthropicProviderInput {
    pub id: String,
    pub model: String,
    pub credential: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeCodeProviderInput {
    pub id: String,
    pub command: String,
    pub model: String,
    pub models: Vec<String>,
    pub allow_mcp: bool,
    pub permission_mode: ClaudeCodePermissionMode,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CursorProviderInput {
    pub id: String,
    pub command: String,
    pub model: String,
    pub models: Vec<String>,
    pub allow_mcp: bool,
    pub mode: CursorMode,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CodexOAuthProviderInput {
    pub id: String,
    pub model: String,
    pub credential: String,
}

#[derive(Debug, Clone)]
pub struct CodexCliProviderInput {
    pub id: String,
    pub command: String,
    pub model: String,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub allow_mcp: bool,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct ExternalAgentInput {
    pub id: String,
    pub mode: ExternalAgentMode,
    pub command: String,
    pub args: Vec<String>,
    pub allow_mcp: bool,
    pub model: Option<String>,
    pub workspace: ExternalWorkspace,
    pub timeout_secs: u64,
}

/// Presets for CLIs that already own their authentication session. These are
/// configuration templates only: no login command or credential is run by the
/// setup UI.
pub fn external_agent_preset(id: &str) -> Option<ExternalAgentInput> {
    external_agent_preset_with_model(id, false, None)
}

pub fn external_agent_preset_with_mcp(id: &str, allow_mcp: bool) -> Option<ExternalAgentInput> {
    external_agent_preset_with_model(id, allow_mcp, None)
}

/// Return the model choices exposed by the desktop setup for a built-in CLI.
///
/// These are CLI-owned aliases/catalogue entries, not Zest provider models.
/// A manually configured model remains supported through `zest.toml`, and the
/// desktop view preserves a currently configured value even if it is not in
/// this small catalogue.
pub fn external_agent_model_options(id: &str) -> &'static [&'static str] {
    match id {
        "claude" => &["sonnet", "opus"],
        "gemini" => &[
            "auto",
            "gemini-3-pro-preview",
            "gemini-3-flash-preview",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ],
        // A worker has no effort axis, so these are Cursor's flat ids with the
        // effort suffix already on them — what `--model` takes verbatim. It is
        // a starting shortlist, not the catalogue: `cursor-agent models` prints
        // over two hundred, and any of them can be written into zest.toml.
        // The *provider* never uses this list; it discovers instead.
        "cursor" => &[
            "composer-2.5",
            "cursor-grok-4.6-high",
            "claude-opus-5-thinking-high",
            "claude-sonnet-5-thinking-high",
            "gpt-5.6-sol-high",
            "gemini-3.1-pro",
        ],
        _ => &[],
    }
}

/// Build a built-in worker preset while optionally pinning the model passed to
/// the vendor CLI. `None` keeps the CLI's configured/default model in charge.
pub fn external_agent_preset_with_model(
    id: &str,
    allow_mcp: bool,
    model: Option<&str>,
) -> Option<ExternalAgentInput> {
    let model = model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string);

    match id {
        "claude" => Some(ExternalAgentInput {
            id: id.to_string(),
            mode: ExternalAgentMode::Headless,
            command: "claude".into(),
            args: {
                let mut args = vec![
                    "--print".into(),
                    "--verbose".into(),
                    "--permission-mode".into(),
                    "acceptEdits".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                ];
                if !allow_mcp {
                    args.push("--strict-mcp-config".into());
                }
                if model.is_some() {
                    args.extend(["--model".into(), "{model}".into()]);
                }
                args.push("{prompt}".into());
                args
            },
            allow_mcp,
            model,
            workspace: ExternalWorkspace::Isolated,
            timeout_secs: 900,
        }),
        "gemini" => Some(ExternalAgentInput {
            id: id.to_string(),
            mode: ExternalAgentMode::Acp,
            command: "gemini".into(),
            args: {
                let mut args = vec!["--acp".into()];
                if !allow_mcp {
                    args.extend(["--allowed-mcp-server-names".into(), "".into()]);
                }
                if model.is_some() {
                    args.extend(["--model".into(), "{model}".into()]);
                }
                args
            },
            allow_mcp,
            model,
            workspace: ExternalWorkspace::Isolated,
            timeout_secs: 900,
        }),
        "cursor" => Some(ExternalAgentInput {
            id: id.to_string(),
            mode: ExternalAgentMode::Acp,
            command: "cursor-agent".into(),
            args: {
                // Cursor takes its options before the subcommand, the way its
                // own docs write `agent --api-key "$KEY" acp`.
                let mut args = Vec::new();
                if model.is_some() {
                    args.extend(["--model".into(), "{model}".into()]);
                }
                args.push("acp".into());
                args
            },
            // Unlike Gemini there is no flag that narrows the servers: Cursor's
            // ACP mode reads `.cursor/mcp.json` itself, and `session/new`
            // carrying an empty `mcpServers` does not override that. So this
            // records the user's choice without being able to enforce it, and
            // the isolated worktree stays the boundary that does.
            allow_mcp,
            model,
            workspace: ExternalWorkspace::Isolated,
            timeout_secs: 900,
        }),
        _ => None,
    }
}

pub fn add_openai_provider(path: &Path, input: &OpenAiProviderInput) -> Result<(), String> {
    let id = input.id.trim();
    let base_url = input.base_url.trim().trim_end_matches('/');
    let model = input.model.trim();
    let credential = input.credential.trim();

    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("provider id may contain only letters, numbers, `_`, and `-`".into());
    }
    if model.is_empty() {
        return Err("a default model is required".into());
    }
    if credential.is_empty() {
        return Err("a credential name is required".into());
    }
    let url =
        reqwest::Url::parse(base_url).map_err(|_| "endpoint must be a valid URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("endpoint must be an http(s) URL with a host".into());
    }

    let original = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "openai_compatible" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }
    provider["kind"] = toml_edit::value("openai_compatible");
    provider["base_url"] = toml_edit::value(base_url);
    provider["model"] = toml_edit::value(model);
    provider["credential"] = toml_edit::value(credential);

    let mut models = Array::new();
    for value in input
        .models
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
    {
        models.push(Value::from(value));
    }
    if models.is_empty() {
        provider.remove("models");
    } else {
        provider["models"] = toml_edit::value(models);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn add_anthropic_provider(path: &Path, input: &AnthropicProviderInput) -> Result<(), String> {
    let id = input.id.trim();
    let model = input.model.trim();
    let credential = input.credential.trim();

    validate_id(id, "provider")?;
    if model.is_empty() {
        return Err("a default model is required".into());
    }
    if credential.is_empty() {
        return Err("a credential name is required".into());
    }

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "anthropic" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }
    provider["kind"] = toml_edit::value("anthropic");
    provider["model"] = toml_edit::value(model);
    provider["credential"] = toml_edit::value(credential);

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Add or update a first-class Claude Code parent provider while preserving
/// comments and unrelated provider entries in the user's TOML file.
pub fn add_claude_code_provider(
    path: &Path,
    input: &ClaudeCodeProviderInput,
) -> Result<(), String> {
    let id = input.id.trim();
    let command = input.command.trim();
    let model = input.model.trim();

    validate_id(id, "provider")?;
    if command.is_empty() {
        return Err("Claude Code command is required".into());
    }
    if model.is_empty() {
        return Err("a Claude Code model is required".into());
    }
    if input.timeout_secs == 0 || input.timeout_secs > 3_600 {
        return Err("Claude Code timeout must be between 1 and 3600 seconds".into());
    }

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "claude_code" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }

    provider["kind"] = toml_edit::value("claude_code");
    provider["command"] = toml_edit::value(command);
    provider["model"] = toml_edit::value(model);
    provider["allow_mcp"] = toml_edit::value(input.allow_mcp);
    provider["permission_mode"] = toml_edit::value(match input.permission_mode {
        ClaudeCodePermissionMode::Default => "default",
        ClaudeCodePermissionMode::AcceptEdits => "accept_edits",
        ClaudeCodePermissionMode::Plan => "plan",
        ClaudeCodePermissionMode::BypassPermissions => "bypass_permissions",
    });
    provider["timeout_secs"] = toml_edit::value(input.timeout_secs as i64);

    let mut models = Array::new();
    for value in input
        .models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        models.push(Value::from(value));
    }
    if models.is_empty() {
        provider.remove("models");
    } else {
        provider["models"] = toml_edit::value(models);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Add or update a Cursor CLI parent provider, preserving comments and
/// unrelated entries.
///
/// `mode` is written every time rather than only when it differs from the
/// default, because on this provider it is the safety setting: Cursor never
/// asks before editing a file, so `agent` versus `plan` is the difference
/// between a chat that can rewrite the checkout and one that cannot.
pub fn add_cursor_provider(path: &Path, input: &CursorProviderInput) -> Result<(), String> {
    let id = input.id.trim();
    let command = input.command.trim();
    let model = input.model.trim();

    validate_id(id, "provider")?;
    if command.is_empty() {
        return Err("Cursor command is required".into());
    }
    if model.is_empty() {
        return Err("a Cursor model is required".into());
    }
    if input.timeout_secs == 0 || input.timeout_secs > 3_600 {
        return Err("Cursor timeout must be between 1 and 3600 seconds".into());
    }

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "cursor_acp" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }

    provider["kind"] = toml_edit::value("cursor_acp");
    provider["command"] = toml_edit::value(command);
    provider["model"] = toml_edit::value(model);
    provider["allow_mcp"] = toml_edit::value(input.allow_mcp);
    provider["mode"] = toml_edit::value(match input.mode {
        CursorMode::Agent => "agent",
        CursorMode::Plan => "plan",
        CursorMode::Ask => "ask",
    });
    provider["timeout_secs"] = toml_edit::value(input.timeout_secs as i64);

    let mut models = Array::new();
    for value in input
        .models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        models.push(Value::from(value));
    }
    if models.is_empty() {
        provider.remove("models");
    } else {
        provider["models"] = toml_edit::value(models);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Add or update a first-class Codex CLI parent provider while preserving
/// comments and unrelated provider entries in the user's TOML file.
pub fn add_codex_cli_provider(path: &Path, input: &CodexCliProviderInput) -> Result<(), String> {
    let id = input.id.trim();
    let command = input.command.trim();
    let model = input.model.trim();

    validate_id(id, "provider")?;
    if command.is_empty() {
        return Err("Codex CLI command is required".into());
    }
    if model.is_empty() {
        return Err("a Codex CLI model is required".into());
    }
    if input.timeout_secs == 0 || input.timeout_secs > 3_600 {
        return Err("Codex CLI timeout must be between 1 and 3600 seconds".into());
    }

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "codex_cli" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }

    provider["kind"] = toml_edit::value("codex_cli");
    provider["command"] = toml_edit::value(command);
    provider["model"] = toml_edit::value(model);
    provider["allow_mcp"] = toml_edit::value(input.allow_mcp);
    provider["timeout_secs"] = toml_edit::value(input.timeout_secs as i64);

    let mut models = Array::new();
    for value in input
        .models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        models.push(Value::from(value));
    }
    if models.is_empty() {
        provider.remove("models");
    } else {
        provider["models"] = toml_edit::value(models);
    }

    let mut efforts = Array::new();
    for value in input
        .efforts
        .iter()
        .map(|effort| effort.trim())
        .filter(|effort| !effort.is_empty())
    {
        efforts.push(Value::from(value));
    }
    if efforts.is_empty() {
        provider.remove("efforts");
    } else {
        provider["efforts"] = toml_edit::value(efforts);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Add or update a ChatGPT Codex parent while preserving comments and
/// unrelated provider entries. Refuses if the id already has another kind.
pub fn add_codex_oauth_provider(
    path: &Path,
    input: &CodexOAuthProviderInput,
) -> Result<(), String> {
    let id = input.id.trim();
    let model = input.model.trim();
    let credential = input.credential.trim();

    validate_id(id, "provider")?;
    if model.is_empty() {
        return Err("a ChatGPT Codex model is required".into());
    }
    let credential = if credential.is_empty() {
        id
    } else {
        credential
    };

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("providers") {
        doc["providers"] = Item::Table(Table::new());
    }
    let providers = doc["providers"]
        .as_table_mut()
        .ok_or_else(|| "[providers] is not a table".to_string())?;
    let entry = providers.entry(id).or_insert(Item::Table(Table::new()));
    let provider = entry
        .as_table_mut()
        .ok_or_else(|| format!("provider `{id}` is not a table"))?;
    if let Some(kind) = provider.get("kind").and_then(Item::as_str) {
        if kind != "codex_oauth" {
            return Err(format!("provider `{id}` already has kind `{kind}`"));
        }
    }

    provider["kind"] = toml_edit::value("codex_oauth");
    provider["model"] = toml_edit::value(model);
    provider["credential"] = toml_edit::value(credential);
    provider.remove("command");
    provider.remove("allow_mcp");
    provider.remove("timeout_secs");
    provider.remove("models");
    provider.remove("efforts");

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn upsert_external_agent(path: &Path, input: &ExternalAgentInput) -> Result<(), String> {
    let id = input.id.trim();
    let command = input.command.trim();

    validate_id(id, "agent")?;
    if command.is_empty() {
        return Err("agent command is required".into());
    }
    if input.timeout_secs == 0 || input.timeout_secs > 3_600 {
        return Err("agent timeout must be between 1 and 3600 seconds".into());
    }
    if input.mode == ExternalAgentMode::Acp && input.args.iter().any(|arg| arg.contains("{prompt}"))
    {
        return Err("ACP agents receive their prompt over stdio, not in the arguments".into());
    }

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("agents") {
        doc["agents"] = Item::Table(Table::new());
    }
    let agents = doc["agents"]
        .as_table_mut()
        .ok_or_else(|| "[agents] is not a table".to_string())?;
    let entry = agents.entry(id).or_insert(Item::Table(Table::new()));
    let agent = entry
        .as_table_mut()
        .ok_or_else(|| format!("agent `{id}` is not a table"))?;

    agent["mode"] = toml_edit::value(match input.mode {
        ExternalAgentMode::Headless => "headless",
        ExternalAgentMode::Acp => "acp",
    });
    agent["command"] = toml_edit::value(command);

    let mut args = Array::new();
    for arg in &input.args {
        args.push(Value::from(arg.as_str()));
    }
    if args.is_empty() {
        agent.remove("args");
    } else {
        agent["args"] = toml_edit::value(args);
    }
    agent["allow_mcp"] = toml_edit::value(input.allow_mcp);

    if let Some(model) = input
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        agent["model"] = toml_edit::value(model);
    } else {
        agent.remove("model");
    }
    agent["workspace"] = toml_edit::value(match input.workspace {
        ExternalWorkspace::Isolated => "isolated",
        ExternalWorkspace::Current => "current",
    });
    agent["timeout_secs"] = toml_edit::value(input.timeout_secs as i64);

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// One `[mcp.<id>]` entry as the desktop supplies it.
#[derive(Debug, Clone)]
pub struct McpServerInput {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub header_credentials: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
    pub enabled: bool,
    pub timeout_secs: u64,
}

pub fn upsert_mcp_server(path: &Path, input: &McpServerInput) -> Result<(), String> {
    let id = input.id.trim();
    validate_id(id, "MCP server")?;

    let draft = crate::config::McpServerConfig {
        command: input.command.clone(),
        args: input.args.clone(),
        url: {
            let url = input.url.trim();
            if url.is_empty() {
                None
            } else {
                Some(url.to_string())
            }
        },
        headers: input.headers.clone(),
        header_credentials: input.header_credentials.clone(),
        env_vars: input.env_vars.clone(),
        enabled: input.enabled,
        timeout_secs: input.timeout_secs,
    };
    draft.validate(id)?;

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if !doc.contains_key("mcp") {
        doc["mcp"] = Item::Table(Table::new());
    }
    let servers = doc["mcp"]
        .as_table_mut()
        .ok_or_else(|| "[mcp] is not a table".to_string())?;
    let entry = servers.entry(id).or_insert(Item::Table(Table::new()));
    let server = entry
        .as_table_mut()
        .ok_or_else(|| format!("MCP server `{id}` is not a table"))?;

    if draft.is_http() {
        server["url"] = toml_edit::value(draft.http_url().unwrap());
        server.remove("command");
        server.remove("args");
        server.remove("env_vars");
        if draft.headers.is_empty() {
            server.remove("headers");
        } else {
            let mut headers = Table::new();
            for (name, env_name) in &draft.headers {
                headers[name.as_str()] = toml_edit::value(env_name.as_str());
            }
            server["headers"] = Item::Table(headers);
        }
        if draft.header_credentials.is_empty() {
            server.remove("header_credentials");
        } else {
            let mut credentials = Table::new();
            for (name, account) in &draft.header_credentials {
                credentials[name.as_str()] = toml_edit::value(account.as_str());
            }
            server["header_credentials"] = Item::Table(credentials);
        }
    } else {
        server["command"] = toml_edit::value(input.command.trim());
        server.remove("url");
        server.remove("headers");
        server.remove("header_credentials");

        let mut args = Array::new();
        for arg in &input.args {
            args.push(Value::from(arg.as_str()));
        }
        if args.is_empty() {
            server.remove("args");
        } else {
            server["args"] = toml_edit::value(args);
        }

        let mut env_vars = Array::new();
        for name in &input.env_vars {
            let name = name.trim();
            if !name.is_empty() {
                env_vars.push(Value::from(name));
            }
        }
        if env_vars.is_empty() {
            server.remove("env_vars");
        } else {
            server["env_vars"] = toml_edit::value(env_vars);
        }
    }

    server["enabled"] = toml_edit::value(input.enabled);
    server["timeout_secs"] = toml_edit::value(input.timeout_secs as i64);

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn remove_mcp_server(path: &Path, id: &str) -> Result<(), String> {
    let id = id.trim();
    validate_id(id, "MCP server")?;

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if let Some(servers) = doc.get_mut("mcp").and_then(Item::as_table_mut) {
        servers.remove(id);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn remove_external_agent(path: &Path, id: &str) -> Result<(), String> {
    let id = id.trim();
    validate_id(id, "agent")?;

    let original = read_config(path)?;
    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("cannot parse existing config: {e}"))?;
    if let Some(agents) = doc.get_mut("agents").and_then(Item::as_table_mut) {
        agents.remove(id);
    }

    let rendered = doc.to_string();
    Config::parse(&rendered).map_err(|e| e.to_string())?;
    atomic_write(path, rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn validate_id(id: &str, noun: &str) -> Result<(), String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "{noun} id may contain only letters, numbers, `_`, and `-`"
        ));
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_provider_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(
            &path,
            "# keep me\n[providers.anthropic]\nkind = \"anthropic\"\napi_key_env = \"KEY\"\n",
        )
        .unwrap();
        add_openai_provider(
            &path,
            &OpenAiProviderInput {
                id: "deepseek".into(),
                base_url: "https://api.deepseek.com/".into(),
                model: "deepseek-v4-flash".into(),
                models: vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()],
                credential: "deepseek".into(),
            },
        )
        .unwrap();
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(raw.contains("# keep me"));
        let config = Config::parse(&raw).unwrap();
        assert!(config.providers.contains_key("deepseek"));
    }

    #[test]
    fn adds_anthropic_api_provider_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(
            &path,
            "# keep me\n[providers.codex]\nkind = \"gateway\"\nbase_url = \"http://localhost\"\nmodel = \"m\"\n",
        )
        .unwrap();

        add_anthropic_provider(
            &path,
            &AnthropicProviderInput {
                id: "anthropic".into(),
                model: "claude-opus-5".into(),
                credential: "anthropic".into(),
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(path).unwrap();
        assert!(raw.contains("# keep me"));
        let config = Config::parse(&raw).unwrap();
        assert!(matches!(
            config.providers["anthropic"],
            crate::config::ProviderConfig::Anthropic {
                credential: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn adds_claude_code_parent_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "# keep me\n[default]\nprovider = \"codex\"\n").unwrap();

        add_claude_code_provider(
            &path,
            &ClaudeCodeProviderInput {
                id: "claude".into(),
                command: "claude".into(),
                model: "sonnet".into(),
                models: vec!["sonnet".into(), "opus".into()],
                allow_mcp: false,
                permission_mode: ClaudeCodePermissionMode::AcceptEdits,
                timeout_secs: 900,
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("[providers.claude]"));
        assert!(raw.contains("permission_mode = \"accept_edits\""));
        let config = Config::parse(&raw).unwrap();
        assert!(matches!(
            config.providers["claude"],
            crate::config::ProviderConfig::ClaudeCode {
                permission_mode: ClaudeCodePermissionMode::AcceptEdits,
                ..
            }
        ));
    }

    #[test]
    fn adds_codex_cli_parent_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "# keep me\n[default]\nprovider = \"claude\"\n").unwrap();

        add_codex_cli_provider(
            &path,
            &CodexCliProviderInput {
                id: "codex".into(),
                command: "codex".into(),
                model: "gpt-5.6-sol".into(),
                models: vec!["gpt-5.6-sol".into()],
                efforts: vec!["low".into(), "high".into()],
                allow_mcp: false,
                timeout_secs: 900,
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("[providers.codex]"));
        assert!(raw.contains("kind = \"codex_cli\""));
        let config = Config::parse(&raw).unwrap();
        assert!(matches!(
            config.providers["codex"],
            crate::config::ProviderConfig::CodexCli { .. }
        ));
    }

    #[test]
    fn adds_codex_oauth_parent_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "# keep me\n[default]\nprovider = \"claude\"\n").unwrap();

        add_codex_oauth_provider(
            &path,
            &CodexOAuthProviderInput {
                id: "codex".into(),
                model: "gpt-5.6-sol".into(),
                credential: "codex".into(),
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("kind = \"codex_oauth\""));
        assert!(!raw.contains("access_token"));
        let config = Config::parse(&raw).unwrap();
        assert!(matches!(
            config.providers["codex"],
            crate::config::ProviderConfig::CodexOAuth { .. }
        ));
    }

    #[test]
    fn add_codex_oauth_refuses_a_kind_clash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "[providers.codex]\nkind = \"codex_cli\"\n").unwrap();
        let err = add_codex_oauth_provider(
            &path,
            &CodexOAuthProviderInput {
                id: "codex".into(),
                model: "gpt-5.6-sol".into(),
                credential: "codex".into(),
            },
        )
        .unwrap_err();
        assert!(err.contains("already has kind"), "{err}");
    }

    #[test]
    fn preset_agent_preserves_comments_and_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "# keep me\n[default]\nprovider = \"codex\"\n").unwrap();

        upsert_external_agent(&path, &external_agent_preset("claude").unwrap()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("[agents.claude]"));
        assert!(raw.contains("--verbose"));
        assert!(raw.contains("acceptEdits"));
        assert!(raw.contains("--strict-mcp-config"));
        assert!(raw.contains("allow_mcp = false"));
        let config = Config::parse(&raw).unwrap();
        assert_eq!(config.agents["claude"].mode, ExternalAgentMode::Headless);

        remove_external_agent(&path, "claude").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(!raw.contains("[agents.claude]"));
        Config::parse(&raw).unwrap();
    }

    #[test]
    fn rejects_prompt_placeholder_for_acp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        let mut input = external_agent_preset("gemini").unwrap();
        input.args.push("{prompt}".into());
        let error = upsert_external_agent(&path, &input).unwrap_err();
        assert!(error.contains("over stdio"));
    }

    #[test]
    fn enabling_cursor_writes_no_model_allow_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        let input = CursorProviderInput {
            id: "cursor".into(),
            command: "cursor-agent".into(),
            model: "composer-2.5".into(),
            models: Vec::new(),
            allow_mcp: false,
            mode: CursorMode::Agent,
            timeout_secs: 900,
        };
        add_cursor_provider(&path, &input).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        // An allow-list is taken literally and suppresses discovery, so writing
        // one here would pin the picker to whatever was hard-coded that day.
        assert!(!written.contains("models"), "{written}");
        assert!(written.contains("mode = \"agent\""), "{written}");

        // A stale list from an older build is cleared rather than preserved.
        let stale = CursorProviderInput {
            models: vec!["composer-2.5".into(), "gpt-5.6-sol".into()],
            ..input.clone()
        };
        add_cursor_provider(&path, &stale).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("models"));
        add_cursor_provider(&path, &input).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("models"));
    }

    #[test]
    fn the_cursor_preset_puts_its_options_before_the_subcommand() {
        let plain = external_agent_preset("cursor").unwrap();
        assert_eq!(plain.command, "cursor-agent");
        assert_eq!(plain.mode, ExternalAgentMode::Acp);
        // Isolation is required for every ACP worker, and Cursor is the reason
        // why: it edits without sending session/request_permission at all.
        assert_eq!(plain.workspace, ExternalWorkspace::Isolated);
        assert_eq!(plain.args, vec!["acp".to_string()]);

        // `acp` is a subcommand, so a pinned model has to precede it — the CLI
        // reads options before the command, as in `agent --api-key "$KEY" acp`.
        let pinned =
            external_agent_preset_with_model("cursor", false, Some("composer-2.5")).unwrap();
        assert_eq!(pinned.model.as_deref(), Some("composer-2.5"));
        assert_eq!(
            pinned.args,
            vec![
                "--model".to_string(),
                "{model}".to_string(),
                "acp".to_string()
            ]
        );
        assert!(upsert_external_agent(
            &tempfile::tempdir().unwrap().path().join("zest.toml"),
            &pinned
        )
        .is_ok());
    }

    #[test]
    fn mcp_enabled_presets_use_the_cli_owned_configuration() {
        let claude = external_agent_preset_with_mcp("claude", true).unwrap();
        assert!(claude.allow_mcp);
        assert!(!claude.args.iter().any(|arg| arg == "--strict-mcp-config"));

        let gemini = external_agent_preset_with_mcp("gemini", true).unwrap();
        assert!(gemini.allow_mcp);
        assert!(!gemini
            .args
            .iter()
            .any(|arg| arg == "--allowed-mcp-server-names"));
    }

    #[test]
    fn model_aware_presets_pin_the_cli_model_without_changing_defaults() {
        let claude = external_agent_preset_with_model("claude", false, Some("opus")).unwrap();
        assert_eq!(claude.model.as_deref(), Some("opus"));
        assert!(claude
            .args
            .windows(2)
            .any(|pair| { pair[0] == "--model" && pair[1] == "{model}" }));

        let gemini =
            external_agent_preset_with_model("gemini", true, Some("gemini-2.5-pro")).unwrap();
        assert_eq!(gemini.model.as_deref(), Some("gemini-2.5-pro"));
        assert!(gemini
            .args
            .windows(2)
            .any(|pair| { pair[0] == "--model" && pair[1] == "{model}" }));

        let default = external_agent_preset("claude").unwrap();
        assert!(default.model.is_none());
        assert!(!default.args.iter().any(|arg| arg == "--model"));
    }

    #[test]
    fn model_options_are_scoped_to_builtin_workers() {
        assert_eq!(external_agent_model_options("claude"), &["sonnet", "opus"]);
        assert!(external_agent_model_options("custom").is_empty());
    }

    fn mcp_input(id: &str) -> McpServerInput {
        McpServerInput {
            id: id.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            url: String::new(),
            headers: BTreeMap::new(),
            header_credentials: BTreeMap::new(),
            env_vars: vec!["GITHUB_TOKEN".into()],
            enabled: true,
            timeout_secs: 120,
        }
    }

    #[test]
    fn adds_an_mcp_server_without_discarding_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(
            &path,
            "# keep me\n[providers.anthropic]\nkind = \"anthropic\"\napi_key_env = \"KEY\"\n",
        )
        .unwrap();

        upsert_mcp_server(&path, &mcp_input("github")).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep me"));
        let config = Config::parse(&raw).unwrap();
        let server = &config.mcp["github"];
        assert_eq!(server.command, "npx");
        assert_eq!(server.env_vars, vec!["GITHUB_TOKEN".to_string()]);
        assert!(server.enabled);

        remove_mcp_server(&path, "github").unwrap();
        let config = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(config.mcp.is_empty());
    }

    /// Turning a server off must keep the entry: the user configured a command
    /// and arguments, and an off switch that deletes them is a trap.
    #[test]
    fn disabling_an_mcp_server_keeps_how_it_was_configured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "").unwrap();

        upsert_mcp_server(&path, &mcp_input("github")).unwrap();
        upsert_mcp_server(
            &path,
            &McpServerInput {
                enabled: false,
                ..mcp_input("github")
            },
        )
        .unwrap();

        let config = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let server = &config.mcp["github"];
        assert!(!server.enabled);
        assert_eq!(server.args.len(), 2);
    }

    #[test]
    fn an_env_var_value_is_refused_so_no_secret_reaches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "").unwrap();

        let error = upsert_mcp_server(
            &path,
            &McpServerInput {
                env_vars: vec!["GITHUB_TOKEN=ghp_secret".into()],
                ..mcp_input("github")
            },
        )
        .expect_err("a value must not be accepted");
        assert!(error.contains("names only"), "{error}");
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("ghp_secret"));
    }

    #[test]
    fn writes_an_http_mcp_server_and_drops_stale_command_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "").unwrap();
        upsert_mcp_server(&path, &mcp_input("github")).unwrap();
        upsert_mcp_server(
            &path,
            &McpServerInput {
                command: String::new(),
                args: Vec::new(),
                url: "https://example.com/mcp".into(),
                headers: BTreeMap::from([("Authorization".into(), "MCP_AUTHORIZATION".into())]),
                header_credentials: BTreeMap::new(),
                env_vars: Vec::new(),
                ..mcp_input("github")
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let config = Config::parse(&raw).unwrap();
        let server = &config.mcp["github"];
        assert_eq!(server.http_url(), Some("https://example.com/mcp"));
        assert!(server.command.is_empty());
        assert!(!raw.contains("command"));
        assert!(!raw.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn writes_only_a_credential_reference_for_an_http_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "").unwrap();

        upsert_mcp_server(
            &path,
            &McpServerInput {
                command: String::new(),
                args: Vec::new(),
                url: "https://example.com/mcp".into(),
                headers: BTreeMap::new(),
                header_credentials: BTreeMap::from([(
                    "Authorization".into(),
                    "mcp-header:test-account".into(),
                )]),
                env_vars: Vec::new(),
                ..mcp_input("remote")
            },
        )
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("header_credentials"));
        assert!(raw.contains("mcp-header:test-account"));
        assert!(!raw.contains("Bearer "));
    }

    #[test]
    fn an_out_of_range_mcp_timeout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zest.toml");
        std::fs::write(&path, "").unwrap();
        assert!(upsert_mcp_server(
            &path,
            &McpServerInput {
                timeout_secs: 0,
                ..mcp_input("github")
            },
        )
        .is_err());
    }
}
