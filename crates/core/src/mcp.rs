//! Zest-owned MCP servers.
//!
//! Why Zest needs its own at all: a CLI-owned parent — Claude Code, Codex —
//! already loads MCP servers from its own configuration, and `allow_mcp` is the
//! only control Zest has over them. A native provider has no CLI to borrow
//! from. An Anthropic key or an OpenAI-compatible endpoint such as DeepSeek is
//! reached over plain HTTP with no harness behind it, so without this module
//! those chats can never reach an MCP server at all.
//!
//! The boundaries from the CLI lane are kept:
//!
//! - A server runs only because the user configured it in `zest.toml`.
//! - Every call goes through the approval gate as exec risk. The server is a
//!   process Zest cannot inspect, so its calls are never auto-eligible.
//! - The child never inherits Zest's provider credentials. Secret-looking
//!   variables are scrubbed unless the server's `env_vars` names them.
//!
//! Tool discovery is cached rather than performed during session startup.
//! `RuntimeBuilder::build` is synchronous and must not block a new chat behind
//! a handshake with every configured server, so the desktop refreshes the
//! catalogue when a server is saved or checked and the runtime registers from
//! it. A stale entry costs one clear error from the server, not a wrong answer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::codex_oauth::SESSION_ENV;
use crate::config::McpServerConfig;
use crate::fsutil::atomic_write_json;
use crate::tools::approval::{ApprovalPreview, ToolRisk};
use crate::tools::external_agent::{prepare_external_command, resolve_program};
use crate::tools::outcome::ToolOutcome;
use crate::tools::prepared::PreparedToolCall;
use crate::tools::{Tool, ToolRegistry};

/// Wire version Zest speaks. A server that negotiates a different one is still
/// used: the fields this client touches have not moved across revisions, and
/// refusing to talk to a newer server would be worse than tolerating it.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Prefix on every registered tool name, so a server can never shadow a Zest
/// tool and the transcript always says which server ran.
pub const MCP_TOOL_PREFIX: &str = "mcp";
/// Servers Zest will start. A ceiling, not a quota — the tool list sits at the
/// front of the cached prompt prefix, so it cannot be unbounded.
pub const MAX_MCP_SERVERS: usize = 12;
/// Tools registered per server, for the same reason.
pub const MAX_TOOLS_PER_SERVER: usize = 48;
/// Ceiling on one tool result before it is clipped. The spill policy handles
/// context pressure above this; this is only protection against a server that
/// answers with an unbounded stream.
const MAX_RESULT_BYTES: usize = 256 * 1024;
const MAX_ERROR_CHARS: usize = 2_000;
/// Upper bound on a configured `timeout_secs`, so a config typo cannot make a
/// turn wait indefinitely.
const MAX_TIMEOUT_SECS: u64 = 600;
/// Handshake budget. Separate from the call timeout because a server that never
/// completes `initialize` is broken, not slow.
const CONNECT_TIMEOUT_SECS: u64 = 20;

/// One tool as the server described it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema, passed to the model unchanged. An absent or non-object
    /// schema becomes an empty object so the tool list stays valid.
    #[serde(default = "empty_schema")]
    pub input_schema: Value,
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// What a server offered the last time Zest talked to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerCatalog {
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Unix seconds. Shown as an age in the UI; never used to expire an entry,
    /// because a server that is merely old is not a server that is wrong.
    #[serde(default)]
    pub fetched_at: u64,
}

/// Cached tool lists for every server Zest has successfully talked to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpCatalog {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerCatalog>,
}

