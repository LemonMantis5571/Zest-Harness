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
  - crates/core/src/tool_input_repair.rs
  - crates/core/src/runtime.rs
  - crates/core/src/prompt.rs
  - crates/core/src/cancel.rs
  - crates/core/src/inbox.rs
  - crates/core/src/persist.rs
  - crates/core/src/tools/todo.rs
tests:
  - crates/desktop/src/selftest.rs
  - crates/core/src/agent_resilience_tests.rs
  - crates/desktop/src/workspace_test.rs
verify:
  - npm run gpui:test
  - npm run gpui -- --selftest-queue
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- native::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_test::
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-core --lib -- inbox::tests
  - cargo test -p zest-core --lib -- tool_input_repair::
  - cargo test -p zest-core --lib -- persist::tests
  - cargo test -p zest-core --lib -- prompt::tests
  - cargo test -p zest-core --lib -- runtime::tests
  - cargo test -p zest-core --lib -- tools::todo::tests
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
- A failed or cancelled turn keeps its partial transcript, marks running or awaiting-approval tool rows as interrupted, and shows the error under the answer. Provider transport details stay out of the error text.
- An interrupted turn's wire history keeps the user message and every completed provider round and tool result, closes any tool call that never ran with an error result, and ends with an `[Zest: this turn was interrupted ...]` note, because tools may already have changed files. The desktop saves that history with the thread and drops the provider session, so the next turn, even after a restart, sees what the interrupted one did.
- A response that stops short does not end the turn. After `max_tokens` the agent keeps the text and any tool calls that finished (the cut-off call gets an error result) and tells the model to continue in smaller pieces. An empty response, a stream that broke after visible text (`stream_interrupted`), or an unknown stop reason is continued the same way with its own note. After 3 such rounds in a row the turn stops with an error. A tool round resets the count. OpenAI-compatible `length` maps to `max_tokens` and `content_filter` to `refusal`; a refusal still ends the turn.
- A turn takes at most `[tools] max_steps` provider rounds (default 200, `0` = no limit; `Agent::max_steps`). At the limit the model gets one more round with tools forbidden and a note asking it to summarize what it did and what is left; that round ends the turn, and any tool call in it is dropped.
- When the model calls the same tool with identical input 3 rounds in a row (repeats inside one batch count once), Zest asks the user through the question card, "Keep going" or "Stop the turn". Keep going resets the count. Stop, or no interactive front-end (CLI, delegation workers), ends the turn before the call runs.
- Tool-call arguments that are not a JSON object do not end the turn. `tool_input_repair.rs` repairs unambiguous cases against the tool's schema (arguments sent as a JSON string, even twice encoded; `"5"` for an integer, `"true"` for a boolean, quoted JSON for an object or array property). Anything else, such as JSON cut off mid-object, is not run: the call gets an error result ("Invalid arguments for `read_file`: ... Call the tool again ...") and the model tries again. Providers keep unparseable argument text as a string in `input`; the agent always replaces it with an object before the history is sent back.
- A served model that differs from the requested one shows "Requested X, provider served Y" in the notice card. The desktop app does not offer reconnect or provider-choice buttons on an authentication error; the message names the problem and the provider is changed in Customize.
- A turn belongs to its chat: each open chat has its own worker, so switching chats or splits lets a turn finish in the background, and a second message to a busy chat is queued.
- The agent uses the turn's run ID (`turn_…`) as its run identity, so provider rounds, tool activity and the run record in `ChatPersistence` can be matched.
- Native runtimes register `todo_write` (`crates/core/src/tools/todo.rs`) so the model can maintain a structured checklist (`pending`, `in_progress`, `completed`, up to 32 items) across multi-step work. Before ending a turn (`end_turn`), `run_turn_steps` sends at most one `[Zest harness check]` reminder round if non-prose code files were modified without a subsequent `bash`/`html_preview` verification run (when `bash` is registered and the user has not denied execution) or if unfinished items remain in `todo_write`.
- Sending `/goal <criteria>` sets an active goal on the thread (shown in the composer status row; `/goal` inspects it, `/goal pause` and `/goal resume` pause/continue it, `/goal clear` removes it, and pressing Stop pauses it). While an unpaused goal is active, Zest frames turns with the goal criteria and continues across up to 4 autonomous evaluation rounds per turn until the model emits `[GOAL_MET]` (clearing the goal) or `[GOAL_BLOCKED]`.
- Drafts live in each open chat's composer for the session; they are not saved to disk.

