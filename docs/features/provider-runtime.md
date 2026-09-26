---
title: Provider runtime and native API providers
summary: One provider-neutral interface for every backend, with Zest's own streaming clients for the Anthropic Messages API, OpenAI-compatible endpoints, and ChatGPT Codex.
paths:
  - crates/core/src/provider/mod.rs
  - crates/core/src/provider/driver.rs
  - crates/core/src/provider/registry.rs
  - crates/core/src/provider/anthropic.rs
  - crates/core/src/provider/openai_compatible.rs
  - crates/core/src/provider/codex_oauth.rs
  - crates/core/src/provider/codex_rig.rs
  - crates/core/src/provider/rig_convert.rs
  - crates/core/src/anthropic/
  - crates/core/src/alpha_prove.rs
tests:
  - crates/core/tests/prompt_cache_e2e.rs
verify:
  - cargo test -p zest-core --lib provider::tests
  - cargo test -p zest-core --lib provider::driver
  - cargo test -p zest-core --lib provider::registry
  - cargo test -p zest-core --lib provider::anthropic
  - cargo test -p zest-core --lib provider::openai_compatible
  - cargo test -p zest-core --lib provider::rig_convert
  - cargo test -p zest-core --lib provider::codex_rig
  - cargo test -p zest-core --lib provider::codex_oauth
  - cargo test -p zest-core --lib anthropic::
  - cargo test -p zest-core --lib alpha_prove
---

# Provider runtime and native API providers

CLI-owned providers (Claude Code, Codex app-server, Cursor ACP) and their shared
transport are in [cli-agent-providers](cli-agent-providers.md). Sign-in and key
storage are in [providers-and-sign-in](providers-and-sign-in.md).

## Behavior

- The agent loop sees only the `Provider` trait (`provider/mod.rs`): `id`,
  `default_model`, `models`, `validate_selection`, `auth_status`,
  `owns_agent_loop`, `supports_prompt_cache`, `stream_turn`. It never branches
  on which backend it talks to.
- Capability follows the config `kind`, never the provider id. One exhaustive
  match, `driver::driver_for`, builds both the picker catalogue
  (`descriptor_from_config`) and the live provider, so the picker cannot offer a
  model the provider rejects.
- A provider that cannot be built is skipped with a user-facing reason
  (`registry::Skipped`); one missing key never stops the others loading.
- Credentials: the OS credential manager account is checked first, then the
  environment variable; a keyring error still falls back to the env var. A blank
  `credential` falls through to env. OpenAI-compatible `deepseek`, `openai`,
  `gemini` get the conventional `{ID}_API_KEY` when `api_key_env` is omitted.
  Vendor-owned kinds are never asked for a key.
- Catalogue (`catalogue`): configured `models`, else the kind's builtin ids, else
  just `default_model`; the default is always present and first. Unknown models
  or efforts outside a model's list are rejected before a turn spends quota.
- Efforts: `low medium high xhigh max`, plus `none` only for `gpt-6-sol` /
  `gpt-6-luna`. OpenAI-compatible advertises no effort control
  (`EffortPolicy::Unsupported`). UI aliases normalize via `normalize_effort`
  (`med` → `medium`, `extra high` → `xhigh`, unknown → `high`).
- Anthropic: always streams, sends `thinking` and `output_config.effort`, retries
  only before the body starts (3 attempts, honours `retry-after`), fails on 120 s
  of silence or a stream that ends without `message_stop`.
- Prompt caching (Anthropic only): long-TTL breakpoint on the last tool, a
  breakpoint splitting the cacheable system prompt from the volatile environment
  block, and two rolling breakpoints on the two messages before the last.
  Thinking blocks are never annotated.
- A maintenance turn (`allow_tool_use = false`) keeps the tool list with
  `tool_choice: none` when caching, and withholds tools otherwise.
- `Completion.served_model` records what the endpoint says it ran; `None` means
  "did not say", not agreement. Rate-limit headers become `RateLimitSnapshot`
  (throughput headroom, never plan quota).
- ChatGPT Codex (`codex_oauth` kind) runs in Zest's loop; its session is refreshed
  and re-stored before each turn and Rig only ever receives an access token.

## How it works

1. `RuntimeBuilder::build` (`crates/core/src/runtime.rs`) calls
   `ProviderRegistry::from_config_at` → `registry::build` →
   `driver::resolve_required(credentials_for(id, entry))` → `driver.create`.
2. `Agent` (`agent.rs`) builds a `TurnRequest` and calls `stream_turn`, rendering
   `StreamEvent`s and folding the `Completion` into history and the ledger.
3. `AnthropicProvider::stream_turn` → `tool_plan`, `cached_system_blocks`,
   `mark_conversation_prefix` → `AnthropicClient::stream_cancellable`
   (`anthropic/client.rs`) → `SseParser` (`sse.rs`) → `TurnAccumulator`
   (`accumulate.rs`, reassembles partial tool JSON, keeps thinking signatures).
   Wire types are in `anthropic/types.rs`.
4. `OpenAiCompatibleProvider` converts history at the HTTP boundary
   (`convert_messages`, `convert_tool`), streams Chat Completions with
   `include_usage`, splits cached prompt tokens out of `prompt_tokens`, and reads
   `x-ratelimit-*` headers.
5. `CodexOAuthProvider::stream_turn` → `codex_oauth::refresh_and_store` →
   `codex_rig::stream_turn`, which converts history with `rig_convert`
   (`to_rig_history`, `to_rig_tools`, `from_rig_content`) and uses
   `rig_core::providers::chatgpt`. The hand-rolled Responses code in
   `provider/codex_oauth.rs` is kept only so its tests guard the wire shape.
6. `provider::probe` sends a one-token turn; used by the desktop's
   `verify_provider`.

## Verify

- Unit tests above need no network except `anthropic::client` tests, which bind
  a local mock server.
- `alpha_prove` drives the real agent loop with a scripted fake provider: model
  pinning, tool round trip, ledger attribution, thread restore.
- Live cache check (spends quota):
  `cargo test -p zest-core --test prompt_cache_e2e -- --ignored --nocapture`
  (DeepSeek key required; Anthropic runs too when a key is present).

## Pitfalls

- `rig_convert` must set both Rig call ids from the provider's id; a synthetic
  id on the wire makes the API reject the `tool_result`. It errors rather than
  dropping an unrepresentable block.
- `CODEX_KNOWN_MODELS` mirrors `MODEL_LABELS` in `ui/src/lib/models.ts`; keep
  them in sync.
- `supports_prompt_cache` defaults to false. Sending `cache_control` to a non-
  Anthropic endpoint is at best ignored.
- `quota.rs` deliberately keeps its own match over `ProviderConfig`.
