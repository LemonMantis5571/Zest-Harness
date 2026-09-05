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

For a reproducible checkout, use `npm ci` from the repository root. Toolchains,
source builds and verification are documented in
[`CONTRIBUTING.md`](../../../CONTRIBUTING.md).

From this directory: `npm run build` / `npm test` / `npm run lint`.

## Browser regression tests

After `npm ci`, install the test browser once with
`npm exec -w ui -- playwright install chromium`. From the repository root, run
`npm run test:e2e -w ui`. Playwright starts Vite on port 1420 (or uses an existing
local dev server). Tests use synthetic fixture data and do not call live providers.
Traces and screenshots for failures are written to `test-results/`.

The growing-code benchmark is opt-in. In PowerShell:

```powershell
$env:ZEST_PERF = "1"
npm run test:e2e -w ui -- streaming-performance.spec.ts --workers=1
Remove-Item Env:ZEST_PERF
```

In Bash, prefix the command with `ZEST_PERF=1`. The report contains five runs
each at 20 and 100 KiB, with 40 updates at a requested 25 ms cadence. These are
browser development-build measurements, not native release performance guarantees.

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
