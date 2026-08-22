# MCP servers

Zest can call tools from Model Context Protocol servers you configure yourself.
Add one in **Customize > MCPs**, or write it into `zest.toml`.

## Two different things called MCP

`allow_mcp` on a `claude_code` / `codex_cli` provider, or on an `[agents.<id>]`
worker, lets **that CLI** use the MCP servers in its own configuration. Zest
neither sees those servers nor approves the individual calls, which is why the
switch is opt-in.

`[mcp.<id>]` is Zest's own. A subscription CLI runs its own agent loop and keeps
using its own MCP configuration, so Zest's servers are not registered on such a
chat. Everything else — an Anthropic key, or an OpenAI-compatible endpoint such
as DeepSeek — has no CLI behind it, and this is the only way those chats can
reach an MCP server at all.

## Configuration

```toml
[mcp.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env_vars = ["GITHUB_TOKEN"]
enabled = true
timeout_secs = 120
```

- `command` / `args` are passed straight to the operating system. No shell is
  involved, so there is no globbing, piping, or variable expansion.
- `env_vars` lists variable **names**. Zest keeps credential-looking variables
  out of the server process unless they are named here; the values stay in your
  environment, so `zest.toml` remains safe to commit. A value written here is
  refused rather than saved.
- `enabled = false` keeps the entry and stops the server being used, so turning
  one off does not lose how it was set up.
- `timeout_secs` bounds one tool call. Between 1 and 600.

Zest starts at most 12 servers and registers at most 48 tools from each.

## How a server is used

A tool appears to the model as `mcp__<server>__<tool>` and always carries exec
risk: the server is a separate process running code Zest cannot inspect, so
every call goes through the approval gate and none is ever auto-approved.

A server process starts the first time a chat calls one of its tools, and stays
up for the rest of that session. A chat that never calls an MCP tool never
starts one.

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
