---
title: Offline fixture mode
summary: Run the desktop UI in an ordinary browser with an in-memory fake backend and a canned turn; every Playwright spec runs against it.
paths:
  - crates/desktop/ui/src/lib/fixture.ts
  - crates/desktop/ui/src/lib/fixtureBackend.ts
  - crates/desktop/ui/src/lib/backend.ts
  - crates/desktop/ui/playwright.config.ts
  - crates/desktop/ui/vite.config.ts
  - crates/desktop/ui/e2e/fixtures/
tests:
  - crates/desktop/ui/src/lib/fixtureBackend.test.ts
  - crates/desktop/ui/e2e/fixtureBoot.ts
verify:
  - npm run ui:test
  - npm run ui:e2e
---

# Offline fixture mode

## Behavior
- `npm run ui:dev`, then `http://127.0.0.1:1420/?fixture=1`: the full chat shell runs with no Tauri, provider, or credentials. `?fixture` is honoured only by the Vite dev server (`import.meta.env.DEV`); release builds drop the module.
- On load the fixture starts session `session-fixture` / thread `fixture` and streams one canned turn: user "What's in README.md?", a thinking delta, four `read_file` tool calls (exercising run grouping), then a word-by-word reply. A second thread, `fixture-local` (provider `ollama`), shows as busy with a `git_status` tool for about 9 s to exercise the sidebar activity mark.
- Sending a message echoes it: `Fixture echo: <text>`. A `followup`/`steer`/`inject` target queues it (`input_queued`) instead, in every scenario, including during a live `split-streaming` turn.
- Like the desktop, `contextUsage` rejects with a `busy` error envelope while a turn is in flight on the active chat (a live `split-streaming` send or a pending safety scenario).
- Every backend method call is counted per page on `globalThis.__zestFixtureCalls` (`recordFixtureCalls`), so a spec or `zest-control inspect backend-calls` can see what the UI asked for. `generateReadingDiff` returns the diff unabridged.
- Seeded sidebar: "Fixture", "Local model chat" (`fixture-local`), "Fifteen turns" (`fixture-long`, 15 user turns for history windowing), and "Free chat" (`fixture-free`) under Free chats. Rename, pin, delete, fork, rewind, edit, search, and older/newer paging work on in-memory state.
- Also faked: two MCP servers (`Haiku`, `github`), external agents `claude` and `gemini`, one delegation job (approve runs the whole worker/reviewer lifecycle synchronously to `ready_to_apply`), Now Playing and Wallpaper plugins (both off until turned on), usage/profile/quota figures, workspace files (`src/`, `README.md`, `Cargo.toml`), git context with PR #13, `/btw` side conversations, system prompt, and the slash-command list.
- Not available (throws "fixture backend: X is not available" or similar): provider key and `configure*Provider` calls, `startLogin`, `openProjectConfig`, `openPricesFile`, `revealWorkspaceFolder`, `jobOutput`, `jobKill`. `checkExternalAgent` and `checkMcpServer` return a not-available result.
- `?scenario=<name>` changes behavior: `approval`, `question`, `cancel`, `tool-error` (safety turns on send), `options-delayed` / `options-failing` (900 ms model/effort saves; the first failing one throws), `provider-picker` (reports `mode: "tauri"`, first `startSession` fails so the picker shows, adds two extra providers), `model-catalogue` (54 extra long-named models), `split-streaming` (sends stay live until cancelled), `btw-streaming` (a side answer stays live after its first chunk until Stop or close), `pull-request-delayed` (800 ms PR diff).

## How it works
1. `getBackend()` in `backend.ts` caches one backend per page; `selectBackend` returns `createFixtureBackend()` when `import.meta.env.DEV` and the URL has `fixture`.
2. `createFixtureBackend` in `fixtureBackend.ts` implements the whole `DesktopBackend` type with closure state; `scenarioFromLocation` reads `?scenario`. `mode` is `"fixture"` except for `provider-picker` and `model-catalogue`, which take App's normal boot path.
3. In fixture mode `App.tsx` calls `startSession("fixture")` (bounded by `BOOT_TIMEOUT_MS`, 15 s), then `backend.boot(handleChatEvent)`, which is not bounded; `boot` runs `runFixtureStream` (`fixture.ts`) only when there is no scenario or a safety scenario.
4. Events go through the handler registered by `onChatEvent`/`onDelegationEvent`; a generation counter stops a StrictMode double subscription from clearing the live sink.
5. Safety scenarios keep one `pendingScenario`; `resolveApproval`, `resolveQuestion`, and `cancelTurn` finish it (cancel also auto-fires after 5 s).
6. Playwright (`playwright.config.ts`): tests in `e2e/`, 2 workers, fully parallel, base URL `http://127.0.0.1:1420`, 1280x800, traces kept on failure. Its `webServer` runs `npm run dev -- --host 127.0.0.1` and reuses an already running dev server outside CI. `vite.config.ts` pins port 1420 (`strictPort`) and the `@` alias.

## Verify
- `node scripts/zest-control.mjs start` drives this fixture one command at a time and `check` smoke-tests it; see [ui-control-cli](ui-control-cli.md).
- `npm run ui:test` runs `fixtureBackend.test.ts` (side conversations, rename, free chats, search, delegation lifecycle, plugins and files, safety scenarios, windowed open, PR review, queued-message recovery).
- `npm run ui:e2e` or one spec: `npm run ui:e2e -- split-workspace`. Every spec except `streaming-performance` opens `/?fixture=1`; that one loads the standalone harness `e2e/fixtures/streaming.html`. Needs `npx playwright install chromium` once.

## Adding fixture behavior for a new e2e test
- New backend method: implement it in `createFixtureBackend` (TypeScript requires it once it is on `DesktopBackend`). Prefer deterministic in-memory state over `notAvailable`.
- New situation: add the name to the `FixtureScenario` union and to the `scenarioFromLocation` check, branch on `scenario` in the methods involved, cover it in `fixtureBackend.test.ts` via `createFixtureBackend({ scenario })`, and open `/?fixture=1&scenario=<name>` from the spec.
- Preference-dependent UI can be pinned with `page.addInitScript` writing `localStorage` (the split spec sets `zest.responseBlockStreaming.v1`).

## Pitfalls
- Fixture state is per page load; nothing persists, and specs cannot share state.
- The canned boot stream is a turn, not startup, and must stay outside the boot timeout. It used to be inside it: on a slow CI runner the stream passed 15 s and the app dropped an open chat back to the provider picker mid-test, which surfaced as an unrelated-looking `slash-commands.spec.ts` click timeout on Windows. To reproduce slow-runner timing locally, throttle the CPU in a spec with CDP `Emulation.setCPUThrottlingRate` (6x reproduced it).
- A spec that types into the seeded chat right after `goto` races that boot turn: sends are queued and its end re-renders the chat. Call `waitForFixtureBoot(page)` (`e2e/fixtureBoot.ts`) first, as `zest-control start` does, or use a scenario (which skips the boot turn). To assert on a live turn or side answer, use `split-streaming` or `btw-streaming` instead of racing the canned timing.
- Default `sendMessage` responds synchronously with the full echo; only `split-streaming` leaves a turn running. Specs that need a live turn must use that scenario or add one.
- A new empty thread is not listed until it has a message, matching the desktop store.
- `lib/*.ts` modules loaded by `node --test` must import siblings with relative `.ts` paths; the `@/` alias only resolves under Vite.
- A local `npm run ui:dev` on port 1420 is reused by Playwright; stale code there means stale test results.
