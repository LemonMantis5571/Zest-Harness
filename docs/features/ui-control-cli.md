---
title: UI control CLI (zest-control)
summary: One-line commands that drive the live UI, inspect it, and report every error, so agents debug and verify without writing throwaway browser scripts.
paths:
  - scripts/zest-control.mjs
  - scripts/zest-control-daemon.mjs
tests:
  - scripts/zest-control.test.mjs
verify:
  - npm run control:test
  - npm run control:check
---

# UI control CLI (zest-control)

## Behavior

- `node scripts/zest-control.mjs <command>` prints JSON (`snapshot` prints YAML)
  and exits 0 on success, 1 when the command failed, 2 for bad usage, 3 when no
  session is running. Failures carry an `error` and, where the cause is known,
  a `hint` with the next command to run.
- `start` launches one session per checkout: the Vite dev server on port 1420
  (reused if the Zest UI is already served there) and headless Chromium on
  `/?fixture=1`. It waits for the seeded chat's canned boot turn to finish, so
  the first `send` is not queued. `--scenario` picks a fixture scenario;
  `--headed` shows the browser. A second `start` reuses the running session.
- The session records console errors and warnings, uncaught page errors,
  failed requests, HTTP errors, crashes, and every toast, each tagged with the
  command that was running (`idle` between commands). Every command's reply
  lists the errors it caused under `newErrors`; `errors` returns them all.
- `send` reports `outcome: "sent"` and the reply once the turn settles. If the
  chat already has a turn running, the message is queued instead and `send`
  fails with a hint, unless `--allow-queue` is given.
- `wait-settle` succeeds only when no turn is running (no Stop button) and two
  samples 300 ms apart match. A turn that stays live (`split-streaming`) fails
  with a hint to use `stop-turn`.
- A failed `click` lists the accessible names that are on screen.
- `inspect backend-calls` returns how many times the UI called each fixture
  backend method since the page loaded, for spotting repeated or missing calls.
- `check` runs boot, send, ruby highlighting, context meter, command palette,
  and stop-turn (with the meter kept during the turn), then fails if any error
  was recorded. Without a session it starts one and stops it afterwards. A
  failed step saves a screenshot to `ZEST_CONTROL_ARTIFACTS`, or the temp
  directory.
- `stop` closes the browser, stops the dev server only if the session started
  it, and removes the session file. An idle session stops itself after 30
  minutes.

## How it works

1. `zest-control.mjs` is the client. It parses arguments (`parseArgs`), reads
   the session file from `statePath()` (`<tmp>/zest-control/<hash>.json`, never
   in the repository), and sends one command per call to the daemon over
   loopback HTTP with a per-session token. `doctor` and `start` run in the
   client; `check` starts an ephemeral session when none is running.
2. `zest-control-daemon.mjs` is spawned detached by `start`. `ensureServer`
   reuses or starts Vite (`node_modules/vite/bin/vite.js` in `crates/desktop/ui`),
   `launchBrowser` opens a 1280x800 page (the Playwright specs' viewport),
   attaches the event listeners, and installs a `MutationObserver` init script
   that reports toasts through `console.debug` with a `[zest-control:toast]`
   prefix. Commands run one at a time in arrival order.
3. UI knowledge lives in the daemon's helpers: `#zest-composer-input`, Stop as
   `Stop` or `Stop response`, message rows as `[data-slot="message"]` with
   `data-align` end (user) or start (assistant), the queued panel under
   "Queued messages", code blocks by `data-slot` with
   `code-block-language[data-unsupported]`, and the meter by its
   `Context usage` title.
4. Scenario names are read from the `FixtureScenario` union in
   `fixtureBackend.ts`, so validation and `help` never drift from the fixture.

## Verify

- `npm run control:test` covers argument parsing, the session path, scenario
  discovery and validation, and help text.
- `npm run control:check` exercises the whole CLI against the live UI. It runs
  in `npm run verify` and both release-verify scripts after the Playwright
  suite, so a broken probe fails the gate.

## Pitfalls

- It drives the offline fixture UI, not the Tauri app. Bugs in Rust commands
  only show here if the fixture models them; when one slips through, fix the
  fixture too ([offline-fixture-mode](offline-fixture-mode.md)).
- One session per checkout: two agents in the same checkout share it, and
  `open`, `scenario`, `pref`, and `check` reload the page and reset fixture
  state.
- When a UI selector the daemon relies on changes (composer id, Stop label,
  `data-slot` names), update the helper in `zest-control-daemon.mjs`; `check`
  in the gate is what catches the drift.
- `pref` reloads the page, so fixture state starts fresh after it.
