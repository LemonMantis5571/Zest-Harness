---
title: Linked pull requests
summary: Zest detects the GitHub pull request for a chat's branch, lists linked pull requests across projects, and shows their checks and diff.
paths:
  - crates/core/src/pull_request.rs
  - crates/core/src/workspace_changes.rs
  - crates/desktop/src/workspace/projects.rs
verify:
  - cargo test -p zest-core --lib -- pull_request::tests
  - cargo test -p zest-core --lib -- workspace_changes
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::projects::
---

# Linked pull requests

## Behavior
- A chat learns its pull request from `gh pr view --json
  number,title,state,isDraft,url,additions,deletions,changedFiles` run in the
  project on the current branch (3 s timeout). The desktop app looks it up when
  the Pull requests page opens, for the active chat, and only when the chat's
  recorded branch is the branch checked out now. The result is stored in the
  thread's `ThreadGitContext.pull_request`, so it survives restarts.
- Once linked, the pull request number shows in the branch strip above the
  composer and in the status line below it, and the chat's sidebar row uses a
  pull request icon. The strip's +/- counts stay the local `git diff --numstat`
  counts.
- The Pull requests page (sidebar) lists linked pull requests from the open
  project, the projects in `<zest config dir>/known-workspaces.json`, and visible
  worktree chats, newest chat first, one row per URL. Each row reads
  "<project> · #<n> <title> · <branch> · <state>". The filter field matches that
  text; status filters are All, Open, Draft (open and draft), Merged, Closed.
  Refresh reloads. The empty state says to open the page from a Git chat with a
  pull request on its branch.
- Clicking a row opens its chat and the review on the same page. A row from
  another project opens that project in its own window (or brings an open one to
  the front) and starts the review there. Rows have no "open in browser"
  modifier; use **Open on GitHub** in the review.
- The review shows "#<n> <title> · <chat title>", **All pull requests**, **Open
  on GitHub**, and **Refresh checks**. Checks read "<state> · <n> checks", with
  one row per check; a check with a link opens its log or details page. The
  checks refresh every 30 seconds while the review is shown and on **Refresh
  checks**. "Fetching checks…" and "Fetching pull request diff…" show while each
  request runs, and one failing leaves the other available.
- The diff comes from `gh pr diff <n>` (30 s timeout); if that fails it falls
  back to `git diff <base_branch>...HEAD`. Both go through the same redaction
  and 2 MiB cap as branch changes. The review shows "<n> files · +<a> −<d>" and
  the patch 80 lines at a time with **Previous 80 lines** / **Next 80 lines**.
  An unavailable diff reads "Pull request diff unavailable."
- PR numbers must be 1-1000000.

## How it works
1. `Workspace::load_pull_requests` (`crates/desktop/src/workspace/projects.rs`)
   compares `chat_git_baseline`'s branch with the chat's `git_context.branch`,
   calls `zest_core::pull_request::lookup_pull_request_cached`, and saves a new
   link with `Chat::edit`. It then runs `list_links` on a blocking thread, which
   reads each project's `ThreadStore::list` summaries and keeps rows whose
   `git_context.pull_request` passes `is_safe_pr_number`. Hidden worktree records
   are skipped.
2. `open_linked_chat` opens the thread in this window, or calls `visit_project`,
   which opens the other project's window with `open_project_window` and then
   opens the chat there.
3. `begin_review` spawns `zest_core::pull_request::diff(root, number, context)`
   (`fetch_github_pull_request_diff` + `snapshot_from_unified_diff`, or
   `inspect_merge_base` from workspace_changes.rs) and `refresh_checks`, which
   loops on `pull_request::status` (`gh pr view <n> --json
   state,url,statusCheckRollup`, 10 s timeout) every 30 s until the page or its
   `report_epoch` changes.
4. Links open through `zest_core::codex_oauth::open_http_url`, which accepts only
   HTTP(S) URLs.

## Verify
`pull_request::tests` covers parsing running and completed checks and external
statuses. `workspace::projects::` checks the worktree count label. There is no
offline end-to-end test for pull requests: in a repository with a pushed branch
and an open pull request, open a chat on that branch, open Pull requests, and
check that the row appears, that clicking it shows the checks and the diff, and
that the chat's branch strip shows the number afterwards.

## Pitfalls
- `gh` must be installed and authenticated; without it no PR is detected and
  the diff falls back to the merge-base diff.
- `PULL_REQUEST_CACHE` is a single process-wide slot keyed by (root, branch) that
  expires after 30 seconds, including a missing PR. Reopening the page after that
  interval can discover a newly opened PR.
- Nothing in the desktop app writes `known-workspaces.json`, so projects only
  ever opened in the desktop app (other than the open one and worktree chats)
  are not listed.
- A late diff or check result is dropped when `report_epoch` has moved; keep
  incrementing it whenever the page changes.
