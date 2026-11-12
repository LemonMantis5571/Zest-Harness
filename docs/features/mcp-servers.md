---
title: MCP servers
summary: Users configure outbound MCP servers (local command or Streamable HTTP URL) whose tools the parent chat can call, always behind the approval card.
paths:
  - crates/core/src/mcp.rs
  - crates/core/src/mcp/http.rs
  - crates/desktop/ui/src/lib/mcpDisplay.ts
  - crates/desktop/ui/src/lib/mcpServerForm.ts
  - crates/desktop/ui/src/components/CustomizePanel.tsx
tests:
  - crates/desktop/ui/src/lib/mcpDisplay.test.ts
  - crates/desktop/ui/src/lib/mcpServerForm.test.ts
verify:
  - cargo test -p zest-core --lib -- mcp::
  - npm run ui:test
---

# MCP servers

See docs/MCP.md for the user-facing guide.

## Behavior
- `[mcp.<id>]` in `zest.toml` is either `command` + `args` (+ `env_vars`) or `url`
  (+ `headers`, `header_credentials`), never both; `timeout_secs` 1-600 bounds one
  call. `enabled = false` keeps the entry but loads nothing.
- Tools are registered as `mcp__<server>__<tool>` (sanitized, truncated to 64
  chars) with the server's description suffixed "(via the <id> MCP server)".
  At most 12 servers and 48 tools per server.
- Every MCP call is `Exec` risk and never auto-eligible: it asks in Manual,
  Accept edits, and Auto (unless allowed for the session), is refused in Plan,
  and runs in Bypass.
- The card and rows show `server · tool` and "Run <tool> on <server>?", never
  the qualified name; an empty `{}` argument preview is hidden.
- Registered only for a parent chat whose agent loop Zest owns (Anthropic key,
  OpenAI-compatible endpoints). Claude Code / Codex chats use their own MCP
  config via `allow_mcp` instead.
- Tools come from the cached catalogue (`~/.zest/mcp-catalog.json`), refreshed on
  save or **Check server**, never by handshaking at chat start. An enabled server
  with no cached tools contributes nothing, its row says "Not checked", and new
  chats get a warning naming it.
- A local server process starts on first tool call and lives for the session;
  a URL server is called per request.
- Secrets stay out of `zest.toml`: `env_vars` and `headers` hold environment
  variable **names**; the Access token field stores the value in the OS
  credential manager under a per-project account (`mcp-header:<blake3>`), and
  removing the server deletes it. Credential-looking variables are scrubbed
  from a local server's environment unless listed in `env_vars`.
- An enabled server id is also a `/id` slash command (see
  skills-and-slash-commands).

## How it works
1. UI: `McpPanel` / `ServerForm` in CustomizePanel.tsx validate with
   `validateMcpServerDraft` (mcpServerForm.ts: `parseArgs`, `parseEnvVars`,
   `parseHeaders`, `normalizeAuthorization` adds `Bearer ` to a pasted GitHub
   PAT), then call backend `saveMcpServer` / `setMcpServerEnabled` /
   `removeMcpServer` / `checkMcpServer`.
2. Tauri commands in crates/desktop/src/lib.rs: `list_mcp_servers` (status via
   `mcp_status`), `save_mcp_server` (stores header secrets with
   `zest_core::credentials::set`, writes via `config_edit::upsert_mcp_server`,
   then `refresh_mcp_catalog` probes), `set_mcp_server_enabled`,
   `remove_mcp_server`, `check_mcp_server`.
3. `probe_server` (mcp.rs) starts the server, lists tools, and drops it;
   results go into `McpCatalog`.
4. runtime.rs calls `register_mcp_tools(&mut tools, &config.mcp,
   &McpCatalog::load(), &root)`, creating one lazy `McpServer` per server and an
   `McpTool` per cached tool.
5. `McpTool::prepare` builds the preview (`server · tool`, argument JSON as the
   diff); `run` -> `McpServer::call_tool` -> `request` connects on first use:
   stdio child (`kill_on_drop`, `scrub_environment`) or `http::connect`.
6. Protocol: `server/discover` is probed first; a 2026-era server stays on
   per-request `_meta` (`MODERN_PROTOCOL_VERSION`), a 2025 server gets the
   `initialize` handshake. HTTP sends `MCP-Protocol-Version`, `Mcp-Method`,
   `Mcp-Name`, and `Mcp-Param-*` headers from `x-mcp-header` schema annotations;
   JSON and SSE replies both work. Results are capped at 256 KiB.

## Verify
`mcp::tests` spawns small Node fixture servers; `mcp::http::tests` covers the
HTTP transport and headers. Fixture mode: `?fixture=1&scenario=approval` shows an
MCP approval card for `Haiku · manifest`.

## Pitfalls
- Two remote tool names that differ only in stripped characters collapse to one
  Zest name; the first registered wins.
- HTTP tools whose schemas have invalid or duplicate `x-mcp-header` annotations
  are dropped from the tool list.
- A stale catalogue entry fails at call time with the server's error; users must
  press Check server after updating a server.
- One connection per server, behind a mutex: concurrent calls to the same server
  serialize.
