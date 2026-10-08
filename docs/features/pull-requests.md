---
title: Linked pull requests
summary: Zest detects the GitHub pull request for a chat's branch, lists linked pull requests across projects, and opens their diff in the review pane.
paths:
  - crates/desktop/ui/src/components/PullRequestsPanel.tsx
  - crates/desktop/ui/src/lib/pullRequestLink.ts
  - crates/desktop/ui/src/components/BranchChangesBar.tsx
  - crates/core/src/workspace_changes.rs
tests:
  - crates/desktop/ui/src/lib/pullRequestLink.test.ts
  - crates/desktop/ui/e2e/pull-requests.spec.ts
verify:
  - cargo test -p zest-desktop --lib -- pull_request_number
  - cargo test -p zest-core --lib -- workspace_changes
  - npm run ui:test
  - npm run ui:e2e -- pull-requests
---

# Linked pull requests

## Behavior
- A chat learns its pull request from `gh pr view --json
  number,title,state,isDraft,url,additions,deletions,changedFiles` run in the
  project on the current branch (3 s timeout). The result is stored in the
  thread's `ThreadGitContext.pull_request`, so it survives restarts.
- When a pull request is linked, `git_context` reports its +/- and file counts
  (`stats_source: "pull_request"`) instead of the local `git diff --numstat`.
- The Pull requests view lists every linked pull request across all projects,
  newest chat first, one row per URL. Search matches project, title, number, and
  branch; status filters are All, Open, Draft (open and draft), Merged, Closed.
  Refresh reloads; the empty state explains how a PR gets linked.
- A plain click on a pull request title opens its chat (switching project if
  needed) and the diff in the review pane, showing "Fetching the pull request
  diff..." until it arrives. Ctrl/Cmd/Shift-click or middle-click opens the URL
  in the browser instead. The external-link icon always opens GitHub.
- The diff comes from `gh pr diff <n>` (30 s timeout); if that fails it falls back
  to `git diff <base_branch>...HEAD`. Both go through the same redaction and
  2 MiB cap as branch changes. If the result is unavailable or empty, or the
  chat is a free chat, the URL opens externally.
- PR numbers must be 1-1000000; a requested number wins over the stored one.

## How it works
1. `git_context` (Tauri, crates/desktop/src/lib.rs) -> `inspect_git_context` ->
   `lookup_pull_request_cached` -> `lookup_pull_request`; `record_git_context`
   saves it on an existing thread.
2. `PullRequestsPanel` (lazy in ChatScreen.tsx) calls backend
   `listChatProjects()` and reads `thread.gitContext.pullRequest`.
3. Links use `pullRequestAnchorProps` (marks `data-internal-link`, which
   externalLinks.ts respects) and `handlePullRequestClick` /
   `shouldOpenPullRequestExternally` (pullRequestLink.ts). The same helpers serve
   BranchChangesBar, Composer, and ChatHistorySidebar.
4. ChatScreen `requestPullRequest` switches chats if needed (holding the link in
   `pendingPullRequestRef`) and calls `openPullRequestDiff` -> backend
   `pullRequestDiff(number)` -> Tauri `pull_request_diff` ->
   `fetch_github_pull_request_diff` + `snapshot_from_unified_diff`, or
   `inspect_merge_base` (workspace_changes.rs).
5. The diff renders in `DiffViewer` with `source: "pull_request"` (see
   workspace-changes-and-review).

## Verify
The e2e specs run in `?fixture=1` (fixture PR #13, "Bot CI"): filtering and
search, opening the review, back/forward navigation, and
`&scenario=pull-request-delayed` for the loading state.
The delayed test adds `&holdPullRequestDiff=1` to hold the fixture response.
It checks the loading message and disabled Clean/Raw controls, then releases
the response to verify the diff appears and the controls become enabled.
This keeps lazy-loaded UI components running and does not rely on the CI
runner observing the fixture's short loading window in real time.

## Pitfalls
- `gh` must be installed and authenticated; without it no PR is detected and
  `pull_request_diff` falls back to the merge-base diff.
- `PULL_REQUEST_CACHE` is a single process-wide slot keyed by (root, branch) that
  also caches "not found". A PR opened after the first lookup is not detected
  until the branch or project changes or the app restarts.
- A stale `pullRequestLoadRef` response is dropped; keep incrementing it when the
  pane closes or the chat changes.
