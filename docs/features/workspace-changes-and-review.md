---
title: Branch changes and diff review
summary: A project chat shows what changed on its branch since the chat started, and Workbench > Changes shows the full diff with secret files redacted.
paths:
  - crates/core/src/workspace_changes.rs
  - crates/core/src/reading_diff.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/workspace.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- workspace_changes
  - cargo test -p zest-core --lib -- reading_diff
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::
  - npm run gpui:test
---

# Branch changes and diff review

## Behavior
- The branch diff compares the chat's `start_commit` with the working tree,
  including staged, unstaged, and untracked files. Every new desktop chat
  records the project's current commit and branch in `ThreadGitContext` when it
  is created (`chat_git_baseline`), so a split or a chat started after a commit
  has its own baseline. With no start commit (an older chat) it compares against
  `HEAD`, falling back to the index in a repo with no commits.
- Likely-secret files (`is_sensitive_path`) keep their name and counts but their
  body is replaced by `@@ sensitive file omitted @@`; an unparseable diff header
  is treated as sensitive. Untracked secret files are never read.
- The patch is capped at 2 MiB (`MAX_DISPLAY_DIFF_BYTES`, `truncated: true`);
  file list and counts stay complete.
- Not a git repo, git missing, or an unsafe base commit / ref returns an
  `unavailable` snapshot instead of an error. Commit ids must be 4-64 hex chars;
  refs cannot start with `-` or `.` or contain `..`.
- The branch strip appears above the composer only when there are changed files
  **and** non-zero +/- counts, no turn is running, and these exact counts were
  not dismissed. It shows the project name, the branch, the +/- counts and the
  linked pull request number when one is known. **Hide branch bar** hides it
  until the counts change. Its counts come from
  `zest_core::workspace_changes::diff_stats` (`git diff --numstat` since the
  chat's start commit). The strip is not a link; open Workbench > Changes for
  the diff.
- The counts refresh when a chat view opens and when each of its turns ends.
  Nothing polls in the background.
- **Workbench > Changes** (the header's Workbench button opens on it) reads the
  diff each time the tab is opened. It shows "Reading Git changes…", then
  "<n> files · +<a> −<d>", one "<status>  <path>" line per file, and the full
  patch in a monospace block with a Copy button, ending in "[Patch truncated]"
  when capped. A clean tree reads "Working tree clean."; an unavailable snapshot
  reads "Git changes unavailable for this folder."
- A tool row that edited a file shows its diff when expanded in the transcript.
  A pull request's diff is shown on the Pull requests page; see
  [pull-requests](pull-requests.md).
- The desktop app has no separate review pane: no per-file collapsible sections,
  no resizable diff panel, and no Clean view. `reading_diff.rs` (a model-made
  plan that folds and hides lines for reading) stays in core, but no client
  calls it now.

## How it works
1. `Workspace::new_chat` (`crates/desktop/src/workspace.rs`) calls
   `zest_core::worktree_chat::chat_git_baseline` and stores the commit and branch
   with `Thread::ensure_git_context`.
2. `Root::refresh_git_stats` (`crates/desktop/src/native_view.rs`) runs
   `diff_stats(root, start_commit)` and reads the checkout's branch on the shared
   Tokio executor. `Root::open_chat` calls it, and so does the turn's `End` event
   in `main.rs`. `branch_strip` draws the strip and keeps the dismissed counts in
   `strip_hidden`.
3. `Workspace::page(Panel::Changes)` spawns
   `zest_core::workspace_changes::inspect(root, start_commit, base_branch)` and
   formats the result into `report`. A `report_epoch` counter drops a result that
   arrives after the user moved to another page.
4. `inspect` runs `git status --porcelain=v1 -z`, fingerprints HEAD, branch,
   status, index (`ls-files --stage`), and the full content of every reported
   path (`worktree_content_fingerprint`), and serves an 8-entry LRU cache keyed on
   that fingerprint; otherwise `git diff --binary <base>`, appends untracked files
   (`format_untracked_diff`), then `redact_diff`, `bounded_utf8`, summaries.

## Verify
`workspace_changes` tests include same-size content edits invalidating the cache
and an opt-in benchmark (`benchmark_repeated_inspection`, `--ignored`); see
plans/006-diff-refresh-results.md. `npm run gpui:test` (and the same scenario
under `cargo test ... -- workspace::`) checks a clean Changes report, commits an
approved edit, and checks that the first chat's Changes report still shows the
committed line, also after the chat is closed and reopened. A split opened after
the commit reads "Working tree clean.".

## Pitfalls
- Do not replace full-content hashing with size/mtime checks; same-size,
  same-timestamp edits would serve a stale diff (plans/006).
- The Changes tab does not refresh itself. A turn that edits files while the tab
  is open is shown only after the tab is opened again.
- A chat without a recorded start commit compares against `HEAD`, so its Changes
  view empties after a commit. Only chats created by the desktop app (or another
  client that records a baseline) keep committed work visible.
