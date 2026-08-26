# Zest desktop UI

Vite + React + TypeScript webview for `zest-desktop` (Tauri). Built assets land
in `dist/` and are loaded by the Rust shell. This package is not a standalone
web app; use the repository root commands for a normal desktop build.

## Commands

From repo root (preferred):

```powershell
npm ci
npm run ui:build      # tsc + vite -> dist/
npm run ui:test
npm run desktop:dev   # Tauri + Vite HMR
npm run ui:dev        # browser-only UI preview; Tauri APIs are unavailable
```

For a reproducible checkout, use `npm ci` from the repository root. A full
source-build and installer walkthrough is in [`README.md`](../../../README.md).

From this directory: `npm run build` / `npm test` / `npm run lint`.

## Layout notes

- **Projects sidebar** - `ChatHistorySidebar` + `list_chat_projects` / `open_project_chat`
- **Chat** - `ChatScreen`, `chatReducer`, streaming events, approvals, `DiffViewer`
- **Composer** — attachments, paste images, folder/branch/context footer
- **Settings** — provider, user profile, system prompt, skills, usage
- **Backend** — `lib/backend.ts` switches Tauri invoke vs `?fixture=1` offline smoke

## Conventions

- No Base UI Menu/Portal (WebView crash risk) - plain positioned panels
- Interactive controls: `cursor-pointer` (global CSS + shared `Button`)
- Generated DTOs under `src/lib/generated/` (see that folder’s README for ts-rs)

This is not a standalone Vite template app; treat it as part of the Zest desktop crate.