impl McpCatalog {
    /// `~/.zest/mcp-catalog.json`. Machine-level like the user config: which
    /// tools a locally installed server exposes is a property of this machine,
    /// not of a repository.
    pub fn path() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".zest").join("mcp-catalog.json"))
    }

    /// A missing or unreadable catalogue is empty, never an error. It is a
    /// cache: losing it costs one refresh, and failing startup over it would
    /// be a worse trade.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(body) = std::fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&body).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::path() else {
            return Err("could not locate the user data directory".into());
        };
        atomic_write_json(&path, self)
            .map_err(|error| format!("could not write the MCP catalogue: {error}"))
    }

    pub fn tools(&self, id: &str) -> &[McpToolDef] {
        self.servers
            .get(id)
            .map(|entry| entry.tools.as_slice())
            .unwrap_or_default()
    }

    pub fn set(&mut self, id: &str, tools: Vec<McpToolDef>) {
        self.servers.insert(
            id.to_string(),
            McpServerCatalog {
                tools,
                fetched_at: now_secs(),
            },
        );
    }

    pub fn forget(&mut self, id: &str) {
        self.servers.remove(id);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Zest's model-visible name for a server tool: `mcp__<server>__<tool>`.
///
/// Both halves are reduced to the character set tool names allow, so a server
/// that names a tool `search.web` cannot produce a definition the provider
/// rejects. Two remote names that differ only in stripped characters — `a.b`
/// and `a-b` — collapse onto one Zest name and the first registered wins;
/// truncation past 64 characters can do the same. Both are preferred to sending
/// the provider a tool list it refuses, which would fail the whole turn.
pub fn qualified_tool_name(server_id: &str, tool: &str) -> String {
    let mut name = format!(
        "{MCP_TOOL_PREFIX}__{}__{}",
        sanitize_segment(server_id),
        sanitize_segment(tool)
    );
    // The provider caps tool names at 64 characters. Truncating is better than
    // sending a definition the API refuses, which would break the whole turn.
    if name.len() > 64 {
        name.truncate(64);
    }
    name
}

fn sanitize_segment(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A configured server and, once something has needed it, its running process.
///
/// Connecting is lazy on purpose: a chat that never calls an MCP tool never
/// starts the server, so a broken entry costs an error on first use instead of
/// a slower startup for every session.
pub struct McpServer {
    id: String,
    config: McpServerConfig,
    cwd: PathBuf,
    /// One connection, one lock. MCP stdio is a single ordered stream, so
    /// concurrent tool calls to the same server have to serialize anyway; the
    /// lock is where that becomes explicit rather than a corrupted read.
    conn: AsyncMutex<Option<Connection>>,
}

struct Connection {
    /// Held so the process is killed when the connection is dropped.
    _child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpServer {
    pub fn new(id: impl Into<String>, config: McpServerConfig, cwd: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            config,
            cwd: cwd.into(),
            conn: AsyncMutex::new(None),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.timeout_secs.clamp(1, MAX_TIMEOUT_SECS))
    }

    /// Ask the server for its tool list, starting it if needed.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, String> {
        let result = self.request("tools/list", json!({})).await?;
        let mut tools = Vec::new();
        for entry in result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(name) = entry.get("name").and_then(Value::as_str) else {
                continue;
            };
            let description = entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input_schema = entry
                .get("inputSchema")
                .filter(|schema| schema.is_object())
                .cloned()
                .unwrap_or_else(empty_schema);
            tools.push(McpToolDef {
                name: name.to_string(),
                description,
                input_schema,
            });
            if tools.len() >= MAX_TOOLS_PER_SERVER {
                break;
            }
        }
        Ok(tools)
    }

    /// Run one tool. The `Err` string goes back to the model verbatim, so it
    /// says which server failed and why.
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> Result<String, String> {
        let arguments = if arguments.is_object() {
            arguments
        } else {
            json!({})
        };
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await?;
        let body = text_from_content(&result);
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(format!(
                "{} reported an error: {}",
                self.id,
                clip(&body, MAX_ERROR_CHARS)
            ));
        }
        Ok(clip(&body, MAX_RESULT_BYTES))
    }

    /// Send a request, reconnecting once if the stream was already dead.
    ///
    /// One retry, not a loop: a server that closed its stdout because it
    /// crashed will do it again, and a retry loop would turn that into a
    /// process-spawning loop inside a single tool call.
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match self.request_once(method, params.clone()).await {
            Ok(value) => Ok(value),
            Err(error) if error.transport => {
                self.conn.lock().await.take();
                self.request_once(method, params)
                    .await
                    .map_err(|error| error.message)
            }
            Err(error) => Err(error.message),
        }
    }

    async fn request_once(&self, method: &str, params: Value) -> Result<Value, RequestError> {
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            *guard = Some(self.connect().await.map_err(RequestError::fatal)?);
        }
        let conn = guard.as_mut().expect("connection was just established");
        let id = conn.next_id;
        conn.next_id += 1;

        let outcome = tokio::time::timeout(self.timeout(), async {
            send_message(
                &mut conn.stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params,
                }),
            )
            .await?;
            read_response(&mut conn.reader, &mut conn.stdin, id).await
        })
        .await;

        match outcome {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                // A protocol-level error leaves the stream usable; a transport
                // error does not, so drop the connection with it.
                if error.transport {
                    *guard = None;
                }
                Err(error)
            }
            Err(_) => {
                // A timed-out request leaves an unread reply in the stream,
                // which would be read as the answer to the *next* call. The
                // process goes with it.
                *guard = None;
                Err(RequestError::fatal(format!(
                    "{} did not answer {method} within {}s",
                    self.id,
                    self.timeout().as_secs()
                )))
            }
        }
    }

    async fn connect(&self) -> Result<Connection, String> {
        let mut command = Command::new(resolve_program(&self.config.command));
        command
            .args(&self.config.args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited rather than piped: nothing reads it while the session
            // is alive, and a piped stderr nobody drains blocks a chatty
            // server once its pipe buffer fills.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        prepare_external_command(&mut command);
        scrub_environment(&mut command, &self.config.env_vars);

        let mut child = command.spawn().map_err(|error| {
            format!(
                "could not start the {} MCP server ({}): {error}",
                self.id, self.config.command
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{} stdin was not piped", self.id))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{} stdout was not piped", self.id))?;
        let mut conn = Connection {
            _child: child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };

        let handshake = tokio::time::timeout(
            Duration::from_secs(CONNECT_TIMEOUT_SECS),
            initialize(&mut conn),
        )
        .await;
        match handshake {
            Ok(Ok(())) => Ok(conn),
            Ok(Err(error)) => Err(error.message),
            Err(_) => Err(format!(
                "{} did not complete the MCP handshake within {CONNECT_TIMEOUT_SECS}s",
                self.id
            )),
        }
    }
}

async fn initialize(conn: &mut Connection) -> Result<(), RequestError> {
    let id = conn.next_id;
    conn.next_id += 1;
    send_message(
        &mut conn.stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "zest", "version": env!("CARGO_PKG_VERSION") },
            },
        }),
    )
    .await?;
    read_response(&mut conn.reader, &mut conn.stdin, id).await?;
    send_message(
        &mut conn.stdin,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await?;
    Ok(())
}

