---
title: Threads, history, and recovery
summary: Chats are saved per project, listed in the sidebar, reopened a page at a time, rewound or forked from checkpoints, and recovered safely after an interrupted run.
paths:
  - crates/core/src/thread.rs
  - crates/core/src/thread_window.rs
  - crates/core/src/chat_persistence.rs
  - crates/core/src/fsutil.rs
  - crates/desktop/ui/src/components/ChatHistorySidebar.tsx
  - crates/desktop/ui/src/components/ChatSkeleton.tsx
  - crates/desktop/ui/src/components/CheckpointRail.tsx
  - crates/desktop/ui/src/components/ConversationRecoveryDialog.tsx
  - crates/desktop/ui/src/lib/threadWindow.ts
  - crates/desktop/ui/src/lib/threadActivity.ts
  - crates/desktop/ui/src/lib/conversationTurns.ts
  - crates/desktop/ui/src/lib/chatView.ts
  - crates/desktop/ui/src/lib/navigationHistory.ts
  - crates/desktop/ui/src/lib/invokeErrors.ts
tests:
  - crates/desktop/ui/src/lib/threadWindow.test.ts
  - crates/desktop/ui/src/lib/threadActivity.test.ts
  - crates/desktop/ui/src/lib/conversationTurns.test.ts
  - crates/desktop/ui/src/lib/chatView.test.ts
  - crates/desktop/ui/src/lib/navigationHistory.test.ts
  - crates/desktop/ui/e2e/sidebar-search.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- sidebar-search
  - cargo test -p zest-core --lib -- thread::characterization
  - cargo test -p zest-core --lib -- thread_window::tests
  - cargo test -p zest-core --lib -- chat_persistence::tests
  - cargo test -p zest-core --lib -- fsutil::tests
  - cargo test -p zest-desktop --lib -- chat_recovery_tests
  - cargo test -p zest-desktop --lib -- delete_thread_tests
  - cargo test -p zest-desktop --lib -- chat_summary_tests
  - cargo test -p zest-desktop --lib -- chat_search_tests
---

# Threads, history, and recovery

## Behavior
- Each chat is `<workspace>/.zest/threads/<id>.json` (format v4). Chats opened without a project live in the "Free chats" bucket (`<zest config dir>/free-chats`). A new chat gets no history row until its first message.
- The sidebar lists known projects plus Free chats; within a project, pinned chats first, then most recently updated. Rows show live activity (working, awaiting approval, elapsed time) for chats running in the background.
- Rows support rename (Enter/blur saves, empty cancels, disabled on the row whose turn is live), pin/unpin, and delete (with confirmation). The open chat's row also has "Fork conversation" (disabled while it is working). Deleting a busy chat cancels its turn; deleting the open chat switches to an unsaved draft.
- "Search chats" opens the shared command palette on its Chats tab; hits can open a chat scrolled to the matching message.
- A chat opens on its last 10 user turns. Scrolling to the top (or "Earlier turns") loads 20 more without moving the viewport; "Later turns" pages forward after opening around a search hit.
- Every submitted turn writes a checkpoint first ("Conversation start" / "Before turn"). The rail beside the transcript lists user turns for jumping; rewinding a checkpoint (Workbench) restores the transcript and model history but never touches workspace files.
- Editing a user message rewinds to just before it and resends the edited text as a new turn.
- If the app died mid-turn, reopening the chat closes the stale run and pending approvals, toasts "Previous turn ready to retry", and puts that turn's prompt into an empty composer.
- Recovery consults the built provider's resume capability and records a stable
  reason on the aborted run: provider changed/unavailable, durable resume
  unsupported, missing cursor, or an execution runtime detached by restart.
  A saved cursor alone never replays work or consumes quota. Existing providers
  do not support durable stream resume, so recovery continues to offer retry.
- A chat whose saved provider is unknown or unavailable opens `ConversationRecoveryDialog`: choose a provider (older chats), open a copy with another provider, or configure the original. The original chat is left unchanged.
- Sidebar layout is "project" or "compact" (`zest.chatViewMode`, set in Customize > Chat). Back/Forward in the sidebar header walk shell panels (`navigationHistory.ts`), not chats.

## How it works
- Storage: `ThreadStore` (`thread.rs`) `save` via `fsutil::atomic_write_json` (temp file, sync, atomic replace). `load_typed` migrates older versions, refuses newer ones, moves corrupt files aside as `*.json.corrupt-<ts>`, and `terminalize_interrupted` closes running tool/approval cards. `Thread` holds UI `messages`, redacted `agent_messages`, `checkpoints`, `pending_inputs`, and a lifecycle `events` ledger.
- Listing: Tauri `list_chat_projects` / `list_cached_threads` (`crates/desktop/src/lib.rs`) read summaries through a per-file mtime cache, hide zero-message chats, sort pinned then `updated_at`. `search_chats` scans titles and transcripts (24 hits max). `ChatHistorySidebar.tsx` reloads on open, focus, and chat changes.
- Windows: `MessageWindow::{tail, around, before, after}` (`thread_window.rs`) cut on user-turn boundaries; `load_older_thread_messages` / `load_newer_thread_messages` serve pages from the live turn or idle session. `threadWindow.ts` mirrors the constants and merges pages in `App.tsx`; `ChatScreen.tsx` preserves scroll offset when prepending.
- Checkpoints: `create_checkpoint_with_metadata` snapshots under `.zest/threads/checkpoints/<thread>/`, capped at 24 snapshots and 64 MiB (oldest automatic first, never the last one). `rewind_thread` -> `rewind_to_checkpoint`; `edit_message` -> `rewind_before_user_message`; `fork_thread` / `fork_thread_from_checkpoint` -> `ThreadStore::fork*`. `conversationTurns.ts` builds the rail index for `CheckpointRail.tsx`.
- Recovery: `turn.rs` records runs and approval/question interrupts in `ChatPersistence` (`.zest/runs/`, `.zest/interrupts/`). On open, `recover_chat_on_load` calls `reconstruct_chat_from_thread` (finds a `RecoverableRun` whose user message is still in the transcript) and `reconcile_after_restart` (marks non-terminal runs aborted, cancels pending interrupts). `SessionInfo.recovery` drives the composer refill in `App.tsx` `applySession`.
- Provider ownership: `Thread::assert_provider` / `ThreadLoadError::ProviderMismatch`; the backend's structured error is parsed by `conversationRecovery` (`invokeErrors.ts`).
- Delete: `delete_thread_inner` tombstones the `PersistWorker` (`forget`) before cancelling, then `ThreadStore::delete` (JSON, checkpoints, spilled tool output) and `ChatPersistence::forget_thread`.

## Verify
- `?fixture=1`: the "Fifteen turns" chat exercises the 10-turn window and "Earlier turns"; a background chat shows the sidebar activity mark; `sidebar-search.spec.ts` covers search and the project menu.
- Rust tests cover round trips, migration, corrupt files, checkpoint bounds, rewind/edit, recovery reconciliation, and delete cleanup.

## Pitfalls
- A user message can be edited only while its checkpoint survives; once pruned past the 24/64 MiB cap, `edit_message` fails with "rewind checkpoint is missing".
- Forks drop `pending_inputs` and start unpinned.
- Rewind/edit clear the provider session cursor on purpose; the next turn replays history.
- Keep `THREAD_WINDOW_USER_TURNS` / `THREAD_OLDER_USER_TURNS` identical in Rust and `threadWindow.ts`.
- Session ids are runtime identities; a reopened chat gets a new one while its old turn may still be finishing. Route by thread id.
