//! Managing the `[mcp.<id>]` servers in `zest.toml` from a settings screen: list them with
//! whether their tools will reach a chat, add or replace one (HTTP header secrets go to the OS
//! credential manager, never into the file), turn one on or off, remove one, and check one by
//! starting it and listing its tools.
//!
//! A server contributes tools only once a check has cached its tool list ([`McpCatalog`]), so
//! saving or turning on a server also checks it. That check is best effort: the entry is on
//! disk either way, and a server that is not installed yet shows as "Not checked" instead of
//! turning a successful save into an error.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{Config, McpServerConfig, CONFIG_FILE};
use crate::config_edit::{self, McpServerInput};
use crate::mcp::{probe_server, McpCatalog, McpServerCatalog, MAX_MCP_SERVERS};

/// One server as a settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    /// Header name to environment variable name.
    pub headers: BTreeMap<String, String>,
    /// Header names whose value is saved in the credential manager.
    pub saved_headers: Vec<String>,
    pub env_vars: Vec<String>,
    pub enabled: bool,
    pub timeout_secs: u64,
    /// Tool names from the last successful check. Empty means never checked.
    pub tools: Vec<String>,
    /// "Off", "Not checked" or "Ready".
    pub status: String,
    pub detail: String,
}

/// What a settings form submits for one server. The whole entry is written every time, so
/// a field the user cleared is cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerForm {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    /// Header name to environment variable name.
    pub headers: BTreeMap<String, String>,
    /// Header name to a value typed in the form. Blank keeps the saved value.
    pub header_secrets: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
    pub enabled: bool,
    pub timeout_secs: Option<u64>,
}

/// The result of starting a server and asking for its tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCheck {
    pub ok: bool,
    pub detail: String,
    pub tools: Vec<String>,
}

/// The file MCP edits go to: the project's `zest.toml` when it has one, otherwise the user
/// config, which is created if missing.
pub fn editable_config_path(config_root: &Path) -> Result<PathBuf, String> {
    let project = config_root.join(CONFIG_FILE);
    if project.is_file() {
        return Ok(project);
    }
    crate::ensure_user_config()
        .map_err(|e| e.to_string())?
        .or_else(crate::user_config_path)
        .ok_or_else(|| "could not locate the user config directory".to_string())
}

/// "Project zest.toml" or "User zest.toml", for saying which file a screen edits.
pub fn config_scope(config_root: &Path) -> &'static str {
    if config_root.join(CONFIG_FILE).is_file() {
        "Project zest.toml"
    } else {
        "User zest.toml"
    }
}

/// The configured servers with their cached tools, in id order.
pub fn list(config: &Config, catalog: &McpCatalog) -> Vec<McpServerRow> {
    config
        .mcp
        .iter()
        .map(|(id, server)| {
            let cached = catalog.servers.get(id);
            let tools: Vec<String> = cached
                .map(|entry| entry.tools.iter().map(|tool| tool.name.clone()).collect())
                .unwrap_or_default();
            let (status, detail) = status(server.enabled, &tools, cached, now_secs());
            McpServerRow {
                id: id.clone(),
                command: server.command.clone(),
                args: server.args.clone(),
                url: server.http_url().unwrap_or("").to_string(),
                headers: server.headers.clone(),
                saved_headers: server.header_credentials.keys().cloned().collect(),
                env_vars: server.env_vars.clone(),
                enabled: server.enabled,
                timeout_secs: server.timeout_secs,
                tools,
                status,
                detail,
            }
        })
        .collect()
}

/// The distinction that matters is not "configured" but "will a chat get its tools": a
/// server that has never answered contributes nothing, however correct its entry looks.
fn status(
    enabled: bool,
    tools: &[String],
    cached: Option<&McpServerCatalog>,
    now: u64,
) -> (String, String) {
    if !enabled {
        return (
            "Off".into(),
            "Turn it on to load its tools into new chats.".into(),
        );
    }
    if tools.is_empty() {
        return (
            "Not checked".into(),
            "Check the server to load the tools it offers.".into(),
        );
    }
    let checked = cached
        .map(|entry| entry.fetched_at)
        .filter(|seconds| *seconds > 0)
        .map(|seconds| format!(" · checked {}", age_label(now.saturating_sub(seconds))))
        .unwrap_or_default();
    let plural = if tools.len() == 1 { "" } else { "s" };
    (
        "Ready".into(),
        format!("{} tool{plural}{checked}", tools.len()),
    )
}