## How it works
1. `composer.rs` emits `ComposerEvent::Send` -> `main.rs` subscriber -> `native_view.rs` `submit_native`, which handles `/model` and `/btw`, then `accept_submission`: pasted images are prepared, image limits checked, and `native::Chat::enqueue` saves a `ThreadInputTarget::Followup` input. If the chat is idle with an empty queue, `start_next` sends `Cmd::Run(input_id)` over the chat's bridge and pushes the user row.
2. `native.rs` `Chat::bridge` runs one Tokio task per chat. It waits for the previous run to finish, creates a fresh `CancelToken`, and runs `Chat::run`; `Cmd::Cancel` cancels the token. Every run ends with `EvKind::End`, after `EvKind::Failure` when it failed.
3. `Chat::run` builds the agent (`Chat::agent`: `RuntimeBuilder` with the chat's provider, model, effort, approval policy, job registry, browser adapter and HTML artifacts; the offline demo uses its own provider), appends enabled plugin context to the volatile system prompt, then in one `Chat::edit` claims the input, expands slash commands (or the `plan` skill in Plan mode), builds content blocks with `attachments::build_user_content`, writes a "Before turn" checkpoint, and records the user message, assistant start and `TurnStarted` event. It creates the run record, then calls `Agent::send_blocks_cancellable`.
4. Each `StreamEvent` is applied to the live thread (`apply` in `native.rs`) and forwarded as an `EvKind`; the thread is saved at most every 250 ms while streaming. On finish, `apply_done` commits the agent's messages and provider session, or `terminalize_interrupted` and `apply_error` record the failure; the final save and the run status (completed, aborted, failed) happen before `End` is sent.
5. `agent.rs` loop: stream, run ungated tools concurrently and gated tools sequentially (results in call order). `run_turn_steps` builds a `TurnProgress`; `commit_turn` commits it on `end_turn`, `commit_interrupted_turn` on an error or cancel. `CancelToken` (`cancel.rs`) is checked between steps. The desktop sends no steer or inject inputs; `InputInbox` (`inbox.rs`) is used by other callers.
6. System prompt: `RuntimeBuilder::build` (`runtime.rs`) -> `prompt.rs` `compose_system_with_docs`: `.zest/system.md` (max 32 KiB), base rules, `AGENTS.md`/`CLAUDE.md`/`PROJECT_CONTEXT.md` (16 KiB total), skills. `env_context` (cwd, platform, branch from `.git/HEAD`) and plugin context go in the volatile half, after the cache breakpoint.
7. UI: `main.rs` pumps bridge events in batches into `Root::apply`, which adds and updates rows (see [Message rendering](message-rendering.md)). On `End` it clears pending cards, rebuilds the turn from the saved thread (`restore_native`), and starts the next queued message unless the turn was stopped, failed or was a compaction.

## Verify
- `npm run gpui:test` drives the native window through the real agent loop with the offline provider: two turns sent back to back, saved wire history and checkpoints, stop with a queued message, the run records (completed and aborted), editing the queued message after reopening, and resuming it.
- `cargo test ... -- workspace_test::` runs the same scenario in a GPUI test window, as Windows CI does.
- `npm run gpui -- --selftest-queue` checks the rendering fixture's in-memory queue: rapid Enter, remove, Stop, idle enqueue, resume, FIFO and image count.
- `native::tests` cover an unavailable provider leaving the input queued for retry and transport details staying out of chat errors. The core tests above cover transactional history, inbox claiming, coalesced persistence and prompt composition.

## Pitfalls
- The UI reserves the turn (`turn_active`) before the worker sends `Start`, and releases it only on `End`; a Stop can cross a successful `End` on its way, so `stop_requested` is checked too before starting the next message.
- Only `commit_turn` and `commit_interrupted_turn` write `Agent::messages` during a turn. Never commit a half-built assistant message or a tool call without a result: providers reject both. The transcript (`thread.messages`) is a separate projection.
- `Chat::run` in `native.rs` saves `agent.messages_for_persist()` on success and on failure or Stop; only a successful turn keeps the provider session.
- `Chat::edit` saves the thread before replacing the in-memory copy, so a failed write leaves both unchanged.
- Environment text must stay out of the cached prompt prefix (`prompt::tests::env_context_is_not_part_of_the_composed_prefix`).
