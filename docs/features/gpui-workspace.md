---
title: Native GPUI workspace
summary: Open a project, run Zest agent turns, and review saved conversations and project files in a Windows GPUI window.
paths:
  - spikes/gpui/Cargo.toml
  - spikes/gpui/src/main.rs
  - spikes/gpui/src/workspace.rs
  - spikes/gpui/src/workspace/board.rs
  - .github/workflows/gpui-windows.yml
  - spikes/gpui/src/native.rs
  - spikes/gpui/src/native_view.rs
  - spikes/gpui/src/search.rs
  - spikes/gpui/src/side.rs
  - spikes/gpui/src/interaction.rs
  - spikes/gpui/src/bridge.rs
  - spikes/gpui/src/composer.rs
  - spikes/gpui/src/md.rs
  - spikes/gpui/src/view.rs
  - spikes/gpui/src/theme.rs
  - spikes/gpui/src/ui.rs
  - spikes/gpui/src/icons_generated.rs
  - spikes/gpui/src/workspace/chrome.rs
  - spikes/gpui/assets/**
  - spikes/gpui/benchmark/**
  - spikes/gpui/examples/workbench_io.rs
  - crates/desktop/src/attachments.rs
  - crates/desktop/src/context_meter.rs
  - crates/desktop/src/workspace_files.rs
tests:
  - spikes/gpui/src/workspace/board/selftest.rs
  - spikes/gpui/src/workspace_test.rs
  - spikes/gpui/src/selftest.rs
  - spikes/gpui/src/selftest_trace.rs
verify:
  - npm run gpui:test
  - cargo test --locked --release --manifest-path spikes/gpui/Cargo.toml
---

# Native GPUI workspace

From this checkout, run `npm run gpui -- --project "D:\Code\MyProject"` to open a project. With no arguments, the native window opens a folder picker. It reads Zest's provider configuration, project chat store and usage ledger. This is a source build on Windows and requires the Rust toolchain, MSVC and the Windows SDK shader compiler described in [the rendering spike](../../spikes/gpui/FINDINGS.md).

Run `npm run gpui -- --demo` for a temporary offline workspace. Send `Demo: write` to review an actual file-write request or `Demo: question` to answer a choice. Files and chats live in the temporary project for the duration of the demo.

## Native workflows

| Area | Available in the native window |
| --- | --- |
| Workspace and history | Open a project, start or rename chats, pin a chat, split the workspace and switch between independent sessions. Ctrl+K searches chat titles and saved message text. A transcript result opens the matching message. |
| Agent conversation | Send, stream, stop, and reopen a conversation with saved provider context. Markdown, code, thinking, tool results, and provider activity render in the transcript. Follow-ups queue in order and can be edited or removed. After stop, error or reopen, use **Resume queued** to send the oldest pending input. |
| Files and interaction | Attach files or paste images, inspect proposed tool inputs, approve or deny individual requests and answer agent questions. The agent uses Zest's shared tools and approval policy. |
| Provider and commands | Choose a configured provider, model, effort and permission mode. The settings panel exposes supported sign-in flows. Use skills, MCP slash commands, plan mode and the independent `/btw` conversation. |
| Context and history | Fork a thread or checkpoint, inspect context usage, and compact agent history. The usage panel reads the shared ledger and can request provider quota. |
| Project inspection | Browse project files, preview bounded text content, read the Git diff, and inspect the current chat's background jobs and the project's saved delegations. Each new chat, including a split, records the project's current commit. Changes shows edits since that commit even after committing and reopening the chat. A legacy chat without a saved starting commit shows working tree changes. |
| Delegation | Create a scoped card, select worker and reviewer targets, approve dispatch, review acceptance evidence, page through the diff, then apply, cancel or prepare a retry. Changes appear in the board while the worker and reviewer run. |

## Implementation and verification

`workspace.rs` owns navigation. Each open chat owns a `Root` view and its own command channel. `native.rs` sends work to the shared Rust agent on Tokio, checkpoints before turns and writes the transcript, provider history and run status. Each approval or question has an individual response channel. `side.rs` keeps side questions out of the main transcript.

The sidebar reads saved summaries in the background at startup and when opening Find chats. Turn events update the affected chat directly, and pinned chats appear first. Find chats uses the [shared local transcript index](threads-and-history.md), including for older histories over 8 MiB. A transcript result opens the answer's rendered row below that turn's reasoning and tool trace.

`npm run gpui:test` drives a native window, the real agent loop, the thread store and an offline provider in a temporary Git repository. Windows CI runs the same scenario in a GPUI test window through `cargo test`, including an approved edit that is committed and reviewed after reopening, then the delegation board's worker, reviewer and apply sequence. The original rendering fixtures remain available with `--selftest-queue` and `--agent --history=500`.

### Native desktop port

The native window uses the React UI's default "zest" theme. `spikes/gpui/src/theme.rs` holds its color tokens from `crates/desktop/ui/src/index.css`, embeds the Geist font, and serves Lucide icons generated from the UI's own `lucide-react` package (`node spikes/gpui/assets/generate-icons.mjs` adds icons and regenerates `icons_generated.rs`). `ui.rs` has the shared controls. `workspace/chrome.rs` draws the sidebar (back and forward, New chat, Search, Pull requests, Customize, the project's chats and the account row), the chat header and the page frame. Settings-type panels are tabs under Customize, and project tools (Changes, Files, Jobs, Checkpoints, Delegation, Worktrees) are tabs under Workbench, opened from the header. Chats sit in a rounded canvas with a centered 720 px transcript and the composer card, toolbar and status line below it. Finished replies have Copy and Download (save as Markdown). With changed lines since the chat's start commit, a strip above the composer shows the project, branch, +/- counts from `zest_core::workspace_changes::diff_stats` and any linked pull request; it hides during a turn, and dismissing it lasts until the counts change.

Customize > Appearance offers the React UI's Zest, Nights and Oceanic themes and its fonts: Geist and System UI always, Inter, Plus Jakarta Sans, JetBrains Mono and Fira Code when installed. A choice applies at once and is saved in `<config dir>/zest/gpui-appearance.json`. Code colors follow the theme's brightness for newly rendered messages; chats already on screen keep theirs until reopened.


The default source launcher, `npm run dev`, opens GPUI. Split right and Split down add independently running chats. A divider supports dragging, arrow keys, Home, End and double-click to reset. Single view keeps the split group in the sidebar so its chats and drafts can be reopened during the session.

Worktrees, Pull requests and Add-ons have native sidebar entries. Browser actions use a private system webview for each chat. HTML cards expose source and export, and on Windows they can open an isolated interactive document. Run `node spikes/gpui/benchmark/web-windows.mjs PATH_TO_BINARY` to drive the real web windows against a local fixture.

Buttons have accessible names. The composer exposes its text as AccessKit text runs with the caret and selection, so Windows UI Automation offers TextPattern as well as ValuePattern. Windows CI launches the demo and runs `spikes/gpui/benchmark/accessibility.ps1 -Check` to fail on an unnamed button or an unreadable composer.

`npm run gpui:package` builds local x64 installers (MSI and NSIS on Windows, DEB, RPM and AppImage on Linux). The Linux AppImage uses the host GTK 3, WebKitGTK 4.1 and Vulkan runtime. The release workflow still ships Tauri.

## Remaining desktop coverage

The [Windows desktop comparison](../../spikes/gpui/benchmark/README.md) measures both frontends on matched offline transcripts and records the status of each desktop workflow.
