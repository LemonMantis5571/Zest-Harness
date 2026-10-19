---
title: Worktree chats
summary: Start parallel coding chats in separate Git branches and checkouts, using any configured provider.
paths:
  - crates/core/src/worktree_chat.rs
  - crates/core/src/prompt.rs
  - crates/desktop/src/lib.rs
  - crates/desktop/ui/src/components/ChatHistorySidebar.tsx
  - crates/desktop/ui/src/lib/api.ts
  - crates/desktop/ui/src/lib/backend.ts
  - crates/desktop/ui/src/lib/fixtureBackend.ts
tests:
  - crates/desktop/ui/e2e/worktree-chats.spec.ts
verify:
  - cargo test -p zest-core --lib -- worktree_chat::tests
  - npm run ui:e2e -- worktree-chats
---

# Worktree chats

## Behavior

- A project's menu offers **New chat in worktree**. It requires a Git repository root with at least one commit.
- Each new chat gets a unique `zest/chat-*` branch and persistent checkout. Its files, chat history, approvals, and running turns belong to that checkout. Claude Code, Codex/GPT, and Zest-driven API providers such as DeepSeek use the same isolated working directory.
- The checkout starts at committed HEAD. Uncommitted and untracked files in the original project are not copied. Other worktree chats and the original project keep their own edits.
- Worktree chats appear as separate, named workspaces in the sidebar. They survive app restarts. The source project's provider configuration is used for each worktree; editing that configuration changes the source project.
- Deleting a chat or removing a worktree from Zest's sidebar does not delete its files or Git branch. Use Git to inspect and remove a worktree deliberately after its work is integrated.

## How it works

`ChatHistorySidebar` calls `create_worktree_chat` through the backend. The desktop checks that the source is a known workspace, asks `zest_core::worktree_chat` to create a branch and linked checkout under the user-local Zest config directory, and records the source project in `worktree-chats.json`. It then opens a normal new project chat with the worktree as its root. Provider sessions, file tools, history, and review already use the active project's root, so they all operate in the checkout. `prompt::git_head_contents` reads both normal `.git/HEAD` and the linked-worktree pointer.

## Verify

Run the focused Rust and fixture-browser tests above. In a disposable Git repository, start two worktree chats and edit the same tracked file differently. Confirm the source file stays unchanged and reopening either chat returns to its own branch.

## Pitfalls

- Worktrees are persistent user data. Never use delegation's temporary worktree cleanup or force-remove a chat checkout.
- Provider configuration belongs to the original project even though execution occurs in the worktree. A missing original project is an error rather than silently choosing another provider table.
- A plain new project chat still uses the selected project folder. Isolation is opt-in.