/// A failed request, and whether the stream survived it.
struct RequestError {
    message: String,
    transport: bool,
}

impl RequestError {
    /// The request failed but the connection is still usable — a JSON-RPC
    /// error reply, or a server that answered nonsense.
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transport: false,
        }
    }

    /// The pipe is gone. The caller may reconnect and try once more.
    fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transport: true,
        }
    }
}

async fn send_message(stdin: &mut ChildStdin, message: &Value) -> Result<(), RequestError> {
    let line = serde_json::to_string(message)
        .map_err(|error| RequestError::fatal(format!("encode MCP request: {error}")))?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| RequestError::transport(format!("write to the MCP server: {error}")))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| RequestError::transport(format!("write to the MCP server: {error}")))?;
    stdin
        .flush()
        .await
        .map_err(|error| RequestError::transport(format!("write to the MCP server: {error}")))
}

/// Read until the reply with `expected_id` arrives.
///
/// Notifications are skipped and server-initiated requests are refused rather
/// than ignored: a server that asks for `sampling/createMessage` and never
/// hears back would wait forever, and Zest will not hand a server the model.
async fn read_response(
    reader: &mut BufReader<ChildStdout>,
    stdin: &mut ChildStdin,
    expected_id: u64,
) -> Result<Value, RequestError> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await.map_err(|error| {
            RequestError::transport(format!("read from the MCP server: {error}"))
        })?;
        if read == 0 {
            return Err(RequestError::transport(
                "the MCP server closed its output before answering",
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            // Servers occasionally print a banner on stdout before speaking
            // JSON-RPC. Skipping the line is recoverable; failing the request
            // over someone's log message is not worth it.
            continue;
        };
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if let Some(id) = value.get("id").cloned() {
                send_message(
                    stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("Zest does not implement {method}"),
                        },
                    }),
                )
                .await?;
            }
            continue;
        }
        if value.get("id") != Some(&json!(expected_id)) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the MCP server rejected the request");
            return Err(RequestError::fatal(clip(message, MAX_ERROR_CHARS)));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

