---
title: Context budget and compaction
summary: The composer shows how full the model's context window is, and Zest trims or summarizes the conversation automatically when it passes 80%.
paths:
  - crates/core/src/context_budget.rs
  - crates/core/src/prune.rs
  - crates/core/src/bounded.rs
  - crates/core/src/agent.rs
  - crates/desktop/src/context_meter.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/workspace.rs
verify:
  - cargo test -p zest-core --lib -- context_budget::tests
  - cargo test -p zest-core --lib -- prune::tests
  - cargo test -p zest-core --lib -- bounded::tests
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-core --lib -- agent::resilience_tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- context_meter::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- native_view::tests
---

# Context budget and compaction

## Behavior
- After each turn, the status line under the composer shows `NN% · XK left` with a dot icon. The reading is measured from the provider's last response when it reported prompt usage, otherwise estimated from the conversation. It appears after the first turn of the session in that chat and stays until the next turn ends.
- The desktop app does not show a context popover with the token breakdown, measured/estimated label, or cache figures.
- After a successful turn, if compaction is due, Zest compacts automatically before the turn's worker finishes: the notice card shows "Compacting context…", then "Compacted N tool results." or "Compacted conversation context.". A failure shows "Compaction: <error>" and the turn still counts as finished. Messages sent meanwhile are queued and start afterwards.
- Workbench > Checkpoints has a "Compact context" button while the chat is idle and has agent history. It returns to the chat and runs the same compaction; queued messages wait for "Resume queued" afterwards.
- Compaction first tries trimming long tool results. If that is enough, nothing is summarized; otherwise the history is replaced by a model-written checkpoint. Either way a "Before compaction" restore point holding the previous history is saved in Checkpoints.
- Compaction never fires for small conversations: it needs more than 4,000 estimated conversation tokens and at least 4 wire messages, whatever the percentage.
- The agent also compacts inside a turn, on every front-end including the CLI. Before each provider step it checks the prompt it is about to send against the same 80% threshold (the provider's count of the previous request plus an estimate of what was added since). Over it, it trims long tool results, or if that is not enough replaces the history with a checkpoint, then the turn's own user message word for word, then a note telling the model to continue the current task. The turn shows this as a provider activity row ("Compacting the conversation to fit the context window"). A failed in-turn compaction does not end the turn.
- When the provider rejects a step as too long (a context-limit error), the agent compacts with a checkpoint and sends the step again, once. A second rejection ends the turn with the usual "This conversation is too long" error.
- A summarization request is fitted to the window before it is sent: long tool results are shortened (recent ones too), then the oldest exchanges are left out at a user message, with a note saying so. Its output budget is the smallest of `Agent::max_tokens`, 16,000 tokens and an eighth of the window. Stop cancels it and leaves the history unchanged.
- The desktop app does not join a compaction's usage trace to the turn it followed; it calls `Agent::compact_context` after the turn's run ID is cleared.

## How it works
- `native.rs` `Chat::run`, after the final save, calls `estimate_context` (`context_meter.rs`) with the agent and checkpoint count. `used_tokens` prefers the provider's `Usage::prompt_tokens()` from `Agent::last_usage` (fresh input plus cache read and write); otherwise it sums char/4 estimates from `context_budget.rs` (`system_tokens`, `conversation_tokens`, `tool_schema_tokens`). The window is `Agent::context_window()` (model descriptor, else `context_window_for_model`).
- If `should_auto_compact` is set and the turn was not cancelled, `Chat::compact_agent` runs on the same agent and the meter is re-estimated. The worker then sends `EvKind::SessionInfo` as "provider · model · N% context · T tokens left (measured|estimated)"; `native_view.rs` `session_parts` turns that into the status line's percentage and "XK".
- `auto_compaction_due` / `auto_compact_threshold` (80%, rounded up) and `MIN_COMPACTION_CONVERSATION_TOKENS` live in `context_budget.rs` so the meter and compaction agree.
- The manual button (`workspace.rs`, Checkpoints panel) sends `Cmd::Compact`, which the chat's worker runs as `Chat::compact` with a fresh agent; it marks the chat busy and `compacting` so `End` does not start the queue or raise "Response ready".
- `Chat::compact_agent` races `Agent::compact_context` against the cancel token, then in one `Chat::edit` writes a `ThreadCheckpointKind::Compaction` checkpoint of the thread as it was and saves the compacted `agent_messages` and provider session.
- `Agent::compact_context` / `compact_context_for_run(run_id, cancel)`: `prune::prune_tool_results` rewrites tool results over 8,192 chars to head 4,096 + marker + tail 1,024, leaving the last 4 messages; `pruning_relieves_pressure` checks the measured prompt minus the estimated saving against the threshold. If not enough, `write_checkpoint` sends one summarization request (`fit_history_for_checkpoint` input, tools declared but `allow_tool_use: false`, no thinking, cancellable). Both paths clear the provider session and `last_usage`. The desktop passes its turn slot's cancel token.
- In a turn, `run_turn_steps` keeps `measured` (the latest request's prompt plus reply, and how many messages it covered) and calls `compact_staged` when `auto_compaction_due` says so, or with no prune option after `HarnessError::is_context_limit`. Tests: `agent::resilience_tests` (`a_turn_near_the_window_compacts_before_its_step`, `a_prompt_the_provider_rejects_as_too_long_is_compacted_and_retried`).
- `bounded.rs` (`ends_within`, `floor_boundary`) caps wire payloads such as bash output, spilled tool results, diffs, and project docs to a byte ceiling without splitting a character; the truncation marker counts against the ceiling.

## Verify
- The core tests above cover threshold arithmetic, prune idempotence, measured vs estimated usage, and the prune-then-summarize decision.
- `context_meter::tests` cover a well-cached prompt measured from every usage column, a silent provider falling back to the estimate, and an output-only turn not counting as a measurement. `native_view::tests` cover parsing the session line into the status line's reading.
- In `npm run gpui -- --demo`, send a message: the status line shows an estimated reading. Open Workbench > Checkpoints and press "Compact context" to check the notice and the new "Before compaction" checkpoint.

## Pitfalls
- Never read `input_tokens` alone as occupancy; with prompt caching it excludes nearly the whole prompt and compaction would fire late or never.
- `can_compact` uses the conversation estimate, not the measured total: system prompt and tool schemas can approach the threshold alone and compaction cannot shrink them.
- Pruning is compaction-only. Rewriting a tool result on an ordinary turn breaks the cached prompt prefix.
- The summarizer request keeps the tool list on the wire (with tool use disallowed) so it shares the cached prefix.
- After compaction, `last_usage` is cleared, so the reading is an estimate until the next turn.
- The status line parses the `SessionInfo` text; change `Chat::run`'s format and `session_parts` together.
- In-turn compaction writes no "Before compaction" restore point: the core does not own the thread store. Between-turn compaction in the desktop still does.
