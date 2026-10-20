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
  - crates/desktop/ui/src/lib/threadUnread.ts
  - crates/desktop/ui/src/lib/conversationTurns.ts
  - crates/desktop/ui/src/lib/chatView.ts
  - crates/desktop/ui/src/lib/compactChatRow.ts
  - crates/desktop/ui/src/lib/chatStatusGroups.ts
  - crates/desktop/ui/src/lib/delegationChildren.ts
  - crates/desktop/ui/src/lib/navigationHistory.ts
  - crates/desktop/ui/src/lib/invokeErrors.ts
tests:
  - crates/desktop/ui/src/lib/threadWindow.test.ts
  - crates/desktop/ui/src/lib/threadActivity.test.ts
  - crates/desktop/ui/src/lib/threadUnread.test.ts
  - crates/desktop/ui/src/lib/conversationTurns.test.ts
  - crates/desktop/ui/src/lib/chatView.test.ts
  - crates/desktop/ui/src/lib/compactChatRow.test.ts
  - crates/desktop/ui/src/lib/chatStatusGroups.test.ts
  - crates/desktop/ui/src/lib/delegationChildren.test.ts
  - crates/desktop/ui/src/lib/navigationHistory.test.ts
  - crates/desktop/ui/e2e/sidebar-search.spec.ts
  - crates/desktop/ui/e2e/compact-sidebar.spec.ts
  - crates/desktop/ui/e2e/sidebar-status.spec.ts
verify:
  - npm run ui:test
  - npm run ui:e2e -- sidebar-search
  - npm run ui:e2e -- compact-sidebar
  - npm run ui:e2e -- sidebar-status
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
- Interactive HTML source is inline in conversation metadata/fences and follows existing save, checkpoint, fork and deletion ownership. New metadata uses thread format v5; older binaries are not compatible. Future-version files are rejected before typed decoding, not renamed as corrupt.
- Each chat is `<workspace>/.zest/threads/<id>.json` (format v4). Chats opened without a project live in the "Free chats" bucket (`<zest config dir>/free-chats`). A new chat gets no history row until its first message.
- The sidebar lists known projects plus Free chats; within a project, pinned chats first, then most recently updated. Rows show live activity (working, awaiting approval, waiting on an answer to a question, elapsed time) for chats running in the background; approval and a pending question both count as needing input (`needsInput` in `threadActivity.ts`).
- A project menu can start a chat in its own persistent Git worktree. The worktree appears as a separate workspace in either sidebar layout, with the source project name and current branch; its chat history stays under that checkout. Worktree records remain discoverable independently of the 40-entry ordinary-project list, and removing one from the sidebar marks its record hidden without deleting files. See [worktree-chats](worktree-chats.md).
- A chat whose turn ends (done or error, not cancel) while its transcript is off screen reads as bold in the compact list until it becomes visible. Every pane in the visible split group counts as seen, regardless of which chat owns the backend-active session. Returning to a split group clears unread marks for all its visible panes. The set lives in localStorage (`zest.unreadThreads`, capped at 200), so it survives a restart on this machine; a turn that ends while the app is closed is not marked.
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
- Compact layout: one heading tier (the project name, with an always-visible +; projectless chats sit under "No project"), an empty project is only its heading, and each chat is one 28 px line: a leading mark from `compactChatMark` (needs input > working > pull request > changed branch > idle circle), the title (shimmering while the turn works, plain text under reduced motion; bold while unread), and only the exceptions — a provider logo when the chat is on a different provider than the session (`foreignProvider`), a pin when pinned, elapsed time while running. Pull request, pin, fork, and delete are hover actions. "Open a project folder" ends the list.
- Compact grouping: a "By project" / "By status" toggle at the top of the compact list (`zest.chatGrouping`, project by default). By status, every chat from every project falls into "Needs input" (awaiting approval or a pending question), "Working", or "Recent", and rows move between them the moment `threadActivity` changes. Empty live sections are hidden. Needs input and Working put the newest turn first (by turn start, so running rows hold still); Recent is pinned first, then most recently updated. Each row carries its project ("No project" for free chats) as a quiet trailing label. Rename, pin, delete, fork, and Tab/Enter work as in the project grouping.
- Delegation jobs in the compact list (both groupings): each feature-card job of the open project hangs under the chat that created it (`parentThreadId`), joined by a tree line from under the row's mark. A child row shows the job's status in words with a matching mark (needs approval, working, reviewing, ready to apply, queued, applied, failed, and so on); running jobs shimmer like working chats. Unsettled jobs come first, newest first, and past three the rest fold into "N more". Clicking a child opens the Workbench on its Delegation tab (project filter) with that job's card focused and outlined. Jobs come from the open project only, so chats in other projects show none.

