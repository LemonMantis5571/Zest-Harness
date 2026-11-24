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
  - crates/core/src/retry.rs
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
  - cargo test -p zest-core --lib retry::
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
  A `ModelSpec` can also carry `resolves_to` (what a provider-decided alias ran
  as) and `needs_cli` (the installed vendor CLI is too old for the model). Both
  are labels: neither removes a model or fails validation. Claude Code uses
  these fields. Installed-version discovery must not execute configured
  commands; unavailable or untrusted metadata stays unknown. See
  [CLI agent providers](cli-agent-providers.md).
- Efforts: `low medium high xhigh max`, plus `none` only for `gpt-6-sol` /
  `gpt-6-luna`. GPT-6.1 Sol and GPT-6 Astra support the five standard levels,
  vision and a 1,050,000-token context window, but reject `none`.
  Haiku 5.5 accepts adaptive thinking and effort and has a 1M window;
  older Haiku models remain excluded from the native builtin list.
  OpenAI-compatible advertises no effort control
  (`EffortPolicy::Unsupported`). UI aliases normalize via `normalize_effort`
  (`med` → `medium`, `extra high` → `xhigh`, unknown → `high`).
- Anthropic: always streams and sends `thinking` and `output_config.effort`.
- Retry (Anthropic, OpenAI-compatible and ChatGPT Codex alike, `retry.rs`): a
  request or stream that fails before any visible text reached the user is tried
  again. That covers 408/429/5xx/529, a connect or header timeout, a stream that
  drops, ends without its final event or goes 120 s silent, and a mid-stream
  `overloaded_error`. Gaps double from 2 s up to 30 s, plus jitter, for up to 10
  tries, or 15 when the provider is unreachable. A server's `retry-after`,
  `retry-after-ms` or HTTP-date wins, up to 10 minutes; a longer one ends the
  retries and shows the provider's error. Each wait appears as a provider
  activity row ("The provider is overloaded. Retrying in 4s (attempt 2 of 10)")
  and Stop ends it at once. A local OpenAI-compatible server that has not sent
  headers is not retried (it is usually loading the model).
- A stream that fails after visible text has streamed keeps that text: the
  provider returns it with stop reason `stream_interrupted`, and the agent asks
  the model to continue from where it stopped. Thinking and unfinished tool calls
  from that response are dropped. After 3 such rounds in a row the turn stops.
- Tool-call arguments that do not parse as JSON are kept as a JSON string in the
  `tool_use` block's `input` (Anthropic and OpenAI-compatible) rather than
  failing the turn; the agent repairs or rejects them (see
  [chat turns](chat-turns-and-streaming.md)). A call with no id or name is still
  a stream error.
- Prompt caching (Anthropic only): long-TTL breakpoint on the last tool, a
  breakpoint splitting the cacheable system prompt from the volatile environment
  block, and two rolling breakpoints on the two messages before the last.
  Thinking blocks are never annotated.
- A maintenance turn (`allow_tool_use = false`) keeps the tool list with
  `tool_choice: none` when caching, and withholds tools otherwise.
- `Completion.served_model` records what the endpoint says it ran; `None` means
  "did not say", not agreement. Rate-limit headers become `RateLimitSnapshot`
  (throughput headroom, never plan quota).
- Vision & multimodal history: Claude (`claude-*`, `opus`/`sonnet`/`haiku`/`fable`),
  GPT-5/GPT-6/GPT-4o, and Gemini models report `supports_vision = true` so
  `browser` and `html_preview` tool screenshots reach the model. User-attached
  base64 images (`png`, `jpeg`, `gif`, `webp`) map cleanly across Anthropic,
  Rig/ChatGPT Codex (`rig_convert::user_content`), and OpenAI-compatible
  (`image_url` data URLs).
- OpenAI-compatible reasoning & token limits: `reasoning_content`, `reasoning`,
  and `analysis` stream deltas emit live `StreamEvent::Thinking` chunks;
  `reasoning_content` is echoed back on assistant `tool_calls` turns (for
  DeepSeek Reasoner / Kimi K2 Thinking multi-step tool loops); and `o1`/`o3`/`o4`/
  `gpt-5`/`gpt-6` models send `max_completion_tokens` instead of `max_tokens`.
- ChatGPT Codex (`codex_oauth` kind) runs in Zest's loop; its session is refreshed
  and re-stored before each turn and Rig only ever receives an access token.
- `resume_support` is consulted by chat restart reconciliation only after the
  provider builds successfully. Recovery records why an old run cannot attach;
  existing adapters remain unsupported and never replay a stale cursor.

## How it works

1. `RuntimeBuilder::build` (`crates/core/src/runtime.rs`) calls
   `ProviderRegistry::from_config_at` → `registry::build` →
   `driver::resolve_required(credentials_for(id, entry))` → `driver.create`.
2. `Agent` (`agent.rs`) builds a `TurnRequest` and calls `stream_turn`, rendering
   `StreamEvent`s and folding the `Completion` into history and the ledger.
3. `AnthropicProvider::stream_turn` → `tool_plan`, `cached_system_blocks`,
   `mark_conversation_prefix` → `AnthropicClient::stream_cancellable`
   (`anthropic/client.rs`) → `SseParser` (`sse.rs`) → `TurnAccumulator`
   (`accumulate.rs`, reassembles partial tool JSON, keeps thinking signatures;
   `salvage` keeps the text of a broken stream). Wire types are in
   `anthropic/types.rs`. Every HTTP provider wraps one try (`try_once`) in a
   `retry::Attempt` and runs it with `retry::run_with_retry`.
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
  a local mock server. `a_stream_that_drops_before_any_text_is_retried` and
  `a_stream_that_drops_after_text_keeps_the_text` cover the two halves of
  stream recovery; `agent::resilience_tests` covers the continuation.
- `alpha_prove` drives the real agent loop with a scripted fake provider: model
  pinning, tool round trip, ledger attribution, thread restore.
- Live cache check (spends quota):
  `cargo test -p zest-core --test prompt_cache_e2e -- --ignored --nocapture`
  (DeepSeek key required; Anthropic runs too when a key is present).

## Pitfalls

- `rig_convert` must set both Rig call ids from the provider's id; a synthetic
  id on the wire makes the API reject the `tool_result`. It errors rather than
  dropping an unrepresentable block.
- `CODEX_KNOWN_MODELS` mirrors `CODEX_LABELS` in `ui/src/lib/models.ts`; keep
  them in sync.
- `supports_prompt_cache` defaults to false. Sending `cache_control` to a non-
  Anthropic endpoint is at best ignored.
- `quota.rs` deliberately keeps its own match over `ProviderConfig`.
- GPT-6.1 Sol and GPT-6 Astra require Responses for tool calling, so their
  builtin availability is on the Codex transports, not the Chat Completions-only
  OpenAI-compatible adapter. Current model/compatibility references:
  [OpenAI](https://developers.openai.com/api/docs/guides/latest-model),
  [Claude](https://platform.claude.com/docs/en/models/overview).
