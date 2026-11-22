---
title: Threads, history, and recovery
summary: Chats are saved per project, listed in the sidebar, searched, forked from checkpoints, and closed safely after an interrupted run.
paths:
  - crates/core/src/thread.rs
  - crates/core/src/thread_search.rs
  - crates/core/src/thread_window.rs
  - crates/core/src/chat_persistence.rs
  - crates/core/src/fsutil.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/workspace/chrome.rs
  - crates/desktop/src/workspace/recent.rs
  - crates/desktop/src/search.rs
  - crates/desktop/src/native.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- thread::characterization
  - cargo test -p zest-core --lib -- thread_search::tests
  - cargo test -p zest-core --lib -- thread_window::tests
  - cargo test -p zest-core --lib -- chat_persistence::tests
  - cargo test -p zest-core --lib -- fsutil::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- search::
  - npm run gpui:test
---

# Threads, history, and recovery

## Behavior
- Interactive HTML source is inline in conversation metadata/fences and follows existing save, checkpoint, fork and deletion ownership. New metadata uses thread format v5; older binaries are not compatible. Future-version files are rejected before typed decoding, not renamed as corrupt.
- Each chat is `<workspace>/.zest/threads/<id>.json`. A desktop window always has one project open (with no arguments it shows a folder picker). Chats started outside a project live in `<config dir>/zest/free-chats` and are reached through the sidebar's Recent section. A new chat gets no history row until its first message or queued input. Until then the sidebar shows it as a draft row: "New chat", or "Draft · <first line of the composer>".
- The sidebar lists the open project, as a Project tree (default) or, with Customize > Chat view > Compact list, one line per chat with a single mark (needs input, working, pull request, idle) under the project name. In the tree, the project row expands to the saved split groups, drafts and chats. Chats are ordered pinned first, then most recently updated. Each row has a glyph (a spinner while its turn runs, a pin, a pull request icon when one is linked, otherwise a message icon), the title, and a short age ("now", "45s", "6m", "8h", "2d").
- Worktree chats open as their own project window from Workbench > Worktrees; see [worktree-chats](worktree-chats.md).
- When a chat that is not on screen needs an approval or an answer, or its turn fails or finishes, the window shows an "Open · <kind> · <title>" banner. When the window is not focused it sends a system notification instead. Opening the chat clears its banners. The desktop app has no unread marks.
- The chat header has Pin/Unpin, Rename and Fork for a saved chat. Rename opens a page with the title selected; Enter saves, and an invalid title shows a notice. Fork does nothing while that chat's turn runs. The desktop app has no delete action for chats.
- Search (sidebar, or "Search messages for …" in the Ctrl+K palette) opens the Search page. Its field matches page names, chat titles and saved message text. A text hit opens the chat scrolled to the matching message, below that turn's reasoning and tool trace. Enter opens the first title match, else the first text hit, else the first matching page.
- A chat opens with its whole saved transcript in a virtualized list, so only visible rows are laid out. The desktop app does not page through turns; the `thread_window.rs` constants are used by the `workbench_io` timing example.
- Every submitted turn writes a "Before turn" checkpoint first, and compaction writes "Before compaction". Workbench > Checkpoints lists them newest first, each with its label and preview, **Fork** (a new chat from that point) and, while no turn runs, **Rewind**, plus "Compact context" when the chat is idle and has history.
- Rewind restores the chat in place: it asks first ("Discard later messages" or Cancel, with a note that files the agent changed stay as they are), then deletes the messages and checkpoints after that point, drops the provider session, reloads the transcript and toasts "Rewound to <label>." The desktop app cannot edit a sent user message.
- Opening a chat that was interrupted mid-turn closes its running tool, approval and provider rows and shows the load warning ("interrupted tools/approvals were closed after restart") as a notice. Queued follow-ups are saved in the thread and come back after a restart; **Resume queued** sends the oldest one. The desktop app does not put the interrupted prompt back into the composer.
- Run records in `ChatPersistence` carry a stable
  reason on an aborted run: provider changed/unavailable, durable resume
  unsupported, missing cursor, or an execution runtime detached by restart.
  A saved cursor alone never replays work or consumes quota. Existing providers
  do not support durable stream resume.
- A chat keeps the provider it first ran with. If that provider is no longer configured, sending fails with "Choose a configured provider. '<id>' is unavailable." Model & permissions shows only the owning provider and says to start a new chat for a different one. There is no flow to copy a chat to another provider.
- Back and Forward in the sidebar header walk the places visited, each a page and a chat.

