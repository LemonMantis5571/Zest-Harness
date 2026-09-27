---
title: App shell and desktop plumbing
summary: The desktop process, its Tauri command surface, window chrome, and the React root that boots into the picker or a chat.
paths:
  - crates/desktop/src/main.rs
  - crates/desktop/src/lib.rs
  - crates/desktop/build.rs
  - crates/desktop/src/window_chrome.rs
  - crates/desktop/src/scratch_dir.rs
  - crates/desktop/tauri.conf.json
  - crates/desktop/tauri.dev.conf.json
  - crates/desktop/capabilities/default.json
  - crates/core/src/lib.rs
  - crates/core/src/error.rs
  - crates/core/src/fsutil.rs
  - .cargo/config.toml
  - crates/desktop/ui/index.html
  - crates/desktop/ui/src/main.tsx
  - crates/desktop/ui/src/App.tsx
  - crates/desktop/ui/src/components/ErrorBoundary.tsx
  - crates/desktop/ui/src/components/TopbarPanel.tsx
  - crates/desktop/ui/src/components/BrandMark.tsx
  - crates/desktop/ui/src/components/ui/button.tsx
  - crates/desktop/ui/src/components/ui/collapsible.tsx
  - crates/desktop/ui/src/components/ui/icon-swap.tsx
  - crates/desktop/ui/src/components/ui/input.tsx
  - crates/desktop/ui/src/components/ui/marker.tsx
  - crates/desktop/ui/src/components/ui/skeleton.tsx
  - crates/desktop/ui/src/components/ui/slider.tsx
  - crates/desktop/ui/src/components/ui/spinner.tsx
  - crates/desktop/ui/src/components/ConfirmDialog.tsx
  - crates/desktop/ui/src/lib/backend.ts
  - crates/desktop/ui/src/lib/api.ts
  - crates/desktop/ui/src/lib/types.ts
  - crates/desktop/ui/src/lib/windowChrome.ts
  - crates/desktop/ui/src/lib/startupPerf.ts
  - crates/desktop/ui/src/lib/navigationHistory.ts
  - crates/desktop/ui/src/lib/utils.ts
  - crates/desktop/ui/src/lib/json.ts
  - crates/desktop/ui/src/lib/visibleInterval.ts
  - crates/desktop/ui/src/lib/useMediaQuery.ts
  - crates/desktop/ui/src/lib/reveal.ts
  - crates/desktop/ui/src/lib/generated/
tests:
  - crates/desktop/ui/src/lib/json.test.ts
  - crates/desktop/ui/src/lib/visibleInterval.test.ts
  - crates/desktop/ui/src/lib/reveal.test.ts
  - crates/desktop/ui/src/lib/navigationHistory.test.ts
verify:
  - cargo test -p zest-desktop --lib -- window_chrome
  - cargo test -p zest-core --lib -- error::
  - cargo test -p zest-core --lib -- fsutil
  - cargo test -p zest-desktop --features export-bindings --lib export_bindings
  - npm run ui:test
---

# App shell and desktop plumbing

## Behavior
- One window titled "Zest", 1100x720 by default, never smaller than 840x560, dark native theme until the UI applies the saved palette (`tauri.conf.json`).
- Release builds open no console window (`windows_subsystem = "windows"` in `main.rs`) and use mimalloc as the global allocator.
- First launch creates `~/.zest/zest.toml` from the embedded starter config and never overwrites an existing one; `.env`, `~/.zest/.env`, and `~/.zest/env` are loaded before anything else.
- The UI boots through `boot` (skeleton) to `picker`, `waiting` (browser sign-in), or `chat`. A Tauri call that hangs past 15 s (`BOOT_TIMEOUT_MS`) lands on the picker with "Zest could not finish opening its desktop runtime"; an IPC failure says "could not reach its desktop runtime".
- A render crash shows "Something broke" with a "Try again" button that resets the boundary, instead of a blank window.
- Right-click shows no Chromium page menu except inside editable controls.
- Native title bar and window background follow the selected theme; on Windows 11 the caption, caption text, and border colours are retinted through DWM.
- Startup timing marks (`zest-startup:*`, logged as `[zest:startup]`) exist only in dev builds.

