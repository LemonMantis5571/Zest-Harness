# Working on Zest

Zest is a local-first coding workbench: a Rust workspace with a Tauri desktop
app and a React/TypeScript UI. `crates/core` holds provider-independent
behavior, `crates/coordinator` the delegation queue, `crates/cli` the `zest`
terminal client and `zest serve` daemon, `crates/desktop` the Tauri app, and
`crates/desktop/ui` its web UI. [CONTRIBUTING.md](CONTRIBUTING.md) has the full
setup and conventions; this page is the short version for agents.

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

Use the control CLI instead of writing a throwaway browser script. It keeps one
live session of the UI (offline fixture backend, no credentials), answers in
JSON, and records every console error, page error, failed request, and toast.
[docs/features/ui-control-cli.md](docs/features/ui-control-cli.md) has the details.

```bash
node scripts/zest-control.mjs doctor            # what is installed/running, with fixes
node scripts/zest-control.mjs check             # smoke-test core flows (starts a session if needed)
node scripts/zest-control.mjs start --scenario split-streaming
node scripts/zest-control.mjs send "hello"      # waits for the turn; says if it was queued
node scripts/zest-control.mjs inspect state     # also: messages, code-blocks, meter, composer, toasts
node scripts/zest-control.mjs snapshot          # accessibility tree as YAML
node scripts/zest-control.mjs click "New chat"  # a miss lists the buttons on screen
node scripts/zest-control.mjs press Mod+K
node scripts/zest-control.mjs errors            # everything that went wrong, tagged by command
node scripts/zest-control.mjs screenshot out.png
node scripts/zest-control.mjs stop
```

If you find yourself scripting the same interaction twice, add a command or an
`inspect` probe to `scripts/zest-control-daemon.mjs` instead.

| Goal | Command |
| --- | --- |
| Look at the UI yourself | `npm run ui:dev`, then open `http://127.0.0.1:1420/?fixture=1` |
| Browser tests against that fixture UI | `npm run ui:e2e` (one spec: `npm run ui:e2e -- split-workspace`) |
| UI unit tests | `npm run ui:test` |
| One Rust module's tests | `cargo test -p zest-core --lib -- provider::` |
| One headless agent turn, machine-readable | `cargo run -p zest -- run --jsonl -- "PROMPT"` (uses a real provider and quota) |
| Coordinator daemon with inbound MCP | `zest serve`, see [docs/SERVE.md](docs/SERVE.md) |
| Everything, before you commit | `npm run verify` |

Playwright needs Chromium once per machine: `npx playwright install chromium`.
npm swallows Playwright flags, so for options such as `--repeat-each=8` run
`npx playwright test <spec> --repeat-each=8` from `crates/desktop/ui`.
The fixture backend (`crates/desktop/ui/src/lib/fixtureBackend.ts`) is what the
browser tests run against; extend it when a new UI test needs backend behavior.

Work the inner loop (`cargo check -p <crate>`, one module's tests, one e2e spec)
and run `npm run verify` once at the end; it compiles the workspace twice and
takes minutes. Do not run `cargo fmt` while a build is in flight.

## Rules that are easy to break

- Keep provider-independent behavior in `crates/core`.
- Preserve approval and credential boundaries on every execution path.
- `zest` (crates/cli) must not depend on `zest-desktop` or WebKit; Linux CI
  builds it without WebKitGTK.
- Never hand-edit `crates/desktop/ui/src/lib/generated/`; regenerate the ts-rs
  bindings (see CONTRIBUTING.md).
- Nothing under `outputs/` that is media or larger than 1 MiB.
- Add a focused regression test for each behavior change.
- Conventional Commits (`fix(ui): ...`, `feat(provider): ...`), one behavior
  per change.
