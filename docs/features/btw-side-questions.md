---
title: BTW side questions
summary: Ask a quick question about the current conversation in a side panel with /btw, without adding it to the main chat or interrupting a running task.
paths:
  - crates/core/src/btw.rs
  - crates/desktop/src/side.rs
  - crates/desktop/src/native_view.rs
  - docs/BTW.md
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - npm run gpui:test
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_test::
  - cargo test -p zest-core --lib -- btw::tests
---

# BTW side questions

User-facing reference: [docs/BTW.md](../BTW.md) (per-provider fork behaviour, lifetime, billing).

## Behavior
- `/btw [question]` as the first whole token in the main composer opens the "Side conversation" panel above the composer and sends the question; the composer text is cleared. `/btw` alone opens the panel without asking. `/btw/file` or `/btw` later in the text is ordinary text.
- Composer attachments stay in the composer: picked files are untouched and pasted images are put back. They are never sent with a side question.
- The panel lists each question and its streamed answer and has its own "Ask a side question" field for follow-ups. "Stop side answer" (or Esc in that field) cancels the answer; "Close" discards the conversation.
- A second question while one is being answered is refused with "Finish or stop the current side answer first." and stays in the field. A stopped or failed answer shows the error in the panel and puts the question back in the field.
- It works while the main task runs; the main turn keeps going and its Stop button stays.
- Nothing from the side exchange enters the main transcript, thread file, context meter, checkpoints, queue, or provider cursor. Closing the panel or opening another chat discards it; the next `/btw` starts fresh.
- `/btw` is handled before a message is queued, so it never reaches the agent as a main-chat message.
- CLI: `/btw [question]` enters a side conversation in the interactive terminal and `/back` discards it (`crates/cli/src/main.rs`).

## How it works
1. `native_view.rs` `submit_native` checks `zest_core::commands::btw_question`; a match calls `Root::ask_side` (`side.rs`) instead of `accept_submission`. Follow-ups from the panel's field reach `ask_side` through the `side_input` subscription in `main.rs`.
2. `ask_side` creates the `SideConversation` on the first question of a panel: `native::Chat::agent` builds an agent from the chat's saved `agent_messages`, and when the main turn is still streaming, the running user prompt is appended so the side sees what was asked. Then `Agent::side_conversation().without_provider_session()`. It does not include the running turn's in-progress tool steps.
3. The send runs on the Tokio executor; `StreamEvent::Text` deltas come back over a channel and are appended to the last answer. A generation counter drops deltas and results that arrive after the panel was closed or reopened. `close_side` resets the state; dropping `Side` cancels its token. `Workspace::open` closes the side panel of the chat being left.
4. `SideConversation::send` (`crates/core/src/btw.rs`) clones the frozen `TurnRequest`, appends a side-question instruction after the unchanged prefix, and streams via `Provider::stream_turn`. Only a text answer is committed; errors, cancellation, empty answers, or a `tool_use` block reset the provider session and commit nothing. Usage goes to the normal ledger.

## Verify
- `npm run gpui:test` (and `workspace_test::` in a GPUI test window) sends `/btw side question` in the offline demo, waits for the side answer, and checks that the saved thread's message count is unchanged before closing the panel.
- By hand in `npm run gpui -- --demo`: send a message, then `/btw what did I ask?`; the answer appears in the panel, the transcript is unchanged, and Close removes the panel.

## Pitfalls
- Never fork a moving provider cursor: side conversations always use `without_provider_session()` and replay the saved transcript.
- Late deltas after close must not reopen the panel or reach the main chat; keep the generation check in both the delta loop and the final result.
- Provider-specific side sessions live in the provider modules: Codex `thread/fork` in `provider/codex_app_server.rs`, Claude `--fork-session` in `provider/claude_code.rs`, and Cursor's separate Ask-mode session via `Provider::side_conversation_provider` in `provider/cursor_acp.rs`.