## How it works
1. `main.rs` calls `zest_desktop_lib::run()` in `lib.rs`: `ensure_user_config`, `load_env`, `remove_implicit_internal_workspace`, then `tauri::Builder` with the notification plugin, a managed `AppState` (session controller, browser host, approval policy, job registry, ledger, `config_edit` mutex, delegation coordinator), a `setup` hook (browser attach, delegation binding, `forward_job_events`, `remove_retired_spaces_state`), and one `generate_handler!` list that is the entire UI-callable surface.
2. Commands return `Result<_, String>`; structured failures are JSON from `desktop_err` (`code`, `message`, `details`). Streaming reaches the UI as the `chat-event` and `delegation-event` Tauri events.
3. `build.rs` runs `npm run build --prefix ui` (`npm.cmd` on Windows) when `ui/dist/index.html` is missing, so a bare `cargo build -p zest-desktop` needs Node installed.
4. UI: `main.tsx` installs the context-menu guard (`shouldPreserveNativeContextMenu`), external-link handling, and renders `<ErrorBoundary><App/></ErrorBoundary>` under StrictMode.
5. `App.tsx` gets the process-wide backend from `getBackend()` (`backend.ts`). `createTauriBackend` wraps the `invoke` calls in `api.ts`; under `?fixture` in a dev build `selectBackend` returns the offline fixture instead (see [offline-fixture-mode](offline-fixture-mode.md)). The boot effect loads providers, last provider, workspace folder, and profile in parallel, then `pickReadyProvider` decides between `enterChat` and the picker.
6. Shell panels (Customize tabs, Profile, Usage, Pull requests) render in place of the transcript; `navigationHistory.ts` keeps back/forward over them. `ConfirmDialog.tsx` is the shared confirmation modal, used by the history sidebar and both provider pickers.
7. Window chrome: `applyTheme` (`themes.ts`) dynamically imports `windowChrome.ts` only when `__TAURI_INTERNALS__` exists and calls `set_window_chrome(background, appearance)`, which `window_chrome.rs` applies with `set_theme`, `set_background_color`, and the DWM attributes.
8. Wire types: Rust structs derive `ts_rs::TS` behind the `export-bindings` feature; the `export_bindings` test in `lib.rs` writes them to `crates/desktop/ui/src/lib/generated/` (`TS_RS_EXPORT_DIR` in `.cargo/config.toml`), and `types.ts` re-exports and adapts them.
9. Core plumbing shared by every feature: `crates/core/src/lib.rs` re-exports the headless API, `error.rs` defines `HarnessError` and its classifiers (`is_transient`, `is_auth_problem`, `is_context_limit`, `is_unreachable`, `provider_user_message`), and `fsutil.rs` provides `atomic_write` for all durable state.

## Verify
- `cargo test -p zest-desktop --lib -- window_chrome` covers `parse_hex_rgb`.
- `cargo test -p zest-core --lib -- error::` covers error classification, including that retries (`Exhausted`) keep the original classification.
- Regenerate bindings after changing a DTO: `cargo test -p zest-desktop --features export-bindings --lib export_bindings`, then commit `lib/generated/`. `scripts/release-verify.*` fails on drift ("binding drift (ts-rs)" step).
- Fixture boot: `npm run ui:dev`, open `http://127.0.0.1:1420/?fixture=1`.

## Pitfalls
- Adding a command takes five edits: the `#[tauri::command]` fn, the `generate_handler!` entry, the `api.ts` wrapper, the `DesktopBackend` type plus `createTauriBackend` in `backend.ts`, and `createFixtureBackend`. TypeScript flags the last two; a missing `generate_handler!` entry only fails at runtime ("unknown command").
- Never hand-edit `lib/generated/`; it is exempt from feature-map coverage because the owning feature is the Rust type.
- `HarnessError::provider_user_message` only trusts `Stream` errors whose kind starts with `provider:`. `Other` and plain `Stream` text are internal and must not reach a chat bubble.
- `TopbarPanel` deliberately does not portal; portalled menus were fragile in the Tauri webview.
- The comment in `selectBackend` has lost its inline code spans; the gate is `import.meta.env.DEV`, which makes the fixture import dead code in release builds.
- `scratch_dir.rs` is a `#[cfg(test)]` helper (`ScratchDir`, a self-deleting temp dir), not runtime code.
- `reveal.ts` (`revealCount`, streamed-text pacing) currently has no caller outside its own test; check before assuming it drives the transcript.
