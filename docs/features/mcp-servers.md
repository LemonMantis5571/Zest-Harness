---
title: MCP servers
summary: Users configure outbound MCP servers (local command or Streamable HTTP URL) in zest.toml whose tools the parent chat can call, always behind the approval card.
paths:
  - crates/core/src/mcp.rs
  - crates/core/src/mcp/http.rs
  - crates/desktop/src/interaction.rs
verify:
  - cargo test -p zest-core --lib -- mcp::
---

# MCP servers

See docs/MCP.md for the user-facing guide.

## Behavior
- `[mcp.<id>]` in `zest.toml` is either `command` + `args` (+ `env_vars`) or `url`
  (+ `headers`, `header_credentials`), never both; `timeout_secs` 1-600 bounds one
  call. `enabled = false` keeps the entry but loads nothing.
- The desktop app has no MCP settings page: servers are added, changed, enabled
  and removed by editing `zest.toml`, and the config is read when a project
  window opens. Customize > Skills & MCP only lists enabled servers as slash
  commands.
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
- The desktop app does not refresh the catalogue (there is no Check server
  button and nothing calls `probe_server` on save), so a newly added server has
  no tools in the desktop app until its catalogue entry exists.
- A local server process starts on first tool call and lives for the session;
  a URL server is called per request.
- Secrets stay out of `zest.toml`: `env_vars` and `headers` hold environment
  variable **names**; `header_credentials` maps a header to an OS credential
  manager account whose value is read at call time (entries written by an
  earlier desktop form use `mcp-header:<blake3>` accounts). Credential-looking
  variables are scrubbed from a local server's environment unless listed in
  `env_vars`. The desktop app has no Access token field, so it cannot store or
  delete these secrets.
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
3. `McpTool::prepare` builds the preview (`server · tool`, a summary, argument
   JSON as the diff). The desktop's `Interactions::decide` (`interaction.rs`)
   turns it into the approval card and returns `AllowOnce` or `Deny`.
4. `run` -> `McpServer::call_tool` -> `request` connects on first use: stdio
   child (`kill_on_drop`, `scrub_environment`) or `http::connect`.
5. Protocol: `server/discover` is probed first; a 2026-era server stays on
   per-request `_meta` (`MODERN_PROTOCOL_VERSION`), a 2025 server gets the
   `initialize` handshake. HTTP sends `MCP-Protocol-Version`, `Mcp-Method`,
   `Mcp-Name`, and `Mcp-Param-*` headers from `x-mcp-header` schema annotations;
   JSON and SSE replies both work. Results are capped at 256 KiB.

## Verify
`mcp::tests` spawns small Node fixture servers; `mcp::http::tests` covers the
HTTP transport and headers.

## Pitfalls
- Two remote tool names that differ only in stripped characters collapse to one
  Zest name; the first registered wins.
- HTTP tools whose schemas have invalid or duplicate `x-mcp-header` annotations
  are dropped from the tool list.
- A stale catalogue entry fails at call time with the server's error.
- The missing-tools warning from `runtime.rs` still says "Check it in Customize
  → MCPs", a page the desktop app does not have.
- One connection per server, behind a mutex: concurrent calls to the same server
  serialize.
