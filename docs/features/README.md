# Feature map

One page per feature: what it is supposed to do, where the code lives, and how
to check it still works. Start here when you need to turn a vague bug report
("the model picker forgot my choice") into the files and tests to look at.

The map is checked in CI. `npm run features:check` fails when an entry names a
file that no longer exists, when a source file under `crates/` or `scripts/`
belongs to no feature, or when the index below is out of date. The pre-commit
hook runs the same check and also lists features whose code you changed without
touching their doc.

## Look things up

```bash
npm run features -- where crates/core/src/tools/bash.rs   # which feature owns a file
npm run features -- show delegation                       # a feature's files, tests, checks
npm run features -- list                                  # every feature
npm run features -- where crates/desktop/src/ --json      # machine-readable
```

## Features

<!-- feature-index:start -->
| Feature | What it does | Checks |
| --- | --- | --- |
| [Advisor](advisor.md) | The chat model can consult a stronger model mid-turn with consult_advisor, on any provider Zest drives, behind an approval when the advisor is on a second provider. | 6 commands |
| [App shell and desktop plumbing](app-shell.md) | How the desktop process starts, which command-line modes it has, how it is stamped and packaged, and the core plumbing every feature shares. | 1 test file, 4 commands |
| [Attachments](attachments.md) | Attach files, PDFs, and images to a message from the file picker or by pasting, within per-image and per-message limits. | 2 test files, 2 commands |
| [Local browser and web search](browser-and-web.md) | The agent can drive a Zest browser window (open, read, click, type) behind approvals, and search the public web through DuckDuckGo without an API key. | 2 commands |
| [BTW side questions](btw-side-questions.md) | Ask a quick question about the current conversation in a side panel with /btw, without adding it to the main chat or interrupting a running task. | 1 test file, 3 commands |
| [Chat turns and streaming](chat-turns-and-streaming.md) | Send a message, watch the answer and reasoning stream in, stop it, and queue follow-ups while a turn is still running. | 3 test files, 10 commands |
| [CLI agent providers](cli-agent-providers.md) | Use Claude Code, the Codex CLI, or Cursor as the main chat provider, with their own sign-in and agent loop and Zest's approval cards. | 10 commands |
| [Command palette and keyboard shortcuts](command-palette-and-shortcuts.md) | Ctrl+K opens a command palette over actions, pages and chats; the Search page finds saved message text; fixed workspace and composer shortcuts; Escape to leave a panel or stop a turn. | 1 test file, 3 commands |
| [Context budget and compaction](context-budget.md) | The composer shows how full the model's context window is, and Zest trims or summarizes the conversation automatically when it passes 80%. | 7 commands |
| [Delegation coordinator](delegation-coordinator.md) | The shared queue that owns a project's feature-card jobs: one coordinator per project, two jobs at a time, idempotent approve/cancel/apply, and recovery after a restart. | 1 test file, 2 commands |
| [Delegation (feature cards)](delegation.md) | Hand a scoped task to a separate native or external worker, have a fresh reviewer check it in its own worktree, then apply the diff yourself. | 2 test files, 9 commands |
| [Verification gates and release tooling](developer-verification-and-release.md) | The local and CI gates (npm run verify, release-verify), the output-artifact and feature-map checks, git hooks, and the desktop packaging and release scripts. | 2 test files, 4 commands |
| [Desktop app (GPUI)](gpui-workspace.md) | The native Zest window for Windows and Linux in crates/desktop. Open a project, run agent turns, review changes and delegations, and build installers. | 4 test files, 2 commands |
| [Interactive HTML artifacts](html-artifacts.md) | Agents publish self-contained interactive HTML cards in chat, with isolated execution, source/export controls, and optional real screenshot feedback. | 1 test file, 4 commands |
| [MCP servers](mcp-servers.md) | Users add, check, switch and remove outbound MCP servers (local command or Streamable HTTP URL) in Customize > MCP servers or zest.toml, and the parent chat calls their tools behind the approval card. | 3 commands |
| [Message rendering](message-rendering.md) | Assistant answers render as Markdown with highlighted code, tables, selectable text, safe links, and copy/save actions, even while they stream. | 2 test files, 9 commands |
| [Model and effort selection](model-and-effort-selection.md) | Pick a provider, model and reasoning effort for a chat in Customize > Models, and have the choice remembered per provider in each project. | 1 test file, 4 commands |
| [Notifications and alerts](notifications.md) | Zest pings only when something needs the user and they are not looking at it, as an Open toast when focused or an OS notification when away, and shows workspace notices as toasts that errors keep until dismissed. | 1 test file, 2 commands |
| [Offline demo mode](offline-fixture-mode.md) | Run the desktop app with no provider or credentials in a temporary project, with a built-in offline provider that can stream a reply, request a file write, or ask a question. | 2 test files, 2 commands |
| [Plugins (Now Playing, Wallpaper)](plugins.md) | Optional out-of-process add-ons, off by default, that show and control music or put an image behind the app. | 4 commands |
| [Project file tools](project-file-tools.md) | The agent reads, searches, lists, creates, and edits files only inside the open project, with likely secrets hidden and oversized results kept retrievable. | 9 commands |
| [Provider runtime and native API providers](provider-runtime.md) | One provider-neutral interface for every backend, with Zest's own streaming clients for the Anthropic Messages API, OpenAI-compatible endpoints, and ChatGPT Codex. | 1 test file, 11 commands |
| [Providers and sign-in](providers-and-sign-in.md) | Add providers (a coding CLI, ChatGPT sign-in, or an API key) and sign in from Customize > Providers, choose the one new chats use, and move a chat to another provider. | 5 commands |
| [Linked pull requests](pull-requests.md) | Zest detects the GitHub pull request for a chat's branch, lists linked pull requests across projects, and shows their checks and diff. | 3 commands |
| [Questions and planning](questions-and-planning.md) | The agent can pause a turn to ask one structured question, and Plan mode runs the plan skill without writes. | 1 test file, 3 commands |
| [Settings, appearance, and profile](settings-and-appearance.md) | The zest.toml config and how Zest finds and edits it, the desktop app's Customize pages, themes, fonts and chat view, and the profile screen with name and photo. | 1 test file, 5 commands |
| [Shell commands and background jobs](shell-commands-and-jobs.md) | The agent runs commands with an explicit working directory, read-only ones unattended and everything else after approval, and can keep dev servers running as background jobs it can read and stop. | 1 test file, 4 commands |
| [Skills and slash commands](skills-and-slash-commands.md) | Personal SKILL.md files become /commands and model-readable instructions; built-ins, skills and enabled MCP servers are listed in Customize > Skills & MCP. | 1 test file, 4 commands |
| [Split workspace](split-workspace.md) | Open several chats side by side in resizable, nested panes, each with its own draft, model, and running turn. | 1 test file, 3 commands |
| [Terminal CLI (zest)](terminal-cli.md) | The zest binary: an interactive terminal chat with y/N approvals, auth and usage reports, a live doctor check, and zest run --jsonl for one machine-readable, deny-only agent turn. | 1 test file, 5 commands |
| [Threads, history, and recovery](threads-and-history.md) | Chats are saved per project, listed in the sidebar, searched, forked from checkpoints, and closed safely after an interrupted run. | 1 test file, 9 commands |
| [Tool approvals and permission modes](tool-approvals.md) | Writes, commands, and other risky tool calls pass one approval gate whose behavior follows the chat's permission mode (Manual, Accept edits, Plan, Auto, Bypass). | 1 test file, 4 commands |
| [Desktop control CLI (gpui-control)](ui-control-cli.md) | One-line commands that launch the desktop app, drive it through UI Automation, and report its state and errors as JSON, so agents verify changes on Windows without writing throwaway scripts. | 4 commands |
| [Usage, cost and provider quota](usage-and-quota.md) | See tokens, estimated cost and cache reuse across Zest and your coding CLIs, and check live limits or balance reported by each provider. | 1 test file, 8 commands |
| [Workbench panel](workbench-panel.md) | A project page with Changes, Files, Jobs, Checkpoints, Delegation, and Worktrees tabs, including checkpoint forks and rewind and a read-only file browser. | 1 test file, 3 commands |
| [Branch changes and diff review](workspace-changes-and-review.md) | A project chat shows what changed on its branch since the chat started, and Workbench > Changes shows the full diff with secret files redacted. | 1 test file, 4 commands |
| [Worktree chats](worktree-chats.md) | Start parallel coding chats in separate Git branches and checkouts, using any configured provider. | 2 commands |
| [zest serve (headless coordinator)](zest-serve.md) | A windowless daemon that owns one project's delegation queue and exposes delegation_* tools over authenticated MCP on loopback, so a bot or script can create, approve and apply feature cards. | 1 test file, 4 commands |
<!-- feature-index:end -->