- The sidebar's Recent section ("No project" in the compact list) lists the chats with messages in `<config dir>/zest/free-chats`, newest first, up to 30. Opening one opens or focuses a window rooted at that folder, because a native window holds one project.

## How it works
- Storage: `ThreadStore` (`thread.rs`) `save` via `fsutil::atomic_write_json` (temp file, sync, atomic replace). `load_typed` migrates older versions, refuses newer ones, moves corrupt files aside as `*.json.corrupt-<ts>`, and `terminalize_interrupted_at` closes running tool/approval cards and running provider activity, then saves the closed projection. `Thread` holds UI `messages`, redacted `agent_messages`, `checkpoints`, `pending_inputs`, and a lifecycle `events` ledger.
- An assistant `StoredMessage` keeps `tools`, `providerActivity` (what a provider-owned loop such as Claude Code reported, with an optional `parentId`) and `steps`, the order thinking blocks, tools and activities arrived in. Tools and activities carry `startedAt`/`finishedAt` (Unix ms). All of these are optional, so older threads load unchanged and show in the old fixed order.
- Live cancellation or failure records the end time of interrupted tools and provider activity. Restart recovery closes unfinished rows without inventing when the old process stopped; missing end times remain missing on subsequent reopens, and terminal subagent rows with an unknown end hide their duration.
- Listing: `Workspace::load_history` (`crates/desktop/src/workspace.rs`) runs `ThreadStore::list` in the background at startup and when Search opens. Saves that land during the scan are kept in `loading_history` and merged by `merge_history`, so a slow scan cannot overwrite a newer title or pin. After that, each chat's `ChatChanged` event updates its row in place (`update_history`, ordered by `history_order`). `chrome.rs` draws the rows with `chat_row` and the age with `age`.
- Search: both clients use `ThreadStore::search_chats`; the desktop calls it from `search::search` after a 180 ms pause in typing, with a cancel token. Each workspace keeps a rebuildable SQLite trigram index in `.zest/threads/chat-search-v1.sqlite3`. A search backfills existing threads, refreshes files that changed, and removes externally deleted files. Streaming saves do not write the index. There is no thread-size cutoff. The index covers titles, visible user/assistant text, and legacy standalone thinking rows. Results retain their message IDs, with one hit per chat and a limit of 24. Deleting a chat through the store also removes its indexed rows. `open_hit` opens the chat and calls `reveal_message`.
- Turns: `Chat::run` (`crates/desktop/src/native.rs`) claims the queued input, writes the checkpoint with `create_checkpoint_with_metadata`, records a run with `runs.create_or_resume_for_turn`, saves the transcript at most every 250 ms while streaming, and marks the run completed, aborted or failed at the end.
- Checkpoints: snapshots live under `.zest/threads/checkpoints/<thread>/`, capped at 24 snapshots and 64 MiB (oldest automatic first, never the last one). `Workspace::fork` calls `ThreadStore::fork` or `fork_from_checkpoint`. `Workspace::rewind` calls `Chat::rewind` (`native.rs`), which runs `ThreadStore::rewind_to_checkpoint` and replaces the open thread, then rebuilds the view with `restore_native`. `rewind_before_user_message` remains in core with no caller.
- Provider ownership: `Thread::assert_provider` and `assert_provider_kind` run before every turn, and `Workspace::select_model` refuses a provider change on a chat that already has one.
- Window: `MessageWindow::{tail, around, before, after}` (`thread_window.rs`) cut on user-turn boundaries.

## Verify
- `npm run gpui:test` runs the workspace scenario in a native window with the offline provider: two saved turns with two checkpoints, a fork from a checkpoint, a reopen that keeps the wire context, a stop with a queued input that survives reopen, editing and sending that queued input, aborted and completed run records, and a transcript search hit that opens the chat. `cargo test ... -- workspace::` runs the same scenario in a GPUI test window, plus `merge_history`.
- `cargo test ... -- search::` checks that a saved message is found with its id and excerpt.
- Rust core tests cover round trips, migration, corrupt files, checkpoint bounds, rewind/edit, recovery reconciliation, and delete cleanup.
- The shared persistence helper also provides a directory-synced JSON write
  for command receipts that must survive a system crash before a side effect.

## Pitfalls
- Atomic saves on Windows briefly retry a locked destination while retaining the completed temporary file. A persistent lock returns an error and leaves the previous revision available.
- Forks drop `pending_inputs` and start unpinned.
- A fork from a checkpoint works only while that checkpoint survives the 24 / 64 MiB cap.
- Rewind/edit in core clear the provider session cursor on purpose; the next turn replays history.
- Route by thread id. Each open chat owns its own `Root` view and command channel, keyed by thread id in `Workspace::chats`.
