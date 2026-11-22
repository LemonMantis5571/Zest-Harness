---
title: Worktree chats
summary: Start parallel coding chats in separate Git branches and checkouts, using any configured provider.
paths:
  - crates/core/src/worktree_registry.rs
  - crates/desktop/src/workspace/projects.rs
  - crates/core/src/worktree_chat.rs
  - crates/core/src/prompt.rs
  - crates/desktop/src/native.rs
verify:
  - cargo test -p zest-core --lib -- worktree_chat::tests
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml -- workspace::projects::
---

# Worktree chats

## Behavior

- **Workbench > Worktrees** offers **New chat in worktree** (it reads "Creating checkout…" while it runs). It requires a Git repository root with at least one commit.
- Each new worktree gets a unique `zest/chat-*` branch and persistent checkout, and opens in its own project window with a new chat. Its files, chat history, approvals, and running turns belong to that checkout. Claude Code, Codex/GPT, and Zest-driven API providers such as DeepSeek use the same isolated working directory.
- The checkout starts at committed HEAD. Uncommitted and untracked files in the original project are not copied. Other worktree chats and the original project keep their own edits.
- A worktree window looks like any project window; its header shows the checkout's branch, and its sidebar lists only that checkout's chats. In a worktree window, the Worktrees tab also offers **Open source project**. Worktrees survive app restarts. The source project's provider configuration is used for each worktree; editing that configuration changes the source project.
- The Worktrees tab lists every visible worktree record, from any project, by branch. A row adds its Git state as " · 2 ahead, 1 behind, 3 uncommitted": commits ahead of and behind the source project's current branch, and the number of uncommitted entries. The entries are what `git status --porcelain` lists, so tracked changes and untracked files count, and a new untracked folder counts once. A level, clean checkout shows no counts. Clicking a row opens that worktree's window, or brings it to the front if it is open.
- The counts are read when the tab opens and on **Refresh**. There is no timer. If the source project is on a detached HEAD, ahead and behind compare with the commit the worktree started from; if that is unknown (a worktree registered before it was recorded) they read as zero. If Git cannot read the checkout, for example because the source project was deleted, the row shows no counts. None of this raises an error.
- **Hide from list** marks the record hidden. It does not delete the worktree's files or Git branch, and hidden worktrees also drop out of the Pull requests list. Use Git to inspect and remove a worktree deliberately after its work is integrated.

## How it works

`Workspace::create_worktree` (`crates/desktop/src/workspace/projects.rs`) calls `zest_core::worktree_registry::create` on a background thread with the window's registry directory (`<config dir>/zest`, or `.zest/demo-desktop` in the demo). `create` asks `zest_core::worktree_chat` to create a branch and linked checkout under `<registry>/worktrees`, and writes one record per checkout under `worktree-chat-records/` before returning. The previous shared `worktree-chats.json` array remains readable for migration; a present per-checkout sidecar takes precedence, and even a damaged sidecar suppresses its legacy entry so removed worktrees cannot reappear. Independent records prevent concurrent app windows from overwriting each other's mappings. Records are canonicalized and checked against the managed storage root, rejecting traversal and symlink escapes; other invalid records are skipped.

`visit_project` then opens the checkout with `Project::open_with_registry` (`crates/desktop/src/native.rs`), which resolves `config_root` through `worktree_registry::config_root`. An unregistered managed worktree fails closed ("This Zest worktree has no provider mapping") rather than silently using another provider config, and a missing source project is an error. `open_project_window` reuses an existing window for the same path. Provider sessions, file tools, history, and review use the project's own path, so they all operate in the checkout. `prompt::git_head_contents` reads both normal `.git/HEAD` and the linked-worktree pointer.

Each record also keeps the `base_commit` the checkout started from (optional, so older records still load). `load_worktrees` runs `zest_core::worktree_chat::chat_worktree_status` for each visible record on a background thread, and `status_label` formats the counts. The core function compares `git rev-list --left-right --count <source branch tip>...HEAD` run in the checkout, using the tip of the branch the source project is on, or the recorded base commit when it has none. Its `git status --porcelain --untracked-files=normal` gives the dirty count. Both run with `GIT_OPTIONAL_LOCKS=0`, so a refresh cannot take the index lock from the agent's own Git commands. **Hide from list** calls `worktree_registry::set_visibility(…, false)`.

## Verify

`worktree_chat::tests` cover creation, isolation and the status counts in disposable repositories; `workspace::projects::` checks that a clean or unreadable status prints nothing. By hand, in a disposable Git repository, open it with `npm run gpui -- --project <path>`, start two worktree chats from Workbench > Worktrees and edit the same tracked file differently. Confirm the source file stays unchanged and reopening either worktree returns to its own branch.

To check the status counts, commit once in one worktree, add an untracked file there, and commit once in the source project. After **Refresh** that row reads 1 ahead, 1 behind and 1 uncommitted, while a clean sibling reads 0 ahead, 1 behind, 0 uncommitted.

## Pitfalls

- Worktrees are persistent user data. Never use delegation's temporary worktree cleanup or force-remove a chat checkout.
- Provider configuration belongs to the original project even though execution occurs in the worktree. A missing original project is an error rather than silently choosing another provider table.
- A damaged legacy registry or one damaged per-checkout record must not block ordinary project chats. A checkout whose own mapping cannot be recovered cannot start a provider session.
- The status check must stay read-only: no `git status` that writes the index. Run it only for registered worktree records, never for an arbitrary path.
- A plain new chat still uses the open project folder. Isolation is opt-in.