/// Flatten an MCP `content` array into the text the model sees.
///
/// Non-text blocks are named rather than dropped, so a tool that answers with
/// an image does not look like it answered with nothing.
fn text_from_content(result: &Value) -> String {
    let Some(blocks) = result.get("content").and_then(Value::as_array) else {
        // A server that answers with structured content and no blocks is
        // within spec; give the model the JSON rather than an empty string.
        return match result.get("structuredContent") {
            Some(value) => value.to_string(),
            None => String::new(),
        };
    };
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            Some(kind) => parts.push(format!("[{kind} content omitted]")),
            None => {}
        }
    }
    parts.join("\n")
}

/// Keep Zest's provider credentials out of the server process.
///
/// The allow-list is names only. An MCP server that needs a token gets it from
/// the environment the user already set on their machine; writing the value
/// into `zest.toml` would commit a secret.
fn scrub_environment(command: &mut Command, allowed: &[String]) {
    for (name, _) in std::env::vars() {
        let upper = name.to_ascii_uppercase();
        let secretish = ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"]
            .iter()
            .any(|marker| upper.contains(marker));
        if name.eq_ignore_ascii_case(SESSION_ENV)
            || (secretish
                && !allowed
                    .iter()
                    .any(|allow| allow.eq_ignore_ascii_case(&name)))
        {
            command.env_remove(name);
        }
    }
    // This variable is a serialized OAuth session, not a conventional
    // KEY/TOKEN name. It must never be revived by an explicit MCP allow-list.
    command.env_remove(SESSION_ENV);
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… clipped", &text[..end])
}

/// One tool on one server, as the model sees it.
pub struct McpTool {
    server: Arc<McpServer>,
    remote_name: String,
    name: String,
    description: String,
    input_schema: Value,
}

