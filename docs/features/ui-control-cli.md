---
title: Desktop control CLI (gpui-control)
summary: One-line commands that launch the desktop app, drive it through UI Automation, and report its state and errors as JSON, so agents verify changes on Windows without writing throwaway scripts.
paths:
  - scripts/gpui-control.mjs
  - scripts/gpui-control.ps1
verify:
  - node scripts/gpui-control.mjs doctor
  - node scripts/gpui-control.mjs start
  - node scripts/gpui-control.mjs send "Demo: write"
  - node scripts/gpui-control.mjs stop
---

# Desktop control CLI (gpui-control)

## Behavior

- `node scripts/gpui-control.mjs <command>` (or `npm run gpui:control -- <command>`) prints JSON and exits 0 on success, 1 when the command failed, 2 for bad usage, 3 when no session is running.
- `doctor` reports the platform, whether the release binary is built (with the build command when it is not), and the running session.
- `start` launches the release build of `zest-desktop`: the offline `--demo` project unless `--project DIR` is given, and `--build` rebuilds first (it also builds when the binary is missing). The app runs with `--no-focus`, so its window does not take focus from whatever the user is doing, and its output goes to a log under `<tmp>/zest-gpui-control/`. A second `start` reuses the running session.
- `inspect` returns the turn state, the transcript rows, the composer text and the page tabs; `messages` returns only the transcript rows as screen readers hear them; `snapshot` lists every named control with its role and state.
- `click NAME [--nth N]` presses a button, tab, switch or radio by accessible name. `type TEXT [--into NAME]` puts text in the message box, or in the field whose accessible name is NAME (a settings form field, for example). `send TEXT [--timeout S]` types and sends, then waits until the reply finishes or an approval or question card appears. `wait-settle [--timeout S]` waits until no turn is running.
- Every action goes through UI Automation patterns (Invoke, SelectionItem, Toggle, Value), never simulated keystrokes or mouse input, so nothing is typed into another window.
- `errors [--clear]` lists panics and error lines from the app's log since start, or since the last `--clear`. Every reply also carries the newest of them under `newErrors`.
- `screenshot FILE` renders the app's window with `PrintWindow`, so other windows on top of Zest never appear in the image.
- `stop` ends the app and removes the session file.
- It is Windows-only. On Linux it says so and points to the desktop tests.

## How it works

1. `gpui-control.mjs` is the client. It keeps the session (process id, log path, arguments, error offset) in `<tmp>/zest-gpui-control/state.json`, launches the app detached with its output redirected to the log, and runs one `gpui-control.ps1` call per command.
2. `gpui-control.ps1` finds the app's main window by process id with `System.Windows.Automation`, walks its descendants, and implements each command: `Find-Named` matches controls by accessible name, `Activate` picks the pattern a control supports, `Get-Composer` reads and sets the message box through the Value pattern, and `Get-State` summarizes the window. It prints one JSON line, which the client parses.
3. The names it relies on come from the app's AccessKit tree: buttons, tabs, switches and the composer all carry accessible names (see [gpui-workspace](gpui-workspace.md)).

## Verify

- `node scripts/gpui-control.mjs start`, then `send "Demo: write"`: the reply stops at the approval card; `click "Allow once"` finishes the turn, and `errors` reports none.
- `inspect` after `click "Customize"` lists the Customize tabs.
- Check that the foreground window stays the same through `start` and `send`.

## Pitfalls

- It drives the release build in `crates/desktop/target/release`. After changing the app, pass `--build` or rebuild before `start`, or you test the old binary.
- A control without an accessible name cannot be clicked. Name new controls (the Windows CI accessibility check fails on unnamed buttons anyway).
- One session at a time per machine: the session file is not per checkout.
- Do not drive the app while someone is typing in another window. On one run, a key pressed in a game reached a Zest form field during `type --into`; the window was started with `--no-focus`, so the cause is not known yet.
- If you find yourself scripting the same interaction twice, add a command to both scripts instead.
