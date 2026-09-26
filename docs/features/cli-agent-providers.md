---
title: CLI agent providers
summary: Use Claude Code, the Codex CLI, or Cursor as the main chat provider, with their own sign-in and agent loop and Zest's approval cards.
paths:
  - crates/core/src/provider/claude_code.rs
  - crates/core/src/provider/claude_control.rs
  - crates/core/src/provider/claude_stream.rs
  - crates/core/src/provider/codex_app_server.rs
  - crates/core/src/provider/cursor_acp.rs
  - crates/core/src/provider/cursor_models.rs
  - crates/core/src/provider/session.rs
  - crates/core/src/provider/stream_contract.rs
  - crates/core/src/provider/turn_spec.rs
  - scripts/acp-probe.mjs
  - scripts/acp-timing-report.mjs
verify:
  - cargo test -p zest-core --lib provider::claude_code
  - cargo test -p zest-core --lib provider::claude_control
  - cargo test -p zest-core --lib provider::claude_stream
  - cargo test -p zest-core --lib provider::codex_app_server
  - cargo test -p zest-core --lib provider::cursor_acp
  - cargo test -p zest-core --lib provider::cursor_models
  - cargo test -p zest-core --lib provider::stream_contract
  - cargo test -p zest-core --lib provider::turn_spec
---

# CLI agent providers

The `Provider` trait and native API clients are in
[provider-runtime](provider-runtime.md). Enabling these providers and CLI
sign-in are in [providers-and-sign-in](providers-and-sign-in.md).

## Behavior

- All three return `owns_agent_loop() = true`: the vendor CLI owns the session,
  model and tool loop, so Zest registers no local tools for them and never offers
  them as native delegated workers. Zest never reads their credentials.
- Claude Code (`claude_code`): runs `claude --print --output-format stream-json
  --input-format stream-json --permission-prompt-tool stdio`, prompt as a stdin
  JSON message. Tools are narrowed with `--tools` to `ZEST_TOOL_SCOPE` (never
  `--allowedTools`). Legacy `default` permission mode maps to `auto`. `--effort`
  is sent except for `haiku`. A stored session is resumed with `--resume` only if
  it was created for the same model; `/btw` side turns use `--fork-session`.
- Claude permission requests (`can_use_tool`) become Zest approval cards; every
  path that cannot reach a human answers deny.
- Codex CLI (`codex_app_server`): JSON-RPC over `codex app-server --listen
  stdio://` (`initialize`, `thread/start|resume|fork`, `turn/start`). With an
  approval host the thread runs `on-request` + read-only sandbox so every write
  asks; hostless maintenance turns are read-only; only hostless tool turns get
  `workspace-write`. "Allow for session" is remembered per command / per path.
- Cursor (`cursor_acp`): `cursor-agent acp` over JSON-RPC. Cursor asks permission
  only for shell commands; file edits are never gated, so `mode = "plan"` or
  `ask` is the only way to stop edits. Answers are always `allow-once` /
  `reject-once`, never `allow-always`. No host means deny.
- Cursor models are discovered from `cursor-agent models`, cached 24 h in
  `<data dir>/zest/cursor-models.json`, falling back to the cache then
  `BUILTIN_MODELS`. Effort suffixes are split off into Zest's effort axis and
  rejoined by `wire_model`; `-fast` stays part of the model id. A configured
  `models` list is taken literally with no effort ladder.
- Cursor reuses one warm process per provider while the wire model is unchanged;
  cancel or error discards it. Side conversations get a separate `Ask`-mode
  instance so they do not evict the parent session.
- Claude Code and Codex report token usage into `Completion.usage`; Cursor ACP
  carries none (`usage_available: false`). Claude Code rate-limit events become
  `Completion.limits`.

## How it works

1. `driver_for` builds `ClaudeCodeProvider`, `CodexAppServerProvider` or
   `CursorAcpProvider`; `auth_status` comes from `detect_claude_code`,
   `detect_codex_cli`, `detect_cursor_cli` (`auth.rs`).
2. Claude Code: `stream_turn` → `turn_spec::compose` (instructions, tool scope,
   delivery; transcript sent once, then carried by `--resume`) → `args` →
   `tools::external_agent::run_headless_command_streaming` with
   `ClaudePermissions` (a `ControlResponder` built on `claude_control`) and
   `ClaudeNormalizer` (`claude_stream.rs`, a `StreamNormalizer`).
3. `stream_contract.rs` checks each stream against `CLAUDE_CODE_CONTRACT` and
   reports Ok / Degraded / Violated (`report_stream_health`); drift is a report,
   not a failure.
4. Codex and Cursor spawn through `session::JsonlProcess` (piped stdio, bounded
   stderr, Windows job object, kill on drop) and pump `rpc_request`, answering
   every server-initiated request through `ProviderInteractionHost`.
5. Cursor timing: `ZEST_ACP_TIMING_FILE` makes `AcpTiming` append one JSON row
   per turn (no prompts or output); summarize with
   `node scripts/acp-timing-report.mjs <file>`. See
   [LATENCY_BENCHMARK.md](../LATENCY_BENCHMARK.md).

## Verify

- Unit tests above use recorded shapes and fake hosts; no CLI needed.
- Live (spends account usage): `live_claude_code_session_survives_a_second_turn`
  and `live_cursor_latency` are ignored tests, e.g.
  `cargo test -p zest-core live_cursor_latency --lib -- --ignored --nocapture`.
- `npm run acp:probe` records Cursor ACP wire frames to
  `outputs/acp-probe/<stamp>.jsonl`; permissions are refused unless
  `ZEST_ACP_ALLOW=1`.

## Pitfalls

- Claude Code closes all pending permission requests when stdin closes; stdin
  must stay open until the turn's terminal event.
- `--input-format stream-json` ignores an argv prompt; the prompt must be the
  stdin message, and the `initialize` control request must be acknowledged.
- The CLI names the subagent tool `Task` in `--tools` but `Agent` in stream
  `tool_use` records.
- Codex `thread/start.sandbox` is kebab-case, `turn/start.sandboxPolicy.type` is
  camelCase.
- Cursor's `toolCallId` contains a newline; approval ids are Zest-minted instead.
- `CodexAppServerProvider::discover_models` has no caller today.