## How it works
- Storage: `ThreadStore` (`thread.rs`) `save` via `fsutil::atomic_write_json` (temp file, sync, atomic replace). `load_typed` migrates older versions, refuses newer ones, moves corrupt files aside as `*.json.corrupt-<ts>`, and `terminalize_interrupted` closes running tool/approval cards and running provider activity. `Thread` holds UI `messages`, redacted `agent_messages`, `checkpoints`, `pending_inputs`, and a lifecycle `events` ledger.
- An assistant `StoredMessage` keeps `tools`, `providerActivity` (what a provider-owned loop such as Claude Code reported, with an optional `parentId`) and `steps`, the order thinking blocks, tools and activities arrived in. Tools and activities carry `startedAt`/`finishedAt` (Unix ms). All of these are optional, so older threads load unchanged and show in the old fixed order.
- Live cancellation or failure records the end time of interrupted tools and provider activity. Restart recovery closes unfinished rows without inventing when the old process stopped; missing end times remain missing on subsequent reopens, and terminal subagent rows with an unknown end hide their duration.
- Unread: `threadUnread.ts` folds every chat event using `App.tsx`'s `isChatOnScreen`, which reads all actually visible panes rather than the single backend-active session. `visibleChatThreads` also drives clearing with `markThreadSeen` whenever the visible set changes, including split-group reentry. `ThreadSummary` has no seen marker; `updatedAt` moves on every saved message and cannot say which chat was on screen. The sidebar reads the set with `useSyncExternalStore`.
- Listing: Tauri `list_chat_projects` / `list_cached_threads` (`crates/desktop/src/lib.rs`) read summaries through a per-file mtime cache, hide zero-message chats, sort pinned then `updated_at`. `search_chats` scans titles and transcripts (24 hits max). `ChatHistorySidebar.tsx` reloads on open, focus, chat changes, and whenever a chat leaves the busy set, so a turn that ended in the background brings its new `updatedAt` and title.
- Status grouping: `groupChatsByStatus` (`chatStatusGroups.ts`) flattens `ProjectChats` into the three sections from `ThreadActivityMap`; `chatProjectLabel` names the row's project. The toggle is saved by `saveChatGrouping` in `chatView.ts`.
- Delegation children: `ChatScreen.tsx` passes its `delegationJobs` (App's list for the open project, kept live by `handleDelegationEvent`) to the sidebar; `delegationChildrenByThread` (`delegationChildren.ts`) files and caps them, and `delegationChildStatus` words each state. A child click calls `openDelegationJob`, which opens the Workbench with `focusDelegationJob`. The row's hover actions belong to the row (`group/thread` is on the row, not the list item), so hovering a child does not reveal them.
- Windows: `MessageWindow::{tail, around, before, after}` (`thread_window.rs`) cut on user-turn boundaries; `load_older_thread_messages` / `load_newer_thread_messages` serve pages from the live turn or idle session. `threadWindow.ts` mirrors the constants and merges pages in `App.tsx`; `ChatScreen.tsx` preserves scroll offset when prepending.
- Checkpoints: `create_checkpoint_with_metadata` snapshots under `.zest/threads/checkpoints/<thread>/`, capped at 24 snapshots and 64 MiB (oldest automatic first, never the last one). `rewind_thread` -> `rewind_to_checkpoint`; `edit_message` -> `rewind_before_user_message`; `fork_thread` / `fork_thread_from_checkpoint` -> `ThreadStore::fork*`. `conversationTurns.ts` builds the rail index for `CheckpointRail.tsx`.
- Recovery: `turn.rs` records runs and approval/question interrupts in `ChatPersistence` (`.zest/runs/`, `.zest/interrupts/`). On open, `recover_chat_on_load` calls `reconstruct_chat_from_thread` (finds a `RecoverableRun` whose user message is still in the transcript) and `reconcile_after_restart` (marks non-terminal runs aborted, cancels pending interrupts). `SessionInfo.recovery` drives the composer refill in `App.tsx` `applySession`.
- Provider ownership: `Thread::assert_provider` / `ThreadLoadError::ProviderMismatch`; the backend's structured error is parsed by `conversationRecovery` (`invokeErrors.ts`).
- Delete: `delete_thread_inner` tombstones the `PersistWorker` (`forget`) before cancelling, then `ThreadStore::delete` (JSON, checkpoints, spilled tool output) and `ChatPersistence::forget_thread`.

## Verify
- `?fixture=1&scenario=sidebar-status` (compact view): chats waiting on an approval and on a question, one running, and one finished in the background (bold); `zest:fixture-finish-working` ends the running one. The open chat has four jobs (three shown, "1 more") and "Pick a migration path" has one. `sidebar-status.spec.ts` covers grouping, live moves, unread, nesting and the Workbench jump, pin, rename, and keyboard access.
- `?fixture=1`: the "Fifteen turns" chat exercises the 10-turn window and "Earlier turns"; a background chat shows the sidebar activity mark; `sidebar-search.spec.ts` covers search and the project menu.
- Rust tests cover round trips, migration, corrupt files, checkpoint bounds, rewind/edit, recovery reconciliation, and delete cleanup.
- The shared persistence helper also provides a directory-synced JSON write
  for command receipts that must survive a system crash before a side effect.

## Pitfalls
- A user message can be edited only while its checkpoint survives; once pruned past the 24/64 MiB cap, `edit_message` fails with "rewind checkpoint is missing".
- Forks drop `pending_inputs` and start unpinned.
- Rewind/edit clear the provider session cursor on purpose; the next turn replays history.
- Keep `THREAD_WINDOW_USER_TURNS` / `THREAD_OLDER_USER_TURNS` identical in Rust and `threadWindow.ts`.
- Session ids are runtime identities; a reopened chat gets a new one while its old turn may still be finishing. Route by thread id.