## Writing an entry

File name is the slug (`docs/features/<slug>.md`). Frontmatter is a small YAML
subset: scalar `title` and `summary`, and `paths`, `tests`, `verify` as lists of
`  - item` lines. A path is an exact file, a directory ending in `/`, or a glob
(`*` within one segment, `**` across segments). No inline comments.

```markdown
---
title: Model and effort selection
summary: One sentence, user's point of view, shown in the index.
paths:
  - crates/desktop/src/workspace/chrome.rs
  - crates/core/src/provider/
  - crates/core/src/model*.rs
tests:
  - crates/core/tests/provider_e2e.rs
verify:
  - cargo test -p zest-core --lib -- provider::
---

# Model and effort selection

## Behavior
What a user sees and what must stay true. Write these as checkable statements;
they are what an agent compares a bug report against.

## How it works
The path through the code, entry point first: desktop view or CLI command,
then the core function. Name the functions and files.

## Verify
How to prove it works: the tests above, steps in the offline demo
(`npm run gpui -- --demo`), or a CLI run.

## Pitfalls
Non-obvious constraints, past regressions, things that look wrong but are not.
```

A file may belong to several features. Put a file under `tests` when it only
tests the feature. Keep the prose short and factual; update it in the same
change as the behavior. After adding or renaming an entry, run
`npm run features -- index` to refresh the table above.
