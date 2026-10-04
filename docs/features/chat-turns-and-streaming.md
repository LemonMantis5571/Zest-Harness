---
title: Chat turns and streaming
summary: Send a message, watch the answer and reasoning stream in, stop it, and queue follow-ups while a turn is still running.
paths:
  - crates/core/src/agent.rs
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
  - crates/desktop/ui/src/lib/chatReducer.test.ts
  - crates/desktop/ui/src/lib/sendTurn.test.ts
  - crates/desktop/ui/src/lib/thinkingSummary.test.ts
  - crates/desktop/ui/src/lib/threadQueue.test.ts
  - crates/desktop/ui/src/lib/serialActions.test.ts
  - crates/desktop/ui/src/lib/invokeErrors.test.ts
  - crates/desktop/ui/e2e/conversation-scroll.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- conversation-scroll
  - cargo test -p zest-core --lib -- agent::tests
  - cargo test -p zest-core --lib -- inbox::tests
  - cargo test -p zest-core --lib -- persist::tests
  - cargo test -p zest-core --lib -- prompt::tests
  - cargo test -p zest-core --lib -- runtime::tests
  - cargo test -p zest-desktop --lib -- turn::tests
  - cargo test -p zest-desktop --lib -- session::tests
  - cargo test -p zest-desktop --lib -- persist_event_tests
---

# Chat turns and streaming

## Behavior
- Enter sends (Shift+Enter is a newline, IME composition is ignored). An empty message with no usable attachment is refused (`empty message`).
- While a turn runs, the Send button is Stop. Esc (when nothing else is open) or the `chat.stop` binding cancels via `cancel_turn`; the UI stays busy until the `cancelled` event arrives.
- Sending while a turn runs, or while the chat already has queued messages, queues a durable **followup** instead of starting a turn. Queued messages show above the composer and can be edited or removed.
- After a turn **completes**, the next followup starts automatically (FIFO). After cancel or error, queued followups stay put; "Resume queued" starts the oldest. Idle queues never spend quota by themselves.
- Text and thinking stream live. Thinking shows as an open, height-capped trace while streaming, then folds to "Thought for Ns" (or "Thought through N steps" for a reopened chat).
- Text from separate provider rounds (split by a tool call) is separated by a blank line, never glued together.
- A failed or cancelled turn keeps its partial transcript, marks running/awaiting-approval tool rows as interrupted, and does not change the model's wire history.
- Auth failures show "Reconnect <provider>" / "Choose provider or API key" buttons on the error bubble. A served model that differs from the requested one raises a "Model changed" warning.
- A turn belongs to its chat: switching chats lets it finish in the background; a second turn on the same chat is refused as busy.
- When task tracing is enabled, the agent uses the desktop turn ID as its run
  identity, so provider rounds, tool activity and related local tasks can be
  matched to the durable lifecycle record.
- Drafts are kept per thread in localStorage (`zest:draft:<threadId>`); question/plan answers (`origin: "answer"`) never clear the draft or send its attachments.

## How it works
1. `Composer.tsx` (`handleSend`, 200 ms debounced `onChange`) -> `ChatScreen.tsx` `onSubmit` (intercepts `/btw` and `/model`) -> `App.tsx` `onSend`, which queues (`backend.sendMessage(..., "followup")`) or calls `submitTurn` (`sendTurn.ts` decides draft ownership).
2. Tauri `send_message` (`crates/desktop/src/lib.rs`): if the chat has an active turn, the input is enqueued on the live thread, persisted, pushed to the turn's `InputInbox`, and `input_queued` is emitted. Otherwise `turn::run`.
3. `turn.rs` `run_loop` / `run_with_sink_internal`: `SessionController::begin_turn` (`session.rs`), checkpoint, slash/plan expansion, run record in `ChatPersistence`, `user` + `assistant_start` events, then `Agent::send_*_cancellable_with_inbox_and_side_context`. Each `StreamEvent` becomes a `ChatEvent` on the `chat-event` channel and is applied to the live thread; `PersistWorker` (`persist.rs`) writes deltas at most every 250 ms, everything else immediately. The terminal save and flush happen before the run is marked completed and before `done`/`cancelled`/`error` is emitted.
4. `agent.rs` loop: stage history, claim steer/inject inputs each provider step (`InputInbox::claim_next_step`), stream, run ungated tools concurrently and gated tools sequentially (results in call order), commit `Agent::messages` only on `end_turn`. `CancelToken` (`cancel.rs`) is checked between steps.
5. System prompt: `RuntimeBuilder::build` (`runtime.rs`) -> `prompt.rs` `compose_system_with_docs`: `.zest/system.md` (max 32 KiB), base rules, `AGENTS.md`/`CLAUDE.md`/`PROJECT_CONTEXT.md` (16 KiB total), skills. `env_context` (cwd, platform, branch from `.git/HEAD`) and plugin context go in the volatile half, after the cache breakpoint.
6. UI: `App.tsx` `handleChatEvent` batches `text_delta`/`thinking_delta` per animation frame (`reduceChatEvents`); other events flush the batch first, then `reduceChatEvent` (`chatReducer.ts`). Queue events update `threadQueue.ts` maps, not the transcript. `ChatScreen` renders rows in `MessageScroller`; `WorkingIndicator` shows while sending with no pending approval/question.

## Verify
- `npm run ui:dev`, open `http://127.0.0.1:1420/?fixture=1`: the fixture boots with a streaming turn (Stop visible). `?fixture=1&scenario=cancel` holds a turn open to test Stop; `approval`, `question`, `tool-error` exercise the other terminal paths. Typing while busy should add a queued message.
- Rust tests above cover transactional history, inbox claiming, coalesced persistence, and turn event order.

## Pitfalls
- Events from another thread or turn are dropped by `isStaleChatEvent` (warnings excepted); routing is by `thread_id`, never session id.
- Steer/inject targets exist in the queue but the UI only sends followups; the backend injects background-job notices.
- `Agent::messages` is transactional: never write partial turns into it. The UI transcript (`thread.messages`) is a separate projection.
- `serialActions.ts` serializes split-pane activate+send because both panes share one session controller.
- Environment text must stay out of the cached prompt prefix (`prompt::tests::env_context_is_not_part_of_the_composed_prefix`).
