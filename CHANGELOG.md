# Changelog

Notable user-facing changes are recorded here. This is a release summary, not
a replacement for the commit history.

## Unreleased

### Removed

- The bundled CLIProxyAPI sidecar, and with it about 349 MiB from the desktop
  installer. Codex and Claude subscriptions are now reached through their own
  CLIs (`codex`, `claude`) instead of a local translating proxy. Zest bundles
  no third-party executable and starts no background server.
- The `kind = "gateway"` provider entry, and the `ZEST_BASE_URL` /
  `ZEST_GATEWAY_KEY` environment override that built one.

### Fixed

- A narrow window squeezed the conversation into a ~300px column: the sidebar
  held its full width and the transcript kept the left gutter that exists only
  to clear the checkpoint rail. Below 768px the sidebar now collapses (restoring
  your own choice when the window grows again), the rail steps aside, and the
  transcript takes the width with even margins.
- The desktop system prompt began with a stray `\` on its own line, which the
  model read as content and the prompt preview showed verbatim. A `"\\` where
  every neighbouring block used `"\`.
- The default-instructions preview in the prompt editor rendered the whole base
  prompt, pushing the box you came to type in off the bottom of the page. It is
  now collapsed by default and scrolls when opened.
- The model picker could offer a different set of models than the provider
  actually accepted. An `anthropic` entry with `model = "claude-haiku-5"` showed
  one model in the picker and accepted two at runtime.
- A Codex turn that failed now reports why. "You've hit your usage limit… try
  again at <date>" used to be replaced by "The provider could not complete the
  request. Try again."

### Changed

- Provider construction moved behind a driver per `kind`
  ([ADR 0005](docs/adr/0005-provider-driver-spi.md)). One visible effect: a
  `codex_cli` provider under any id now gets the built-in Codex model list,
  where previously only an entry literally named `codex` did.
- An existing `kind = "gateway"` config still starts. On load, `codex` becomes
  `codex_cli` (keeping its model and effort lists) and `claude` becomes
  `claude_code` (its model list is dropped, because gateway model ids are not
  CLI aliases); anything else is skipped with a reason. A warning explains each
  change and **`zest.toml` is never rewritten**.
- Both subscription providers run their own agent loop, so Zest's file, shell,
  browser, and delegation tools are not registered on them. A migrated Codex
  provider therefore loses the tool layer it had through the gateway. Use an
  API-key provider (`anthropic`, `openai_compatible`) for Zest's own loop.
  See [ADR 0004](docs/adr/0004-remove-the-bundled-cliproxyapi-gateway.md).
- Claude Code now raises Zest approval cards, with a rendered diff for edits,
  instead of applying changes under `--permission-mode accept_edits`.

## 0.1.0 beta - 2026-08-13

### Added

- Customize panel (`Ctrl+Shift+,`, or Customize in the sidebar) holding MCP
  servers, skills, extras, this project's instructions, and keyboard shortcuts.
  Those last four moved out of Settings, which keeps User, Typography, Provider,
  CLI delegation, and Usage and links across. Customize opens in the
  transcript's place, so the sidebar and header stay put.
- Your profile moved from the chat header to the bottom of the sidebar, and the
  Profile and Usage pages now open inside the chat shell as well. The header is
  free to lead with the project instead of your name.
- A folder button in the chat header shows the active project in the operating
  system's file manager. Hidden for a chat with no project, since Zest's own
  free-chat store is not somewhere you put anything.
- Zest's own MCP servers, configured under `[mcp.<id>]` in `zest.toml` or from
  Customize > MCPs. This is separate from the existing `allow_mcp` switch, which
  lets a CLI use servers Zest never sees: a Claude Code or Codex parent runs its
  own agent loop and keeps using that path, while an API provider — an Anthropic
  key, or an OpenAI-compatible endpoint such as DeepSeek — previously had no way
  to reach an MCP server at all. Tools appear as `mcp__<server>__<tool>`, every
  call is approval-gated as an exec-risk tool, and a server process is started
  only when a chat calls one of its tools. `env_vars` names the environment
  variables a server may keep; values stay out of `zest.toml`.
- Windows-first Tauri desktop app and terminal front-end sharing one Rust core.
- Linux x64 packages for the beta release.
- Approval cards for file writes and shell commands, with diff previews.
- Bundled, pinned CLIProxyAPI sidecar with gateway verification and installer
  checksums.
- OS credential-manager setup for OpenAI-compatible API endpoints, including
  DeepSeek, OpenAI, and local servers; native Anthropic keeps its environment
  variable configuration.
- ACP/headless delegation to already-authenticated Claude Code and Gemini CLI
  workers, with isolated Git worktrees and approval boundaries.
- Workbench activity and outline views, forked conversations, checkpoints, and
  automatic context compaction.
- Usage screen (`Ctrl+Shift+U`, or "Full report" on the profile) with a 7/30/90
  day window, daily spend stacked by provider, and a per-model breakdown.
- Local price book at `<data dir>/zest/prices.toml`, seeded once and never
  rewritten by Zest. Models with no rate are reported as unpriced rather than
  free.
- Usage read back from Claude Code's and Codex's on-disk transcripts, so turns
  run in those CLIs directly are counted even though Zest never sent them.
- JSONL/headless protocol for editor and CI integrations.
- Optional plugins, personal skills, free chats, workspace folders, and
  project-scoped conversations.

### Notes

- Cost figures are estimates at list API rates, not a bill. Zest has no billing
  relationship with any provider, and a subscription does not charge at these
  rates.
- Provider quota is shown only when the provider reports it. Local usage is not
  a subscription balance, and Zest never invents a remaining number.
- Refreshing rates is an unauthenticated GET of one public file. Nothing about
  your usage is sent anywhere, and transcripts are only ever read from disk.
- Official packages do not include optional plugins. Install add-ons separately
  from a trusted source and review their permissions.

### Beta limitations

- Windows and Linux x64 are the packaged targets. macOS and ARM packages are
  not part of this beta release.
- Approved shell commands are not OS-sandboxed.
- Provider usage may be estimated or unavailable when an endpoint does not
  report token counts or quota.
- Configuration and headless protocol details may change before 1.0.
