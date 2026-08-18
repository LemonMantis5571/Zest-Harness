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
