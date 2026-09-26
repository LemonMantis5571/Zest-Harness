---
title: BTW side questions
summary: Ask a quick question about the current conversation in a side panel with /btw, without adding it to the main chat or interrupting a running task.
paths:
  - crates/core/src/btw.rs
  - crates/desktop/src/btw.rs
  - crates/desktop/ui/src/components/BtwPanel.tsx
  - docs/BTW.md
tests:
  - crates/desktop/ui/e2e/btw.spec.ts
verify:
  - npm run ui:e2e -- btw
  - cargo test -p zest-core --lib -- btw::tests
  - cargo test -p zest-desktop --lib -- btw::tests
---

# BTW side questions

User-facing reference: [docs/BTW.md](../BTW.md) (per-provider fork behaviour, lifetime, billing).

## Behavior
- `/btw [question]` as the first whole token in the main composer opens the "Side conversation" panel and sends the question; the composer is cleared. `/btw/file` or `/btw` later in the text is ordinary text. Picking `/btw` from the slash menu opens the panel and keeps the rest of the main draft.
- Composer attachments stay in the composer; they are never sent with a side question.
- The panel streams answers, accepts follow-ups, and has "Stop side answer" and "Close side conversation" (Esc also closes). A stopped or failed answer keeps the question editable in the panel and shows an alert (for example "Answer stopped").
- It works while the main task runs ("Main task is still running"); the main turn keeps going and its Stop button stays.
- Nothing from the side exchange enters the main transcript, thread file, context meter, checkpoints, queue, or provider cursor. Closing the panel or switching chats discards it; reopening `/btw` starts fresh.
- `send_message` refuses text starting with `/btw` ("Open /btw from the main chat composer to ask a side question.").
- CLI: `/btw [question]` enters a side conversation in the interactive terminal and `/back` discards it (`crates/cli/src/main.rs`).

## How it works
1. `ChatScreen.tsx` composer `onSubmit` -> `btwQuestion` (`lib/slashCommands.ts`) -> `setBtw` renders `BtwPanel.tsx`.
2. `BtwPanel` -> `backend.startBtw(sessionId)` -> Tauri `btw::start_btw` (`crates/desktop/src/btw.rs`) -> `SessionController::side_context` (`session.rs`): the snapshot the running turn's worker last published (`ActiveTurn::side_context`, via `fork_snapshot`), or `Agent::side_conversation()` when idle. `SideConversations::open` keeps one slot per session (opening again closes the previous) and at most 8 overall.
3. `backend.sendBtw(id, text, onDelta)` -> `btw::send_btw` streams `StreamEvent::Text` over a Tauri `Channel`; a second send while answering is refused. `cancel_btw` cancels the slot's `CancelToken`; `close_btw` removes the slot. `set_session` / `end_session` close the owner's slots.
4. `SideConversation::send` (`crates/core/src/btw.rs`) clones the frozen `TurnRequest`, appends a side-question instruction after the unchanged prefix, and streams via `Provider::stream_turn`. Only a text answer is committed; errors, cancellation, empty answers, or a `tool_use` block reset the provider session and commit nothing. Usage goes to the normal ledger.
5. `turn.rs` publishes side snapshots: the submitted prompt before persistence, then after each completed provider/tool step (`publish_side_context` in `agent.rs`), with sensitive tool results redacted and no provider session.

## Verify
- `npm run ui:e2e -- btw` (fixture mode): follow-ups then an unchanged main transcript; opening during a running task without queueing; slash-menu discovery; stop keeps the question editable.
- `cargo test -p zest-desktop --lib -- btw::tests` checks close/reopen cancels only the side answer and that switching sessions drops it.

## Pitfalls
- Never fork a moving provider cursor: snapshots from a running turn use `without_provider_session()` and replay the frozen transcript.
- Late deltas after close must not reopen the panel or reach the main chat; the panel guards with a lifetime flag and the backend cancels on a closed channel.
- Provider-specific side sessions live in the provider modules: Codex `thread/fork` in `provider/codex_app_server.rs`, Claude `--fork-session` in `provider/claude_code.rs`, and Cursor's separate Ask-mode session via `Provider::side_conversation_provider` in `provider/cursor_acp.rs`.
