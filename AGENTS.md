# Working on Zest

Zest is a local-first coding workbench: a Rust workspace plus a native desktop
app built with GPUI. `crates/core` holds provider-independent behavior,
`crates/coordinator` the delegation queue, `crates/cli` the `zest` terminal
client and `zest serve` daemon, and `crates/desktop` the desktop app. The
desktop crate is its own Cargo workspace with its own `Cargo.lock`, so root
`cargo` commands do not build it.
[CONTRIBUTING.md](CONTRIBUTING.md) has the full setup and conventions; this
page is the short version for agents.

## Find your way with the feature map

[docs/features/](docs/features/README.md) describes every feature: how it should
behave, the path through the code, its tests, and how to verify it. Read the
relevant entry before changing a feature or triaging a bug report against it.

```bash
npm run features -- where crates/core/src/tools/bash.rs   # which feature owns a file
npm run features -- show delegation                       # its files, tests, verify commands
npm run features -- list                                  # every feature
```

Keep it true. When you change what a feature does or where it lives, update its
entry in the same change. A new source file under `crates/` or `scripts/` must
be added to a feature's `paths` or `tests`; `npm run features:check` fails
otherwise, and so do the pre-commit hook and CI.

## Drive the app and prove a change

On Windows, use the control CLI instead of writing a throwaway automation
script. It launches the release app (an offline demo project unless you pass
`--project`), answers in JSON, and collects panics and errors from the app's
output. It drives the window only through UI Automation patterns, never
simulated keystrokes, and starts the app with `--no-focus` so it does not take
focus from whatever else is on screen.

```bash
node scripts/gpui-control.mjs doctor             # what is built and running, with fixes
node scripts/gpui-control.mjs start              # offline demo; --project DIR for a real one, --build to rebuild
node scripts/gpui-control.mjs send "Demo: write" # waits for the reply or an approval/question card
node scripts/gpui-control.mjs click "Allow once" # buttons, tabs, switches and radios by accessible name
node scripts/gpui-control.mjs inspect            # turn state, messages, composer, tabs
node scripts/gpui-control.mjs snapshot           # every named control with its role and state
node scripts/gpui-control.mjs errors             # panics and errors since start (or the last --clear)
node scripts/gpui-control.mjs screenshot out.png
node scripts/gpui-control.mjs stop
```

If you find yourself scripting the same interaction twice, add a command to
`scripts/gpui-control.mjs` and `scripts/gpui-control.ps1` instead. On Linux,
rely on the desktop tests below.

| Goal | Command |
| --- | --- |
| Look at the app yourself | `npm run gpui -- --demo` (offline) or `npm run dev` |
| Desktop tests, including the scripted workspace scenario | `cargo test --locked --release --manifest-path crates/desktop/Cargo.toml` |
| The same scenario in a real window | `npm run gpui:test` |
| One Rust module's tests | `cargo test -p zest-core --lib -- provider::` |
| One headless agent turn, machine-readable | `cargo run -p zest -- run --jsonl -- "PROMPT"` (uses a real provider and quota) |
| Coordinator daemon with inbound MCP | `zest serve`, see [docs/SERVE.md](docs/SERVE.md) |
| Everything, before you commit | `npm run verify` |

The offline demo provider streams a canned reply; `Demo: write` asks to write a
file and `Demo: question` asks a question, so approval and question cards can
be exercised without credentials
([docs/features/offline-fixture-mode.md](docs/features/offline-fixture-mode.md)).

Work the inner loop (`cargo check -p <crate>`, one module's tests,
`cargo check --manifest-path crates/desktop/Cargo.toml`) and run
`npm run verify` once at the end; it builds both workspaces and takes minutes.
Do not run `cargo fmt` while a build is in flight.

## Rules that are easy to break

- Keep provider-independent behavior in `crates/core`.
- Preserve approval and credential boundaries on every execution path.
- `zest` (crates/cli) must not depend on `zest-desktop` or WebKit; Linux CI
  builds it without WebKitGTK.
- A new dependency of `zest-core` or `zest-coordinator` must also land in
  `crates/desktop/Cargo.lock`, or the desktop CI jobs fail on `--locked`.
- Nothing under `outputs/` that is media or larger than 1 MiB.
- Add a focused regression test for each behavior change.
- Conventional Commits (`fix(desktop): ...`, `feat(provider): ...`), one
  behavior per change.
