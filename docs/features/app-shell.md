---
title: App shell and desktop plumbing
summary: How the desktop process starts, which command-line modes it has, how it is stamped and packaged, and the core plumbing every feature shares.
paths:
  - crates/desktop/src/main.rs
  - crates/desktop/src/workspace.rs
  - crates/desktop/src/web_window.rs
  - crates/desktop/src/scratch_dir.rs
  - crates/desktop/build.rs
  - crates/desktop/zest.rc
  - crates/core/src/lib.rs
  - crates/core/src/error.rs
  - crates/core/src/fsutil.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- error::
  - cargo test -p zest-core --lib -- fsutil
  - cargo test --locked --release --manifest-path crates/desktop/Cargo.toml
  - npm run gpui -- --help
---

# App shell and desktop plumbing

## Behavior
- `zest-desktop` opens one window per project. With `--project DIR` it opens that folder; with no arguments it shows a folder picker. Installers start it with `--launcher`, which is accepted and ignored.
- `--demo` opens a temporary offline project with a built-in provider (see [offline demo mode](offline-fixture-mode.md)). `--selftest-workspace` runs the same project through the scripted workspace check that `npm run gpui:test` uses.
- Outside the demo, startup loads `.env`, `~/.zest/.env` and `~/.zest/env`, then creates `~/.zest/zest.toml` from the embedded starter config if it is missing. An existing config is never overwritten; a failure is printed and the window still opens.
- `--help` lists the workspace options. The rendering-spike modes (`--selftest`, `--selftest-queue`, `--bench`, `--scroll`, `--langs`, `--agent` and friends) still run the original stream fixtures and cannot be combined with `--project` or `--demo`.
- Notification clicks reopen the chat they belong to and bring its window forward.
- Keyboard shortcuts bound at startup: Ctrl+N new chat, Ctrl+K find chats, Ctrl+O open folder, Ctrl+\ toggle split, Ctrl+Shift+Enter zoom pane, Ctrl+Shift+H minimize pane, Escape close panel.
- On Windows the executable carries Zest's icon and the release version, which GPUI also uses for its window and taskbar icon.

## How it works
1. `main.rs` first calls `web_window::dispatch()`. When the first argument names a browser or HTML-artifact helper mode, the same executable runs as an isolated web window process and exits. Otherwise `workspace::launch_if_requested()` parses the workspace options and returns `true` once the app has run. Only spike modes fall through to the old fixture code in `main.rs`.
2. `launch_if_requested` creates the temporary demo directory, loads the environment and user config, then starts the GPUI application from `theme::application()` with the fonts loaded, the app identity `dev.zest.app`, the notification handler and the key bindings, and opens the workspace window.
3. `build.rs` reads the version from the root `Cargo.toml` (`[workspace.package]`) and, on Windows, compiles `zest.rc` with `embed-resource` so the icon (`crates/desktop/icons/icon.ico`) and version resource are linked in.
4. Core plumbing shared by every feature: `crates/core/src/lib.rs` re-exports the headless API, `error.rs` defines `HarnessError` and its classifiers (`is_transient`, `is_auth_problem`, `is_context_limit`, `is_unreachable`, `provider_user_message`), and `fsutil.rs` provides `atomic_write` for all durable state.

## Verify
- `cargo test -p zest-core --lib -- error::` covers error classification, including that retries (`Exhausted`) keep the original classification.
- `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml` runs the desktop unit tests and the GPUI workspace scenario.
- `npm run gpui -- --help` prints the workspace options without opening a window.

## Pitfalls
- The desktop crate is a separate Cargo workspace. `cargo build` at the repository root does not build it; use `--manifest-path crates/desktop/Cargo.toml` or the `npm run gpui*` scripts.
- `build.rs` finds the version two directories up (`../../Cargo.toml`). Moving the crate means updating that path.
- Provider failures reach the transcript through `HarnessError::turn_message`, so raw HTTP bodies and internal stream diagnostics stay out of chat bubbles. `provider_user_message` only trusts `Stream` errors whose kind starts with `provider:`.
- `scratch_dir.rs` is a `#[cfg(test)]` helper (`ScratchDir`, a self-deleting temp dir), not runtime code.
