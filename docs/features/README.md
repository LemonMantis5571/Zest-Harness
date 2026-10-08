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
| [App shell and desktop plumbing](app-shell.md) | The desktop process, its Tauri command surface, window chrome, and the React root that boots into the picker or a chat. | 4 test files, 5 commands |
| [Attachments](attachments.md) | Attach files, PDFs, and images to a message from the file picker or by pasting, within per-image and per-message limits. | 2 test files, 2 commands |
| [Local browser and web search](browser-and-web.md) | The agent can drive a Zest browser window (open, read, click, type) behind approvals, and search the public web through DuckDuckGo without an API key. | 3 commands |
| [BTW side questions](btw-side-questions.md) | Ask a quick question about the current conversation in a side panel with /btw, without adding it to the main chat or interrupting a running task. | 1 test file, 3 commands |
| [Chat turns and streaming](chat-turns-and-streaming.md) | Send a message, watch the answer and reasoning stream in, stop it, and queue follow-ups while a turn is still running. | 7 test files, 10 commands |
| [CLI agent providers](cli-agent-providers.md) | Use Claude Code, the Codex CLI, or Cursor as the main chat provider, with their own sign-in and agent loop and Zest's approval cards. | 8 commands |
| [Command palette and keyboard shortcuts](command-palette-and-shortcuts.md) | Mod+K search over chats, actions, commands, and settings; rebindable shortcuts; Escape closing whatever is on top. | 5 test files, 3 commands |
| [Context budget and compaction](context-budget.md) | The composer shows how full the model's context window is, and Zest trims or summarizes the conversation automatically when it passes 80%. | 2 test files, 6 commands |
| [Delegation coordinator](delegation-coordinator.md) | The shared queue that owns a project's feature-card jobs: one coordinator per project, two jobs at a time, idempotent approve/cancel/apply, and recovery after a restart. | 1 test file, 2 commands |
| [Delegation (feature cards)](delegation.md) | Hand a scoped task to a separate native or external worker, have a fresh reviewer check it in its own worktree, then apply the diff yourself. | 2 test files, 9 commands |
| [Verification gates and release tooling](developer-verification-and-release.md) | The local and CI gates (npm run verify, release-verify), the output-artifact and feature-map checks, git hooks, UI lint rules, and the release packaging scripts. | 4 test files, 5 commands |
| [Interactive HTML artifacts](html-artifacts.md) | Agents publish self-contained interactive HTML cards in chat, with isolated execution, source/export controls, and optional real screenshot feedback. | 2 test files, 5 commands |
| [MCP servers](mcp-servers.md) | Users configure outbound MCP servers (local command or Streamable HTTP URL) whose tools the parent chat can call, always behind the approval card. | 2 test files, 2 commands |
| [Message rendering](message-rendering.md) | Assistant answers render as Markdown with highlighted code, Mermaid diagrams, zoomable images, safe links, and copy/save actions, even while they stream. | 14 test files, 4 commands |
| [Model and effort selection](model-and-effort-selection.md) | Pick a model and reasoning effort from the composer, search across providers, and have the choice remembered per provider in each project. | 7 test files, 6 commands |
| [Notifications and toasts](notifications.md) | In-app toasts when Zest is focused, OS notifications when it is not, grouped duplicates, and an explicit policy for background failures. | 1 test file, 1 command |
| [Offline fixture mode](offline-fixture-mode.md) | Run the desktop UI in an ordinary browser with an in-memory fake backend and a canned turn; every Playwright spec runs against it. | 2 test files, 2 commands |
| [Plugins (Now Playing, Wallpaper)](plugins.md) | Optional out-of-process add-ons, off by default, that show and control music or put an image behind the app. | 3 test files, 5 commands |
| [Project file tools](project-file-tools.md) | The agent reads, searches, lists, creates, and edits files only inside the open project, with likely secrets hidden and oversized results kept retrievable. | 2 test files, 8 commands |
| [Provider runtime and native API providers](provider-runtime.md) | One provider-neutral interface for every backend, with Zest's own streaming clients for the Anthropic Messages API, OpenAI-compatible endpoints, and ChatGPT Codex. | 1 test file, 10 commands |
| [Providers and sign-in](providers-and-sign-in.md) | Choose a provider on launch, connect it through a coding CLI sign-in, ChatGPT, or your own API key, and switch providers from inside a chat. | 6 test files, 6 commands |
| [Linked pull requests](pull-requests.md) | Zest detects the GitHub pull request for a chat's branch, lists linked pull requests across projects, and opens their diff in the review pane. | 2 test files, 4 commands |
| [Questions and planning](questions-and-planning.md) | The agent can pause a turn to ask one structured question, Plan mode runs the plan skill without writes, and a finished plan offers a Build button. | 3 test files, 3 commands |
| [Settings, appearance, and profile](settings-and-appearance.md) | The zest.toml config and how Zest edits it, the Settings and Customize panels, themes, fonts, and the profile screen with its avatar. | 3 test files, 3 commands |
| [Shell commands and background jobs](shell-commands-and-jobs.md) | The agent runs commands with an explicit working directory, read-only ones unattended and everything else after approval, and can keep dev servers running as background jobs it can read and stop. | 1 test file, 4 commands |
| [Skills and slash commands](skills-and-slash-commands.md) | Personal SKILL.md files become /commands and model-readable instructions; enabled MCP servers and built-ins share the same / list in the composer. | 2 test files, 5 commands |
| [Split workspace](split-workspace.md) | Open several chats side by side in resizable, nested panes, each with its own draft, model, and running turn. | 3 test files, 2 commands |
| [Terminal CLI (zest)](terminal-cli.md) | The zest binary: an interactive terminal chat with y/N approvals, auth and usage reports, a live doctor check, and zest run --jsonl for one machine-readable, deny-only agent turn. | 1 test file, 5 commands |
| [Threads, history, and recovery](threads-and-history.md) | Chats are saved per project, listed in the sidebar, reopened a page at a time, rewound or forked from checkpoints, and recovered safely after an interrupted run. | 6 test files, 10 commands |
| [Tool approvals and permission modes](tool-approvals.md) | Writes, commands, and other risky tool calls pass one approval gate whose behavior follows the mode picked in the composer (Manual, Accept edits, Plan, Auto, Bypass). | 1 test file, 3 commands |
| [UI control CLI (zest-control)](ui-control-cli.md) | One-line commands that drive the live UI, inspect it, and report every error, so agents debug and verify without writing throwaway browser scripts. | 1 test file, 2 commands |
| [Usage, cost and provider quota](usage-and-quota.md) | See tokens, estimated cost and cache reuse across Zest and your coding CLIs, and check live limits or balance reported by each provider. | 5 test files, 8 commands |
| [Workbench panel](workbench-panel.md) | A side panel for a project chat with Activity, Outline, Delegation, and Files tabs, including a quick workspace check, checkpoint rewind, and a read-only file browser. | 1 test file, 3 commands |
| [Branch changes and diff review](workspace-changes-and-review.md) | A project chat shows what changed on its branch since the chat started and opens a review pane with a cleaned-up or raw diff, with secret files redacted. | 4 test files, 4 commands |
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
  - crates/desktop/ui/src/components/ModelEffortPicker.tsx
  - crates/core/src/provider/
  - crates/desktop/ui/src/lib/model*.ts
tests:
  - crates/desktop/ui/e2e/model-picker.spec.ts
verify:
  - npm run test:e2e -w ui -- model-picker
---

# Model and effort selection

## Behavior
What a user sees and what must stay true. Write these as checkable statements;
they are what an agent compares a bug report against.

## How it works
The path through the code, entry point first: UI component, Tauri command,
core function. Name the functions and files.

## Verify
How to prove it works: the tests above, fixture-mode steps, or a CLI run.

## Pitfalls
Non-obvious constraints, past regressions, things that look wrong but are not.
```

A file may belong to several features. Put a file under `tests` when it only
tests the feature. Keep the prose short and factual; update it in the same
change as the behavior. After adding or renaming an entry, run
`npm run features -- index` to refresh the table above.
