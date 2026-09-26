---
title: Branch changes and diff review
summary: A project chat shows what changed on its branch since the chat started and opens a review pane with a cleaned-up or raw diff, with secret files redacted.
paths:
  - crates/core/src/workspace_changes.rs
  - crates/core/src/reading_diff.rs
  - crates/desktop/ui/src/components/BranchChangesBar.tsx
  - crates/desktop/ui/src/components/DiffViewer.tsx
  - crates/desktop/ui/src/lib/diffSections.ts
  - crates/desktop/ui/src/lib/readingDiff.ts
  - crates/desktop/ui/src/lib/visibleInterval.ts
tests:
  - crates/desktop/ui/src/lib/diffSections.test.ts
  - crates/desktop/ui/src/lib/readingDiff.test.ts
  - crates/desktop/ui/src/lib/visibleInterval.test.ts
verify:
  - cargo test -p zest-core --lib -- workspace_changes
  - cargo test -p zest-core --lib -- reading_diff
  - npm run ui:test
---

# Branch changes and diff review

## Behavior
- The branch diff compares the chat's `start_commit` (recorded in
  `ThreadGitContext` the first time `git_context` runs) with the working tree,
  including staged, unstaged, and untracked files. With no start commit it
  compares against `HEAD`, falling back to the index in a repo with no commits.
- Likely-secret files (`is_sensitive_path`) keep their name and counts but their
  body is replaced by `@@ sensitive file omitted @@`; an unparseable diff header
  is treated as sensitive. Untracked secret files are never read.
- The patch sent to the UI is capped at 2 MiB (`MAX_DISPLAY_DIFF_BYTES`,
  `truncated: true`); file list and counts stay complete.
- Not a git repo, git missing, or an unsafe base commit / ref returns an
  `unavailable` snapshot instead of an error. Commit ids must be 4-64 hex chars;
  refs cannot start with `-` or `.` or contain `..`.
- `BranchChangesBar` appears above the composer only when there are changed
  files **and** non-zero +/- counts, no diff pane is open, no turn is running,
  and this exact `changeId` was not dismissed. Dismissing hides it until the
  change id moves. It also links the branch's pull request when one is known.
- The review pane (`DiffViewer`) opens from the bar, from a tool row's diff, or
  from a pull request. It shows per-file sections with +/- counts, collapsible,
  resizable 360-760 px, Esc closes. View choice (Clean / Raw) is remembered per
  thread.
- While a branch diff is open it refreshes every 2.5 s, but not while the
  document is hidden (one refresh on becoming visible again). An open-diff flag
  is remembered per thread and restored only if there are still changes.
- Clean view: a local conservative version is shown at once (`makeReadingDiff`
  hides import lines, folds context runs longer than 4). The pane also asks the
  active provider for a remove/fold plan (`generate_reading_diff`); the model
  returns only line ranges inside hunks, applied locally and rejected if they
  touch metadata or cross diff markers. Raw always shows the exact patch.

## How it works
1. App.tsx refreshes changes when a project chat opens and on the
   `workspace_changed` event, which turn.rs emits when the post-turn inspection's
   change id differs from the pre-turn baseline.
2. `refreshWorkspaceChanges` -> backend `workspaceChanges` -> Tauri
   `workspace_changes` (crates/desktop/src/lib.rs) ->
   `zest_core::workspace_changes::inspect(root, start_commit, base_branch)`.
3. `inspect` runs `git status --porcelain=v1 -z`, fingerprints HEAD, branch,
   status, index (`ls-files --stage`), and the full content of every reported
   path (`worktree_content_fingerprint`), and serves an 8-entry LRU cache keyed on
   that fingerprint; otherwise `git diff --binary <base>`, appends untracked files
   (`format_untracked_diff`), then `redact_diff`, `bounded_utf8`, summaries.
4. ChatScreen.tsx owns `diffTarget` (`source: "tool" | "branch" |
   "pull_request"`), `openBranchChanges`, the `visibleInterval` poll, and dismiss
   keys in localStorage.
5. DiffViewer.tsx splits with `splitDiffSections`, builds `makeReadingDiff`, and
   calls `generateReadingDiff` (lib/api.ts) -> Tauri `generate_reading_diff` ->
   `zest_core::abridge_reading_diff` (reading_diff.rs `abridge` / `apply_plan`).

## Verify
`workspace_changes` tests include same-size content edits invalidating the cache
and an opt-in benchmark (`benchmark_repeated_inspection`, `--ignored`); see
plans/006-diff-refresh-results.md. Fixture mode (`?fixture=1`) serves a
synthetic `workspaceChanges` snapshot; `generateReadingDiff` bypasses the
fixture backend, so only the local Clean view appears there.

## Pitfalls
- Do not replace full-content hashing with size/mtime checks; same-size,
  same-timestamp edits would serve a stale diff (plans/006).
- Opening the Clean view sends the (redacted) diff to the active provider.
  `DiffViewer` re-runs that request whenever `target` changes, and each 2.5 s
  poll builds a new branch target object, so an open branch diff appears to call
  the provider (and reset collapsed sections) on every tick.
- `workspace_changed` only fires when a turn changes something; a branch that
  was already dirty is found by the refresh on chat open.
