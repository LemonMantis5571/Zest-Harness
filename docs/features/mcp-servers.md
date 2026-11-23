---
title: MCP servers
summary: Users add, check, switch and remove outbound MCP servers (local command or Streamable HTTP URL) in Customize > MCP servers or zest.toml, and the parent chat calls their tools behind the approval card.
paths:
  - crates/core/src/mcp.rs
  - crates/core/src/mcp/http.rs
  - crates/core/src/mcp_admin.rs
  - crates/desktop/src/workspace/mcp.rs
  - crates/desktop/src/interaction.rs
verify:
  - cargo test -p zest-core --lib -- mcp::
  - cargo test -p zest-core --lib -- mcp_admin::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::mcp::
---

# MCP servers

See docs/MCP.md for the user-facing guide.

## Behavior
- `[mcp.<id>]` in `zest.toml` is either `command` + `args` (+ `env_vars`) or `url`
  (+ `headers`, `header_credentials`), never both; `timeout_secs` 1-600 bounds one
  call. `enabled = false` keeps the entry but loads nothing.
- Customize > MCP servers edits the project's `zest.toml` when it has one, otherwise
  the user's (the page says which). Each server shows its name, a status pill (Off,
  Not checked, Ready), the command line or URL, and a detail line ("3 tools · checked
  2h ago", or the last failed check's error), with an on/off switch, **Check**,
  **Edit** and **Remove** (which asks first).
- **Add server** opens a form: name, Command or HTTP URL, command and arguments (one
  line, quotes keep a path with spaces together), or URL and Access token (sent as the
  `Authorization` header), environment variables to pass (names only) and timeout.
  **Add GitHub** fills in GitHub's hosted server; paste a personal access token.
  Editing keeps the name and any header-to-variable mappings the file already has;
  a blank Access token keeps the saved one. **Save and check** writes the whole entry.
- Saving, or switching a server on, also checks it: Zest starts it, lists its tools
  and caches them, which is what lets chats use them. A check that fails does not undo
  the save; the server shows "Not checked" and **Check** reports the error as a toast.
- Changes reach new chats; open chats keep the servers they started with.
  Customize > Skills & MCP lists enabled servers as slash commands.
- Tools are registered as `mcp__<server>__<tool>` (sanitized, truncated to 64
  chars) with the server's description suffixed "(via the <id> MCP server)".
  At most 12 servers and 48 tools per server.
- Every MCP call is `Exec` risk and never auto-eligible: it asks in Manual,
  Accept edits, and Auto, and is refused in Plan. Bypass would run it, but the
  desktop app's Tool permissions offer only Manual, Plan, Accept edits and Auto.
- The desktop approval card is titled "Allow <tool>? (Exec)" with the qualified
  `mcp__<server>__<tool>` name, followed by the preview: `server · tool`,
  "Run <tool> on the <server> MCP server", and the argument JSON. Allow runs the
  call once; there is no allow-for-session choice.
- Registered only for a parent chat whose agent loop Zest owns (Anthropic key,
  OpenAI-compatible endpoints). Claude Code / Codex chats use their own MCP
  config via `allow_mcp` instead.
- Tools come from the cached catalogue (`~/.zest/mcp-catalog.json`), never by
  handshaking at chat start. An enabled server with no cached tools contributes
  nothing, and each turn in the desktop app shows a notice naming it.
- A local server process starts on first tool call and lives for the session;
  a URL server is called per request.
- Secrets stay out of `zest.toml`: `env_vars` and `headers` hold environment
  variable **names**; `header_credentials` maps a header to an OS credential
  manager account whose value is read at call time (entries written by an
  Customize > MCP servers use `mcp-header:<blake3>` accounts, from the config path, server and header name). Credential-looking
  variables are scrubbed from a local server's environment unless listed in
  `env_vars`. Saving a typed token replaces an environment mapping for the same header;
  tokens the entry no longer uses, and all of a removed server's, are deleted from
  the credential manager.
- An enabled server id is also a `/id` slash command (see
  skills-and-slash-commands).

## How it works
1. `register_mcp_tools(&mut tools, &config.mcp, &McpCatalog::load(), &root)`
   runs in `RuntimeBuilder::build` (runtime.rs), which the desktop calls for
   every turn, creating one lazy `McpServer` per server and an `McpTool` per
   cached tool. Servers without cached tools become the session warning, shown
   as a notice in the chat.
2. `probe_server` (mcp.rs, exported as `zest_core::probe_mcp_server`) starts a
   server, lists tools, and drops it; results go into `McpCatalog`.
3. Settings: `mcp_admin.rs` holds the provider-independent part of the page.
   `list` builds rows with `status`; `save` cleans the form, moves typed header
   values into the credential manager (`header_credential_account`), validates,
   writes with `config_edit::upsert_mcp_server` (refusing a 13th server), deletes
   unused credentials and refreshes the catalogue; `set_enabled`, `remove` and
   `check` do the rest. The desktop page (`workspace/mcp.rs`) runs them on the
   Tokio executor, then reloads the rows and `Project::reload_config`.
4. `McpTool::prepare` builds the preview (`server · tool`, a summary, argument
   JSON as the diff). The desktop's `Interactions::decide` (`interaction.rs`)
   turns it into the approval card and returns `AllowOnce` or `Deny`.
5. `run` -> `McpServer::call_tool` -> `request` connects on first use: stdio
   child (`kill_on_drop`, `scrub_environment`) or `http::connect`.
6. Protocol: `server/discover` is probed first; a 2026-era server stays on
   per-request `_meta` (`MODERN_PROTOCOL_VERSION`), a 2025 server gets the
   `initialize` handshake. HTTP sends `MCP-Protocol-Version`, `Mcp-Method`,
   `Mcp-Name`, and `Mcp-Param-*` headers from `x-mcp-header` schema annotations;
   JSON and SSE replies both work. Results are capped at 256 KiB.

## Verify
`mcp::tests` spawns small Node fixture servers; `mcp::http::tests` covers the
HTTP transport and headers. `mcp_admin::tests` cover argument splitting, status
text, credential account ids, and saving, switching and removing entries in a
temporary project config (with servers that are off or missing, so no check runs).
By hand: Customize > MCP servers > Add server, a command such as
`npx -y @modelcontextprotocol/server-everything`, then **Save and check**.

## Pitfalls
- Two remote tool names that differ only in stripped characters collapse to one
  Zest name; the first registered wins.
- HTTP tools whose schemas have invalid or duplicate `x-mcp-header` annotations
  are dropped from the tool list.
- A stale catalogue entry fails at call time with the server's error.
- One connection per server, behind a mutex: concurrent calls to the same server
  serialize.
