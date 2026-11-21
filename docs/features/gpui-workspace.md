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

From this checkout, run `npm run gpui -- --project "D:\Code\MyProject"` to open a project. With no arguments, the native window opens the current directory. It reads Zest's provider configuration, project chat store and usage ledger. This is a source build on Windows and requires the Rust toolchain, MSVC and the Windows SDK shader compiler described in [the rendering spike](../../spikes/gpui/FINDINGS.md).

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

## Remaining desktop coverage

The [Windows desktop comparison](../../spikes/gpui/benchmark/README.md) measures both frontends on matched offline transcripts and records the status of each desktop workflow.

The integrated agent browser, interactive HTML artifacts, advanced Git/worktree and pull-request actions, plugin management, native installers, and full accessibility and cross-platform verification remain open. The existing desktop launcher still runs Tauri.
