---
title: Chat turns and streaming
summary: Send a message, watch the answer and reasoning stream in, stop it, and queue follow-ups while a turn is still running.
paths:
  - crates/desktop/src/main.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/bridge.rs
  - crates/desktop/src/composer.rs
  - crates/desktop/src/view.rs
  - crates/core/src/agent.rs
  - crates/core/src/runtime.rs
  - crates/core/src/prompt.rs
  - crates/core/src/cancel.rs
  - crates/core/src/inbox.rs
  - crates/core/src/persist.rs
tests:
  - crates/desktop/src/selftest.rs
  - crates/desktop/src/workspace_test.rs
verify:
  - npm run gpui:test
  - npm run gpui -- --selftest-queue
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- native::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_test::
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-core --lib -- inbox::tests
  - cargo test -p zest-core --lib -- persist::tests
  - cargo test -p zest-core --lib -- prompt::tests
  - cargo test -p zest-core --lib -- runtime::tests
---

# Chat turns and streaming

## Behavior
- Enter sends (Shift+Enter is a newline; Enter during IME composition is ignored). An empty draft with no image and no attached file does not send. A message whose content is empty after attachments are read fails the turn with "Message has no usable content".
- While a turn runs, the round Send button is Stop ("Stop response") and a "Working…" line shows above the composer ("Stopping…" after Stop). Esc in the composer also stops. The chat stays busy until the worker's final `End` event arrives.
- Every message is first saved as a durable **followup** in the thread's `pending_inputs`. If the chat is idle and nothing is queued, it starts at once; otherwise it shows in "Queued (N)" above the composer, where each row can be edited ("Edit", saved with Enter) or removed.
- If saving the message fails, the error shows in the notice card and the text and pasted images go back into the composer.
- After a turn **completes**, the next queued message starts automatically (FIFO). After Stop or an error, queued messages stay put and "Resume queued" starts the oldest. Idle queues never spend quota by themselves; a reopened chat restores its queue without starting it.
- Text and thinking stream live. Each thinking block is a "Thinking" row that becomes "Thought for Ns" when the next text, tool call or activity arrives. Rows start collapsed and the user can open any of them.
- Text from separate provider rounds (split by a tool call) is separated by a blank line, never glued together.
- Each assistant turn records its steps in the order they happened (thinking, tool call, provider activity), so a reopened chat shows the same order. Provider activity is saved with the message.
- A failed or cancelled turn keeps its partial transcript, marks running or awaiting-approval tool rows as interrupted, shows the error under the answer, and does not change the model's wire history. Provider transport details stay out of the error text.
- A served model that differs from the requested one shows "Requested X, provider served Y" in the notice card. The desktop app does not offer reconnect or provider-choice buttons on an authentication error; the message names the problem and the provider is changed in Customize.
- A turn belongs to its chat: each open chat has its own worker, so switching chats or splits lets a turn finish in the background, and a second message to a busy chat is queued.
- The agent uses the turn's run ID (`turn_…`) as its run identity, so provider rounds, tool activity and the run record in `ChatPersistence` can be matched.
- Drafts live in each open chat's composer for the session; they are not saved to disk.

## How it works
1. `composer.rs` emits `ComposerEvent::Send` -> `main.rs` subscriber -> `native_view.rs` `submit_native`, which handles `/model` and `/btw`, then `accept_submission`: pasted images are prepared, image limits checked, and `native::Chat::enqueue` saves a `ThreadInputTarget::Followup` input. If the chat is idle with an empty queue, `start_next` sends `Cmd::Run(input_id)` over the chat's bridge and pushes the user row.
2. `native.rs` `Chat::bridge` runs one Tokio task per chat. It waits for the previous run to finish, creates a fresh `CancelToken`, and runs `Chat::run`; `Cmd::Cancel` cancels the token. Every run ends with `EvKind::End`, after `EvKind::Failure` when it failed.
3. `Chat::run` builds the agent (`Chat::agent`: `RuntimeBuilder` with the chat's provider, model, effort, approval policy, job registry, browser adapter and HTML artifacts; the offline demo uses its own provider), appends enabled plugin context to the volatile system prompt, then in one `Chat::edit` claims the input, expands slash commands (or the `plan` skill in Plan mode), builds content blocks with `attachments::build_user_content`, writes a "Before turn" checkpoint, and records the user message, assistant start and `TurnStarted` event. It creates the run record, then calls `Agent::send_blocks_cancellable`.
4. Each `StreamEvent` is applied to the live thread (`apply` in `native.rs`) and forwarded as an `EvKind`; the thread is saved at most every 250 ms while streaming. On finish, `apply_done` commits the agent's messages and provider session, or `terminalize_interrupted` and `apply_error` record the failure; the final save and the run status (completed, aborted, failed) happen before `End` is sent.
5. `agent.rs` loop: stream, run ungated tools concurrently and gated tools sequentially (results in call order), and commit `Agent::messages` only on `end_turn`. `CancelToken` (`cancel.rs`) is checked between steps. The desktop sends no steer or inject inputs; `InputInbox` (`inbox.rs`) is used by other callers.
6. System prompt: `RuntimeBuilder::build` (`runtime.rs`) -> `prompt.rs` `compose_system_with_docs`: `.zest/system.md` (max 32 KiB), base rules, `AGENTS.md`/`CLAUDE.md`/`PROJECT_CONTEXT.md` (16 KiB total), skills. `env_context` (cwd, platform, branch from `.git/HEAD`) and plugin context go in the volatile half, after the cache breakpoint.
7. UI: `main.rs` pumps bridge events in batches into `Root::apply`, which adds and updates rows (see [Message rendering](message-rendering.md)). On `End` it clears pending cards, rebuilds the turn from the saved thread (`restore_native`), and starts the next queued message unless the turn was stopped, failed or was a compaction.

## Verify
- `npm run gpui:test` drives the native window through the real agent loop with the offline provider: two turns sent back to back, saved wire history and checkpoints, stop with a queued message, the run records (completed and aborted), editing the queued message after reopening, and resuming it.
- `cargo test ... -- workspace_test::` runs the same scenario in a GPUI test window, as Windows CI does.
- `npm run gpui -- --selftest-queue` checks the rendering fixture's in-memory queue: rapid Enter, remove, Stop, idle enqueue, resume, FIFO and image count.
- `native::tests` cover an unavailable provider leaving the input queued for retry and transport details staying out of chat errors. The core tests above cover transactional history, inbox claiming, coalesced persistence and prompt composition.

## Pitfalls
- The UI reserves the turn (`turn_active`) before the worker sends `Start`, and releases it only on `End`; a Stop can cross a successful `End` on its way, so `stop_requested` is checked too before starting the next message.
- `Agent::messages` is transactional: never write partial turns into it. The transcript (`thread.messages`) is a separate projection.
- `Chat::edit` saves the thread before replacing the in-memory copy, so a failed write leaves both unchanged.
- Environment text must stay out of the cached prompt prefix (`prompt::tests::env_context_is_not_part_of_the_composed_prefix`).
