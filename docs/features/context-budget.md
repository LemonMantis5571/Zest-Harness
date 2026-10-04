---
title: Context budget and compaction
summary: The composer shows how full the model's context window is, and Zest trims or summarizes the conversation automatically when it passes 80%.
paths:
  - crates/core/src/context_budget.rs
  - crates/core/src/prune.rs
  - crates/core/src/bounded.rs
  - crates/core/src/agent.rs
  - crates/desktop/src/context_meter.rs
  - crates/desktop/ui/src/components/ContextUsageButton.tsx
  - crates/desktop/ui/src/lib/contextUsageState.ts
tests:
  - crates/desktop/ui/src/lib/contextUsageState.test.ts
  - crates/desktop/ui/e2e/context-meter.spec.ts
verify:
  - cargo test -p zest-core --lib -- context_budget::tests
  - cargo test -p zest-core --lib -- prune::tests
  - cargo test -p zest-core --lib -- bounded::tests
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-desktop --lib -- context_meter::tests
  - npm run ui:e2e -- context-meter
---

# Context budget and compaction

## Behavior
- The composer footer shows `NN% · XK left`. Clicking it opens a popover with used/window tokens, whether the number is measured ("Based on the latest response.") or estimated ("Estimated from the conversation."), a System / Conversation / Tool schemas / Messages / Checkpoints breakdown, and fresh-input / cache-read / cache-write figures when measured.
- The popover says automatic compaction starts at 80% full, or warns that the conversation will be compacted after this turn.
- While a turn is running the backend cannot measure the chat (`context_usage` answers `busy`), so the meter keeps its last reading for that chat and refreshes when the turn ends. It never shows another chat's reading, and any other failure shows `Context —`. It refetches when the thread, message count, checkpoint count, or busy state changes.
- After a turn finishes (`done`) in the open chat, if compaction is due, Zest compacts automatically. Sending is blocked while compaction runs.
- Optional maintenance traces inherit the last successful turn's durable ID
  from the thread lifecycle, even after the agent is idle or a later turn fails.
- Compaction first tries trimming long tool results. If that is enough, nothing is summarized and the toast says "Trimmed long tool output"; otherwise the history is replaced by a model-written checkpoint ("Conversation compacted automatically"). Either way a "Before compaction" restore point is written first.
- Compaction never fires for small conversations: it needs more than 4,000 estimated conversation tokens and at least 4 wire messages, whatever the percentage.

## How it works
- `ContextUsageButton.tsx` (scoped to the chat by `contextScope` from `ChatScreen`; `settleContextReading` in `contextUsageState.ts` decides what to keep) -> `backend.contextUsage()` -> Tauri `context_usage` (`crates/desktop/src/lib.rs`) -> `estimate_context` (`context_meter.rs`). `used_tokens` prefers the provider's `Usage::prompt_tokens()` from `Agent::last_usage` (fresh input plus cache read and write); otherwise it sums char/4 estimates from `context_budget.rs` (`system_tokens`, `conversation_tokens`, `tool_schema_tokens`). The window is `Agent::context_window()` (model descriptor, else `context_window_for_model`).
- `auto_compaction_due` / `auto_compact_threshold` (80%, rounded up) and `MIN_COMPACTION_CONVERSATION_TOKENS` live in `context_budget.rs` so the meter and compaction agree.
- `App.tsx` `maybeAutoCompact` (after `done`, skipped in split view) re-reads usage, retrying briefly while the turn slot frees, then calls `compact_context`.
- Tauri `compact_context` requires an idle session, occupies the turn slot, writes a `ThreadCheckpointKind::Compaction` checkpoint, runs `Agent::compact_context` (`agent.rs`), then persists redacted `agent_messages` and returns `CompactionResultView` (`pruned_only`, `results_pruned`, new usage).
- `Agent::compact_context`: `prune::prune_tool_results` rewrites tool results over 8,192 chars to head 4,096 + marker + tail 1,024, leaving the last 4 messages; `pruning_relieves_pressure` checks the measured prompt minus the estimated saving against the threshold. If not enough, one summarization request with tools declared but `allow_tool_use: false`, no thinking, 4,096 max tokens. Both paths clear the provider session and `last_usage`.
- `bounded.rs` (`ends_within`, `floor_boundary`) caps wire payloads such as bash output, spilled tool results, diffs, and project docs to a byte ceiling without splitting a character; the truncation marker counts against the ceiling.

## Verify
- Rust tests above cover threshold arithmetic, prune idempotence, measured vs estimated usage, and the prune-then-summarize decision.
- `cargo test -p zest-desktop --lib idle_compaction_joins_the_last_successful_desktop_turn`
  drives successful and failed desktop turns, then the actual idle compaction
  handler, and checks the persisted checkpoint and joined task trace.
- `?fixture=1`: the fixture backend serves a canned `contextUsage()` (4.7%, measured) and `compactContext()`, so the footer and popover render offline. Like the desktop, it answers `busy` while a turn is in flight (`split-streaming` sends, safety scenarios).
- `npm run ui:e2e -- context-meter` keeps a `split-streaming` turn open and asserts the meter keeps its reading through it.

## Pitfalls
- Never read `input_tokens` alone as occupancy; with prompt caching it excludes nearly the whole prompt and compaction would fire late or never.
- `can_compact` uses the conversation estimate, not the measured total: system prompt and tool schemas can approach the threshold alone and compaction cannot shrink them.
- Pruning is compaction-only. Rewriting a tool result on an ordinary turn breaks the cached prompt prefix.
- The summarizer request keeps the tool list on the wire (with tool use disallowed) so it shares the cached prefix.
- After compaction, `last_usage` is cleared, so the meter shows an estimate until the next turn.
