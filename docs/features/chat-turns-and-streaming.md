---
title: Chat turns and streaming
summary: Send a message, watch the answer and reasoning stream in, stop it, and queue follow-ups while a turn is still running.
paths:
  - spikes/gpui/src/main.rs
  - crates/core/src/agent.rs
  - crates/core/src/tool_input_repair.rs
  - crates/core/src/runtime.rs
  - crates/core/src/prompt.rs
  - crates/core/src/cancel.rs
  - crates/core/src/inbox.rs
  - crates/core/src/persist.rs
  - crates/desktop/src/turn.rs
  - crates/desktop/src/session.rs
  - crates/desktop/ui/src/components/ChatScreen.tsx
  - crates/desktop/ui/src/components/Composer.tsx
  - crates/desktop/ui/src/components/WorkingIndicator.tsx
  - crates/desktop/ui/src/components/ThinkingReasoning.tsx
  - crates/desktop/ui/src/components/ZestPulse.tsx
  - crates/desktop/ui/src/components/ui/message.tsx
  - crates/desktop/ui/src/components/ui/message-scroller.tsx
  - crates/desktop/ui/src/components/ui/bubble.tsx
  - crates/desktop/ui/src/lib/sendTurn.ts
  - crates/desktop/ui/src/lib/chatReducer.ts
  - crates/desktop/ui/src/lib/thinkingSummary.ts
  - crates/desktop/ui/src/lib/threadQueue.ts
  - crates/desktop/ui/src/lib/drafts.ts
  - crates/desktop/ui/src/lib/serialActions.ts
  - crates/desktop/ui/src/lib/invokeErrors.ts
tests:
  - spikes/gpui/src/selftest.rs
  - crates/core/src/agent_resilience_tests.rs
  - crates/desktop/ui/src/lib/chatReducer.test.ts
  - crates/desktop/ui/src/lib/sendTurn.test.ts
  - crates/desktop/ui/src/lib/thinkingSummary.test.ts
  - crates/desktop/ui/src/lib/threadQueue.test.ts
  - crates/desktop/ui/src/lib/serialActions.test.ts
  - crates/desktop/ui/src/lib/invokeErrors.test.ts
  - crates/desktop/ui/e2e/conversation-scroll.spec.ts
  - crates/desktop/ui/e2e/failed-followup.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- conversation-scroll
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-core --lib -- inbox::tests
  - cargo test -p zest-core --lib -- tool_input_repair::
  - cargo test -p zest-core --lib -- persist::tests
  - cargo test -p zest-core --lib -- prompt::tests
  - cargo test -p zest-core --lib -- runtime::tests
  - cargo test -p zest-desktop --lib -- turn::tests
  - cargo test -p zest-desktop --lib -- session::tests
  - cargo test -p zest-desktop --lib -- persist_event_tests
---

# Chat turns and streaming

## Behavior
- Composer footer folder and pull-request controls have minimum 24px hit areas
  without changing their navigation or approval behavior.
- Enter sends (Shift+Enter is a newline, IME composition is ignored). An empty message with no usable attachment is refused (`empty message`).
- While a turn runs, the Send button is Stop. Esc (when nothing else is open) or the `chat.stop` binding cancels via `cancel_turn`; the UI stays busy until the `cancelled` event arrives.
- Sending while a turn runs, or while the chat already has queued messages, queues a durable **followup** instead of starting a turn. Queued messages show above the composer and can be edited or removed.
- When saving a follow-up fails, its chat keeps the text and attachments in an unsent row for the session. Restore draft returns them to an empty composer, or Discard removes the row. Switching chats or typing a new draft does not overwrite either message.
- After a turn **completes**, the next followup starts automatically (FIFO). After cancel or error, queued followups stay put; "Resume queued" starts the oldest. Idle queues never spend quota by themselves.
- Text and thinking stream live. Each thinking block is a Reasoning line in the turn's Trace: it reads "Thinking", shimmers and stays open (height-capped, following the newest line) while it streams, then folds to "Thought for Ns" (or "Thought through N steps" for a reopened chat). Once the user opens or closes it, their choice wins.
- Text from separate provider rounds (split by a tool call) is separated by a blank line, never glued together.
- Each assistant turn records its steps in the order they happened: a thinking block, a tool call, a provider activity, then another thinking block if the model thought again. Provider activity is saved with the message, so a reopened Claude Code chat still shows what it did. A subagent (a delegation or a Claude Code `Agent`) streams as its own line that opens while it runs, with its steps arriving under it.
- A failed or cancelled turn keeps its partial transcript and marks running/awaiting-approval tool rows as interrupted. The model's wire history keeps the user message and every completed provider round and tool result, closes any tool call that never ran with an error result, and ends with an `[Zest: this turn was interrupted ...]` note. Tools may already have changed files, so the next turn has to know what was done.
- Tool-call arguments that are not a JSON object do not end the turn. `tool_input_repair.rs` repairs unambiguous cases against the tool's schema (arguments sent as a JSON string, even twice encoded; `"5"` for an integer, `"true"` for a boolean, quoted JSON for an object or array property). Anything else, such as JSON cut off mid-object, is not run: the call gets an error result ("Invalid arguments for `read_file`: ... Call the tool again ...") and the model tries again. Providers keep unparseable argument text as a string in `input`; the agent always replaces it with an object before the history is sent back.
- Auth failures show "Reconnect <provider>" / "Choose provider or API key" buttons on the error bubble. A served model that differs from the requested one raises a "Model changed" warning.
- A turn belongs to its chat: switching chats lets it finish in the background; a second turn on the same chat is refused as busy.
- When task tracing is enabled, the agent uses the desktop turn ID as its run
  identity, so provider rounds, tool activity and related local tasks can be
  matched to the durable lifecycle record.
