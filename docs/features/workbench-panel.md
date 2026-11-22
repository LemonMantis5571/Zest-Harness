---
title: Workbench panel
summary: A project page with Changes, Files, Jobs, Checkpoints, Delegation, and Worktrees tabs, including checkpoint forks and a read-only file browser.
paths:
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/workspace/chrome.rs
  - crates/desktop/src/workspace_files.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace_files::
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::
  - npm run gpui:test
---

# Workbench panel

## Behavior
- The **Workbench** button in the chat header opens it on the Changes tab. It
  replaces the chat area with a page titled "Workbench"; **Back to chat** or Esc
  returns to the chat. Every window has a project, so it is available for every
  chat. Back and Forward in the sidebar also step through Workbench tabs.
- Tabs, in order: Changes, Files, Jobs, Checkpoints, Delegation, Worktrees. Each
  tab reads its data when it is opened; nothing refreshes while it stays open.
  The Workbench button always opens on Changes.
- Changes shows the active chat's Git changes since its start commit; see
  [workspace-changes-and-review](workspace-changes-and-review.md).
- Files is a read-only, shallow browser rooted at the project: directories first
  (shown with a trailing `/`), then files, max 400 entries per folder. Clicking a
  folder opens it, clicking a file shows its text, and **Up** goes back. A text
  preview is limited to 200000 bytes and then reads "Preview truncated (<n>
  bytes on disk)". A binary file shows "binary files do not have a text
  preview".
- The file browser hides `.git`, `.zest`, `target`, `node_modules`, `.venv`,
  `dist`, symlinks, and likely-secret files; asking for a sensitive file directly
  errors with "sensitive files are hidden from the workspace preview".
- No path can leave the project: `..`, absolute, and prefix components are
  refused before canonicalizing, and the canonical path must stay under the root.
- Jobs lists the active chat's background jobs as "<kind> · <status> ·
  <label>", or "No background jobs for this chat.", with a Copy button.
- Checkpoints lists the active chat's checkpoints newest first, each a "Fork
  from <label> · <preview>" button that opens the fork as a new chat. **Compact
  context** appears when the chat is idle and has agent history; it starts a
  compaction and returns to the chat. See
  [threads-and-history](threads-and-history.md).
- Delegation is the agent board for delegation jobs (create, approve, cancel,
  retry, apply); its behavior belongs to the delegation feature.
- Worktrees creates and lists worktree chats; see
  [worktree-chats](worktree-chats.md).
- The desktop app does not have the old Activity tab (provider state, subagent
  and tool task list, the `git diff --check` workspace check), the Outline tab,
  checkpoint rewind, or jumping from a delegation job in the sidebar to its card.

## How it works
1. `chrome.rs` defines the tabs in `WORKBENCH` and draws them in `page_frame`.
   The header's Workbench button calls `Workspace::page(Panel::Changes)`; Esc is
   the `ClosePanel` action, which returns to `Panel::Chat`.
2. `Workspace::page` (`crates/desktop/src/workspace.rs`) loads each tab:
   Changes spawns `workspace_changes::inspect`, Files calls `browse`, Jobs reads
   `JobRegistry::list(Some(thread))`, Delegation calls `load_board`, and
   Worktrees calls `load_worktrees`. `panel_body` draws the result. Each load
   bumps `report_epoch`, and a result for an older epoch is dropped.
3. `browse` runs `workspace_files::list` or `read` on the background executor
   (`resolve_path`, `contains_noise_directory`, `is_sensitive_path`,
   `decode_preview`).
4. A checkpoint button calls `Workspace::fork(Some(id))`; Compact context sends
   `Cmd::Compact` to the chat's command channel.

## Verify
`workspace_files::` tests cover parent paths, ordering and hidden folders, and
truncation at the byte limit and at a UTF-8 boundary. `npm run gpui:test` (and
the same scenario under `cargo test ... -- workspace::`) opens the Settings,
Search, Usage, Commands, Checkpoints and Jobs pages, forks from a checkpoint,
reads Changes before and after a commit, and in Files lists `demo.txt` and
previews its contents.

## Pitfalls
- The file browser's hidden-directory list (adds `.venv`, `dist`) differs from
  the agent tools' walk skip list (`.git`, `.zest`, `target`, `node_modules`).
- A UTF-8 character cut by the 200000-byte preview limit is trimmed rather than
  reported as binary; invalid UTF-8 elsewhere means "binary".
- Tabs share one `report` string and `report_epoch`. A new background load must
  check the epoch (and the panel) before writing, or a slow result from one tab
  can appear on another.