fn age_label(age: u64) -> String {
    match age {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", age / 60),
        3_600..=86_399 => format!("{}h ago", age / 3_600),
        _ => format!("{}d ago", age / 86_400),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Stable, non-secret credential account for one header of one server in one config file.
/// The path is part of it so two projects with the same server id never share a token.
pub fn header_credential_account(config_path: &Path, id: &str, header: &str) -> String {
    let material = format!(
        "{}\0{}\0{}",
        config_path.to_string_lossy(),
        id,
        header.to_ascii_lowercase()
    );
    format!("mcp-header:{}", blake3::hash(material.as_bytes()).to_hex())
}

/// Splits an argument line the way a shell does for the simple cases: whitespace separates,
/// and single or double quotes keep a run together (a path with a space survives). Nothing
/// else is interpreted, because the command is spawned directly, not through a shell.
pub fn parse_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in line.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(ch);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

fn config_from_input(input: &McpServerInput) -> McpServerConfig {
    let url = input.url.trim();
    McpServerConfig {
        command: input.command.clone(),
        args: input.args.clone(),
        url: (!url.is_empty()).then(|| url.to_string()),
        headers: input.headers.clone(),
        header_credentials: input.header_credentials.clone(),
        env_vars: input.env_vars.clone(),
        enabled: input.enabled,
        timeout_secs: input.timeout_secs,
    }
}

fn clean(mut input: McpServerInput) -> McpServerInput {
    input.id = input.id.trim().to_string();
    input.command = input.command.trim().to_string();
    input.args = input
        .args
        .into_iter()
        .map(|arg| arg.trim().to_string())
        .filter(|arg| !arg.is_empty())
        .collect();
    input.url = input.url.trim().to_string();
    input.env_vars = input
        .env_vars
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    input
}

fn input_from(id: &str, server: &McpServerConfig, enabled: bool) -> McpServerInput {
    McpServerInput {
        id: id.to_string(),
        command: server.command.clone(),
        args: server.args.clone(),
        url: server.http_url().unwrap_or("").to_string(),
        headers: server.headers.clone(),
        header_credentials: server.header_credentials.clone(),
        env_vars: server.env_vars.clone(),
        enabled,
        timeout_secs: server.timeout_secs,
    }
}

fn load(config_root: &Path) -> Result<Config, String> {
    Config::find(config_root).map_err(|e| e.to_string())
}

/// Writes one entry, refusing a new server past [`MAX_MCP_SERVERS`].
fn write(config_root: &Path, input: &McpServerInput) -> Result<(), String> {
    let current = load(config_root)?;
    if !current.mcp.contains_key(input.id.as_str()) && current.mcp.len() >= MAX_MCP_SERVERS {
        return Err(format!(
            "Zest starts at most {MAX_MCP_SERVERS} MCP servers. Remove one first."
        ));
    }
    let path = editable_config_path(config_root)?;
    config_edit::upsert_mcp_server(&path, input)
}

/// Adds or replaces a server, then checks it. A value typed for a header replaces an
/// environment-backed header of the same (case-insensitive) name and is stored in the
/// credential manager; a blank value keeps what was saved. Credentials the entry no longer
/// uses are deleted.
pub async fn save(
    config_root: &Path,
    project_root: &Path,
    form: McpServerForm,
) -> Result<(), String> {
    let id = form.id.trim().to_string();
    let existing = load(config_root)?.mcp.get(&id).cloned();
    let previous_accounts: HashSet<String> = existing
        .as_ref()
        .map(|server| server.header_credentials.values().cloned().collect())
        .unwrap_or_default();
    let mut input = clean(McpServerInput {
        id,
        command: form.command,
        args: form.args,
        url: form.url,
        headers: form.headers,
        header_credentials: existing
            .as_ref()
            .map(|server| server.header_credentials.clone())
            .unwrap_or_default(),
        env_vars: form.env_vars,
        enabled: form.enabled,
        timeout_secs: form.timeout_secs.unwrap_or(120),
    });
    let typed: Vec<(String, String)> = form
        .header_secrets
        .into_iter()
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .collect();
    if !typed.is_empty() && input.url.is_empty() {
        return Err("Saved header values are only used with an HTTP URL.".into());
    }
    if input.url.is_empty() {
        input.header_credentials.clear();
    }
    let path = editable_config_path(config_root)?;
    let mut to_store = Vec::new();
    for (name, value) in typed {
        input
            .headers
            .retain(|header, _| !header.eq_ignore_ascii_case(&name));
        input
            .header_credentials
            .retain(|header, _| !header.eq_ignore_ascii_case(&name));
        let account = header_credential_account(&path, &input.id, &name);
        input.header_credentials.insert(name, account.clone());
        to_store.push((account, value));
    }
    // A hand-edited file may name both sources for one header; the environment mapping wins
    // unless the form just replaced it with a saved value.
    let env_headers: Vec<String> = input.headers.keys().cloned().collect();
    input.header_credentials.retain(|header, _| {
        !env_headers
            .iter()
            .any(|env| env.eq_ignore_ascii_case(header))
    });
    config_from_input(&input).validate(&input.id)?;
    for (account, value) in &to_store {
        crate::credentials::set(account, value)?;
    }
    write(config_root, &input)?;
    let current: HashSet<&String> = input.header_credentials.values().collect();
    for account in previous_accounts
        .iter()
        .filter(|account| !current.contains(account))
    {
        let _ = crate::credentials::delete(account);
    }
    refresh_catalog(&input.id, &config_from_input(&input), project_root).await;
    Ok(())
}

/// Turns a server on or off. Turning it on checks it, since that is when its tools must exist.
pub async fn set_enabled(
    config_root: &Path,
    project_root: &Path,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let id = id.trim();
    let server = load(config_root)?
        .mcp
        .get(id)
        .cloned()
        .ok_or_else(|| format!("{id} is not configured."))?;
    let input = clean(input_from(id, &server, enabled));
    write(config_root, &input)?;
    refresh_catalog(id, &config_from_input(&input), project_root).await;
    Ok(())
}

/// Removes a server, its saved header values and its cached tool list. The file edit is
/// what counts; credential and catalog cleanup are best effort.
pub fn remove(config_root: &Path, id: &str) -> Result<(), String> {
    let id = id.trim();
    let accounts: Vec<String> = load(config_root)?
        .mcp
        .get(id)
        .map(|server| server.header_credentials.values().cloned().collect())
        .unwrap_or_default();
    let path = editable_config_path(config_root)?;
    config_edit::remove_mcp_server(&path, id)?;
    for account in accounts {
        let _ = crate::credentials::delete(&account);
    }
    // Otherwise the cached tools would come back if another entry later reused the id.
    let mut catalog = McpCatalog::load();
    if catalog.servers.contains_key(id) {
        catalog.forget(id);
        let _ = catalog.save();
    }
    Ok(())
}

/// Starts the server, lists its tools, caches them, and stops it again.
pub async fn check(config_root: &Path, project_root: &Path, id: &str) -> Result<McpCheck, String> {
    let id = id.trim();
    let server = load(config_root)?
        .mcp
        .get(id)
        .cloned()
        .ok_or_else(|| format!("{id} is not configured."))?;
    match probe_server(id, &server, project_root).await {
        Ok(tools) => {
            let names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
            let mut catalog = McpCatalog::load();
            catalog.set(id, tools);
            catalog.save()?;
            let count = catalog.tools(id).len();
            let plural = if count == 1 { "" } else { "s" };
            Ok(McpCheck {
                ok: true,
                detail: if count == 0 {
                    "Server answered but offered no tools.".to_string()
                } else {
                    format!("Ready — {count} tool{plural}.")
                },
                tools: names,
            })
        }
        Err(error) => Ok(McpCheck {
            ok: false,
            detail: error,
            tools: Vec::new(),
        }),
    }
}

async fn refresh_catalog(id: &str, server: &McpServerConfig, project_root: &Path) {
    if !server.enabled {
        return;
    }
    if let Ok(tools) = probe_server(id, server, project_root).await {
        let mut catalog = McpCatalog::load();
        catalog.set(id, tools);
        let _ = catalog.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_split_on_whitespace_and_keep_quoted_runs() {
        assert_eq!(parse_args(""), Vec::<String>::new());
        assert_eq!(parse_args("  -y  @scope/server "), ["-y", "@scope/server"]);
        assert_eq!(
            parse_args(r#"--root "C:\Program Files\x" 'a b' """#),
            ["--root", r"C:\Program Files\x", "a b", ""]
        );
    }

    #[test]
    fn status_says_whether_a_chat_gets_the_tools() {
        let off = status(false, &["a".into()], None, 0);
        assert_eq!(off.0, "Off");
        let unchecked = status(true, &[], None, 0);
        assert_eq!(unchecked.0, "Not checked");
        let cached = McpServerCatalog {
            tools: Vec::new(),
            fetched_at: 1_000,
        };
        let ready = status(
            true,
            &["a".into(), "b".into()],
            Some(&cached),
            1_000 + 7_200,
        );
        assert_eq!(ready, ("Ready".into(), "2 tools · checked 2h ago".into()));
        assert_eq!(status(true, &["a".into()], None, 0).1, "1 tool");
    }

    #[test]
    fn credential_accounts_differ_per_config_file_and_ignore_header_case() {
        let a = header_credential_account(Path::new("/p/zest.toml"), "github", "Authorization");
        assert_eq!(
            a,
            header_credential_account(Path::new("/p/zest.toml"), "github", "authorization")
        );
        assert_ne!(
            a,
            header_credential_account(Path::new("/q/zest.toml"), "github", "Authorization")
        );
        assert!(a.starts_with("mcp-header:"));
    }

    /// Unlikely to name a real server, since remove() also forgets it in the user's catalog.
    const ID: &str = "zest-test-docs-4f1a";

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE), "").unwrap();
        dir
    }

    #[tokio::test]
    async fn save_writes_the_whole_entry_to_the_project_config_and_remove_deletes_it() {
        let dir = project();
        let root = dir.path();
        let form = McpServerForm {
            id: format!(" {ID} "),
            command: " missing-mcp-server-for-test ".into(),
            args: vec!["--stdio".into(), "  ".into()],
            env_vars: vec![" DOCS_TOKEN ".into()],
            enabled: false,
            timeout_secs: Some(30),
            ..Default::default()
        };
        save(root, root, form).await.unwrap();
        let rows = list(&load(root).unwrap(), &McpCatalog::default());
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            (row.id.as_str(), row.command.as_str()),
            (ID, "missing-mcp-server-for-test")
        );
        assert_eq!(row.args, ["--stdio"]);
        assert_eq!(row.env_vars, ["DOCS_TOKEN"]);
        assert_eq!(
            (row.enabled, row.timeout_secs, row.status.as_str()),
            (false, 30, "Off")
        );
        assert_eq!(config_scope(root), "Project zest.toml");

        remove(root, ID).unwrap();
        assert!(load(root).unwrap().mcp.is_empty());
    }

    #[tokio::test]
    async fn a_header_value_needs_an_http_url() {
        let dir = project();
        let root = dir.path();
        let form = McpServerForm {
            id: "local".into(),
            command: "server".into(),
            header_secrets: BTreeMap::from([("Authorization".into(), "Bearer x".into())]),
            ..Default::default()
        };
        let err = save(root, root, form).await.unwrap_err();
        assert!(err.contains("HTTP URL"), "{err}");
        assert!(load(root).unwrap().mcp.is_empty(), "nothing is written");
    }

    #[tokio::test]
    async fn set_enabled_rewrites_only_the_switch() {
        let dir = project();
        let root = dir.path();
        let form = McpServerForm {
            id: ID.into(),
            command: "missing-mcp-server-for-test".into(),
            args: vec!["a".into()],
            enabled: false,
            ..Default::default()
        };
        save(root, root, form).await.unwrap();
        set_enabled(root, root, ID, true).await.unwrap();
        let config = load(root).unwrap();
        let server = &config.mcp[ID];
        assert!(server.enabled);
        assert_eq!(server.args, ["a"]);
        let missing = set_enabled(root, root, "zest-test-missing", true)
            .await
            .unwrap_err();
        assert!(missing.contains("not configured"));
    }
}