- Drafts are kept per thread in localStorage (`zest:draft:<threadId>`); question/plan answers (`origin: "answer"`) never clear the draft or send its attachments.

## How it works
1. `Composer.tsx` (`handleSend`, 200 ms debounced `onChange`) -> `ChatScreen.tsx` `onSubmit` (intercepts `/btw` and `/model`) -> `App.tsx` `onSend`, which queues (`backend.sendMessage(..., "followup")`) or calls `submitTurn` (`sendTurn.ts` decides draft ownership).
2. Tauri `send_message` (`crates/desktop/src/lib.rs`): if the chat has an active turn, the input is enqueued on the live thread, persisted, pushed to the turn's `InputInbox`, and `input_queued` is emitted. Otherwise `turn::run`.
3. `turn.rs` `run_loop` / `run_with_sink_internal`: `SessionController::begin_turn` (`session.rs`), checkpoint, slash/plan expansion, run record in `ChatPersistence`, `user` + `assistant_start` events, then `Agent::send_*_cancellable_with_inbox_and_side_context`. Each `StreamEvent` becomes a `ChatEvent` on the `chat-event` channel and is applied to the live thread; `PersistWorker` (`persist.rs`) writes deltas and provider activity at most every 250 ms, everything else immediately. `Thread::apply_*` (`thread.rs`) append a `steps` entry the first time a thinking block, tool or activity appears; `chatReducer.ts` does the same live, so the live and reopened orders match. The terminal save and flush happen before the run is marked completed and before `done`/`cancelled`/`error` is emitted.
4. `agent.rs` loop: stage history, claim steer/inject inputs each provider step (`InputInbox::claim_next_step`), stream, run ungated tools concurrently and gated tools sequentially (results in call order). `run_turn_steps` builds a `TurnProgress`; `commit_turn` commits it on `end_turn`, `commit_interrupted_turn` on an error or cancel. `CancelToken` (`cancel.rs`) is checked between steps, after a completed response or tool batch is staged.
5. System prompt: `RuntimeBuilder::build` (`runtime.rs`) -> `prompt.rs` `compose_system_with_docs`: `.zest/system.md` (max 32 KiB), base rules, `AGENTS.md`/`CLAUDE.md`/`PROJECT_CONTEXT.md` (16 KiB total), skills. `env_context` (cwd, platform, branch from `.git/HEAD`) and plugin context go in the volatile half, after the cache breakpoint.
6. UI: `App.tsx` `handleChatEvent` batches `text_delta`/`thinking_delta` per animation frame (`reduceChatEvents`); other events flush the batch first, then `reduceChatEvent` (`chatReducer.ts`). Queue events update `threadQueue.ts` maps, not the transcript. `ChatScreen` renders rows in `MessageScroller`; `WorkingIndicator` shows while sending with no pending approval/question.

## Verify
- In the [native GPUI workspace](gpui-workspace.md), run `npm run gpui:test` to exercise chat and durable follow-ups through the shared agent. Its composer sends on Enter or the round send button, which becomes Stop while a turn runs; queued follow-ups, approval and question cards stack above the composer card. The rendering fixture has a separate in-memory queue test, available with `npm run gpui -- --selftest-queue`.
- `npm run ui:dev`, open `http://127.0.0.1:1420/?fixture=1`: the fixture boots with a streaming turn (Stop visible). `?fixture=1&scenario=cancel` holds a turn open to test Stop; `approval`, `question`, `tool-error` exercise the other terminal paths. Typing while busy should add a queued message.
- Rust tests above cover committed and interrupted history (`agent::resilience_tests`), inbox claiming, coalesced persistence, and turn event order.

## Pitfalls
- Events from another thread or turn are dropped by `isStaleChatEvent` (warnings excepted); routing is by `thread_id`, never session id.
- Steer/inject targets exist in the queue but the UI only sends followups; the backend injects background-job notices.
- Only `commit_turn` and `commit_interrupted_turn` write `Agent::messages` during a turn. Never commit a half-built assistant message or a tool call without a result: providers reject both. The UI transcript (`thread.messages`) is a separate projection.
- The native GPUI workspace (`spikes/gpui/src/native.rs`) still saves wire history only after a successful turn.
- `serialActions.ts` serializes split-pane activate+send because both panes share one session controller.
- Environment text must stay out of the cached prompt prefix (`prompt::tests::env_context_is_not_part_of_the_composed_prefix`).
