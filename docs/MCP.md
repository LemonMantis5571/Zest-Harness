# MCP servers

Zest can call tools from Model Context Protocol servers you configure yourself.
Add one in **Customize > MCPs**, or write it into `zest.toml`. That outbound
path is not the coordinator daemon.

## Four different things called MCP or headless

`zest serve` is an **inbound** MCP server. Any host that can spawn a process
and hold `ZEST_SERVE_TOKEN` can call the delegation tools. See
[SERVE.md](SERVE.md).

`zest run --jsonl` is one deny-only parent turn for CI and editors. It is not
a coordinator and does not host MCP.

`[agents.<id>].mode = "headless"` configures an **external worker CLI** that
Zest launches for a feature card. It is not Zest running without a UI.

`allow_mcp` on a `claude_code` / `codex_cli` provider, or on an `[agents.<id>]`
worker, lets **that CLI** use the MCP servers in its own configuration. Zest
neither sees those servers nor approves the individual calls, which is why the
switch is opt-in.

`[mcp.<id>]` is Zest's own **outbound** client. A subscription CLI runs its own
agent loop and keeps using its own MCP configuration, so Zest's servers are not
registered on such a chat. Everything else — an Anthropic key, or an
OpenAI-compatible endpoint such as DeepSeek — has no CLI behind it, and this is
the only way those chats can reach an MCP server at all.

## Configuration

Customize always shows GitHub as the starter MCP. It stays off until you turn
it on and paste a personal access token. Fresh installs already have the
official remote entry in `zest.toml`; older configs get the same row and write
it on first use.

```toml
[mcp.github]
url = "https://api.githubcopilot.com/mcp/"
enabled = false
timeout_secs = 120
```

A local process is a command instead of a URL. Do not set both.

```toml
[mcp.github-local]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_vars = ["GITHUB_TOKEN"]
enabled = true
timeout_secs = 120
```

Any other Streamable HTTP server is the same shape as GitHub: a URL, then a
token in Customize or a header that names an environment variable.

```toml
[mcp.remote]
url = "https://example.com/mcp"
timeout_secs = 120

[mcp.remote.headers]
Authorization = "MCP_AUTHORIZATION"
```

`MCP_AUTHORIZATION` is an environment variable name. Put the full header value
there, for example `Bearer …`. The secret never goes in `zest.toml`.

For a remote server that uses `Authorization`, the **Customize > MCPs** form
also has an **Access token** password field. Paste the value supplied by the
service. Zest stores that value in the operating system credential manager and
keeps only a non-secret reference in `zest.toml`. When editing an existing
server, leaving the field blank keeps its saved value; entering a new value
replaces it. Removing the server removes its saved credential as well.

- `command` / `args` are passed straight to the operating system. No shell is
  involved, so there is no globbing, piping, or variable expansion.
- `url` is a Streamable HTTP MCP endpoint. Zest POSTs JSON-RPC there, with
  `MCP-Protocol-Version`, `Mcp-Method`, and `Mcp-Name` headers. JSON and SSE
  replies both work.
- `env_vars` lists variable **names** for a local process. Zest keeps
  credential-looking variables out of the server process unless they are named
  here; the values stay in your environment, so `zest.toml` remains safe to
  commit. A value written here is refused rather than saved.
- `headers` on a URL entry is the same idea: header name to environment
  variable name.
- `header_credentials` on a URL entry is managed by the desktop form. Its
  values are credential-manager references, not token values; do not copy a
  secret into this table by hand.
- `enabled = false` keeps the entry and stops the server being used, so turning
  one off does not lose how it was set up.
- `timeout_secs` bounds one tool call. Between 1 and 600.

Zest starts at most 12 servers and registers at most 48 tools from each.

## How a server is used

A tool appears to the model as `mcp__<server>__<tool>` and always carries exec
risk. Zest cannot inspect a local process or a remote URL, so every call goes
through the approval gate and none is ever auto-approved. The desktop shows the
call as `server · tool`, not the qualified name.

Type `/<id>` in chat to point the model at that server. `/haiku write a verse`
is the same idea as a skill command: the transcript keeps what you typed, and
the model is told to use the Haiku MCP tools for the rest of the message.
Enabled servers share the `/` list with personal skills. A skill of the same
name wins.

A local server process starts the first time a chat calls one of its tools, and
stays up for the rest of that session. A chat that never calls an MCP tool never
starts one. A URL is called per request and does not spawn a process.

## Checking a server

The tool list is read when you save a server or press **Check server**, and
cached in `~/.zest/mcp-catalog.json`. Starting a new chat does not re-handshake
with every configured server — that would put a network- or npm-speed delay in
front of your first message.

The practical consequence: a server that has never answered contributes no
tools. Its row says **Not checked**, and a new chat warns that the server loaded
nothing. Press **Check server** after installing or updating one.

Zest treats server tools as capabilities you granted. Only add servers you
trust.