impl McpTool {
    pub fn new(server: Arc<McpServer>, def: McpToolDef) -> Self {
        let name = qualified_tool_name(server.id(), &def.name);
        let description = if def.description.trim().is_empty() {
            format!("`{}` on the {} MCP server.", def.name, server.id())
        } else {
            format!("{} (via the {} MCP server)", def.description, server.id())
        };
        Self {
            server,
            remote_name: def.name,
            name,
            description,
            input_schema: def.input_schema,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    /// Exec, always. The server is a separate process running code Zest cannot
    /// read, so there is no version of this call the harness can vouch for.
    fn risk(&self) -> ToolRisk {
        ToolRisk::Exec
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        let arguments = serde_json::to_string_pretty(&input).unwrap_or_else(|_| input.to_string());
        Ok(PreparedToolCall::plain_with_preview(
            self.name(),
            self.risk(),
            input,
            ApprovalPreview {
                path: format!("{} · {}", self.server.id(), self.remote_name),
                summary: format!(
                    "Run {} on the {} MCP server",
                    self.remote_name,
                    self.server.id()
                ),
                diff: clip(&arguments, 4_000),
            },
        ))
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let body = self.server.call_tool(&self.remote_name, input).await?;
        Ok(ToolOutcome::text(if body.trim().is_empty() {
            format!("{} returned no content.", self.remote_name)
        } else {
            body
        }))
    }
}

/// Start a server, ask what it offers, and stop it again.
///
/// Used by the desktop when a server is saved or checked. It deliberately does
/// not keep the process: the point is to answer "does this work, and what does
/// it expose", and holding a child open for a settings screen would leak one
/// process per check.
pub async fn probe_server(
    id: &str,
    config: &McpServerConfig,
    cwd: impl Into<PathBuf>,
) -> Result<Vec<McpToolDef>, String> {
    let server = McpServer::new(id, config.clone(), cwd);
    let tools = server.list_tools().await;
    // Dropping the session kills the child (`kill_on_drop`), including when
    // the listing failed part-way.
    drop(server);
    tools
}

/// Register the enabled servers' cached tools.
///
/// Returns the servers that contributed nothing, so a front-end can say
/// "configured but never checked" instead of silently offering no tools.
pub fn register_mcp_tools(
    registry: &mut ToolRegistry,
    servers: &BTreeMap<String, McpServerConfig>,
    catalog: &McpCatalog,
    cwd: &Path,
) -> Vec<String> {
    let mut uncatalogued = Vec::new();
    let mut started = 0usize;
    for (id, config) in servers {
        if !config.enabled {
            continue;
        }
        if started >= MAX_MCP_SERVERS {
            uncatalogued.push(id.clone());
            continue;
        }
        let tools = catalog.tools(id);
        if tools.is_empty() {
            uncatalogued.push(id.clone());
            continue;
        }
        started += 1;
        let server = Arc::new(McpServer::new(id, config.clone(), cwd));
        for def in tools.iter().take(MAX_TOOLS_PER_SERVER) {
            registry.register(Arc::new(McpTool::new(server.clone(), def.clone())));
        }
    }
    uncatalogued
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn config() -> McpServerConfig {
        McpServerConfig {
            command: "node".into(),
            args: Vec::new(),
            env_vars: Vec::new(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    #[test]
    fn tool_names_survive_characters_the_provider_rejects() {
        assert_eq!(
            qualified_tool_name("supa base", "search.web"),
            "mcp__supa_base__search_web"
        );
    }

    #[test]
    fn tool_names_stay_within_the_provider_limit() {
        let name = qualified_tool_name(&"s".repeat(40), &"t".repeat(40));
        assert_eq!(name.len(), 64);
    }

    /// A server the user has never checked must not silently contribute zero
    /// tools: the caller needs to know so the UI can say why.
    #[test]
    fn an_uncatalogued_server_is_reported_rather_than_registered() {
        let mut registry = ToolRegistry::new();
        let mut servers = BTreeMap::new();
        servers.insert("github".to_string(), config());
        let uncatalogued = register_mcp_tools(
            &mut registry,
            &servers,
            &McpCatalog::default(),
            Path::new("."),
        );
        assert_eq!(uncatalogued, vec!["github".to_string()]);
        assert!(registry.names().is_empty());
    }

    #[test]
    fn a_disabled_server_registers_nothing_and_is_not_reported() {
        let mut registry = ToolRegistry::new();
        let mut servers = BTreeMap::new();
        servers.insert(
            "github".to_string(),
            McpServerConfig {
                enabled: false,
                ..config()
            },
        );
        let mut catalog = McpCatalog::default();
        catalog.set(
            "github",
            vec![McpToolDef {
                name: "search".into(),
                description: "Search".into(),
                input_schema: empty_schema(),
            }],
        );
        assert!(register_mcp_tools(&mut registry, &servers, &catalog, Path::new(".")).is_empty());
        assert!(registry.names().is_empty());
    }

    #[test]
    fn cached_tools_register_under_the_prefixed_name() {
        let mut registry = ToolRegistry::new();
        let mut servers = BTreeMap::new();
        servers.insert("github".to_string(), config());
        let mut catalog = McpCatalog::default();
        catalog.set(
            "github",
            vec![McpToolDef {
                name: "search_issues".into(),
                description: "Search issues".into(),
                input_schema: empty_schema(),
            }],
        );
        assert!(register_mcp_tools(&mut registry, &servers, &catalog, Path::new(".")).is_empty());
        assert_eq!(registry.names(), vec!["mcp__github__search_issues"]);
        assert_eq!(
            registry.risk("mcp__github__search_issues"),
            Some(ToolRisk::Exec)
        );
    }

    #[test]
    fn text_blocks_are_joined_and_other_blocks_are_named() {
        let result = json!({
            "content": [
                { "type": "text", "text": "first" },
                { "type": "image", "data": "…" },
                { "type": "text", "text": "second" },
            ]
        });
        assert_eq!(
            text_from_content(&result),
            "first\n[image content omitted]\nsecond"
        );
    }

    #[test]
    fn structured_content_is_used_when_there_are_no_blocks() {
        let result = json!({ "structuredContent": { "ok": true } });
        assert_eq!(text_from_content(&result), "{\"ok\":true}");
    }

    #[test]
    fn an_absent_schema_becomes_an_object_the_provider_accepts() {
        let def: McpToolDef = serde_json::from_value(json!({ "name": "run" })).unwrap();
        assert_eq!(def.input_schema, empty_schema());
    }

    #[test]
    fn clipping_never_splits_a_character() {
        let text = "é".repeat(10);
        let clipped = clip(&text, 5);
        assert!(clipped.starts_with("éé"));
        assert!(clipped.ends_with("clipped"));
    }

    #[test]
    fn a_missing_server_reports_which_one_failed() {
        let server = McpServer::new(
            "ghost",
            McpServerConfig {
                command: "zest-mcp-server-that-does-not-exist".into(),
                ..config()
            },
            ".",
        );
        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(server.list_tools())
            .expect_err("a missing command cannot list tools");
        assert!(error.contains("ghost"), "{error}");
    }

    #[tokio::test]
    async fn bounded_fixture_covers_handshake_call_denial_timeout_and_malformed_output() {
        let node_available = std::process::Command::new(resolve_program("node"))
            .arg("--version")
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !node_available {
            // The desktop verification job provisions Node; keep this unit
            // test harmless for minimal Rust-only environments.
            return;
        }

        let script = tempfile::NamedTempFile::new().unwrap();
        let body = r#"
const mode = process.argv[2];
const readline = require("readline");
const rl = readline.createInterface({ input: process.stdin });
function reply(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n");
}
rl.on("line", (line) => {
  const request = JSON.parse(line);
  if (request.method === "initialize") {
    reply(request.id, { protocolVersion: "2025-06-18", capabilities: {}, serverInfo: { name: "fixture" } });
  } else if (request.method === "tools/list") {
    reply(request.id, {
      tools: [{
        name: "echo",
        description: "env-session=" + Boolean(process.env.ZEST_CODEX_OAUTH_SESSION) + ";env-key=" + Boolean(process.env.ZEST_MCP_FIXTURE_KEY),
        inputSchema: { type: "object" }
      }]
    });
  } else if (request.method === "tools/call") {
    if (mode === "timeout") return;
    if (mode === "deny") {
      process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { message: "fixture denied" } }) + "\n");
      return;
    }
    if (mode === "malformed") process.stdout.write("this is not JSON-RPC\n");
    const text = mode === "large" ? "x".repeat(300000) : "fixture success";
    reply(request.id, { content: [{ type: "text", text }] });
  }
});
"#;
        script.as_file().write_all(body.as_bytes()).unwrap();
        std::env::set_var("ZEST_MCP_FIXTURE_KEY", "fixture");
        std::env::set_var(crate::codex_oauth::SESSION_ENV, "fixture-session");

        let mut success_config = config();
        success_config.args = vec![script.path().display().to_string(), "success".into()];
        success_config.timeout_secs = 1;
        let success = McpServer::new("fixture", success_config, ".");
        let tools = success.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert!(tools[0].description.contains("env-session=false"));
        assert!(tools[0].description.contains("env-key=false"));
        assert_eq!(
            success.call_tool("echo", json!({})).await.unwrap(),
            "fixture success"
        );

        let mut large_config = config();
        large_config.args = vec![script.path().display().to_string(), "large".into()];
        let large = McpServer::new("large", large_config, ".");
        let clipped = large.call_tool("echo", json!({})).await.unwrap();
        assert!(clipped.ends_with("clipped"));
        assert!(clipped.len() <= MAX_RESULT_BYTES + 32);

        let mut denied_config = config();
        denied_config.args = vec![script.path().display().to_string(), "deny".into()];
        let denied = McpServer::new("denied", denied_config, ".");
        assert!(denied
            .call_tool("echo", json!({}))
            .await
            .unwrap_err()
            .contains("fixture denied"));

        let mut malformed_config = config();
        malformed_config.args = vec![script.path().display().to_string(), "malformed".into()];
        let malformed = McpServer::new("malformed", malformed_config, ".");
        assert_eq!(
            malformed.call_tool("echo", json!({})).await.unwrap(),
            "fixture success"
        );

        let mut timeout_config = config();
        timeout_config.args = vec![script.path().display().to_string(), "timeout".into()];
        timeout_config.timeout_secs = 1;
        let timeout = McpServer::new("timeout", timeout_config, ".");
        let error = timeout.call_tool("echo", json!({})).await.unwrap_err();
        assert!(error.contains("did not answer"), "{error}");

        std::env::remove_var("ZEST_MCP_FIXTURE_KEY");
        std::env::remove_var(crate::codex_oauth::SESSION_ENV);
    }
}
