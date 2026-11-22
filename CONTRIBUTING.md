# Contributing

Thanks for helping improve Zest. Small, focused changes are easier to review
and safer to release than broad refactors.

## Before you start

Read the [README](README.md) for the product workflow and supported platforms.
For release work, also read [docs/RELEASING.md](docs/RELEASING.md).

Use the toolchain versions pinned in `rust-toolchain.toml`, `.nvmrc`, and
`package.json`:

- Rust 1.97.1
- Node.js 24.16.0+ (the repository's scripts are plain Node with no packages)
- npm 11.21.0

## Local development

From the repository root:

```powershell
npm run hooks:install
npm run dev
```

`npm run dev` builds and opens the desktop app in release mode; the first build
takes several minutes. `npm run gpui -- --demo` opens it on a temporary offline
project that needs no provider or credentials. On Windows, building the desktop
needs MSVC and the Windows SDK.

The hook installer enables a local pre-commit check for staged files under
`outputs/` and for the [feature map](docs/features/README.md). CI checks tracked files and every file introduced or modified by a
pull-request commit, including files later removed in that branch. Image,
audio/video, and output files larger than 1 MiB are blocked unless their exact
repository path is added to the allowlist in
`scripts/check-output-artifacts.mjs` after review. Keep generated captures out
of `outputs/`; put intentional product or docs media in the corresponding
`assets/` or `docs/` directory.

On Linux, run the same npm commands from Bash. PowerShell is not required.
For Ubuntu 24.04, the CI-equivalent prerequisites are:

```bash
sudo apt-get update
# The root workspace (core, coordinator, CLI)
sudo apt-get install -y build-essential pkg-config libssl-dev cmake libdbus-1-dev
# The desktop app: GPUI's windowing and Vulkan headers, WebKitGTK for web views
sudo apt-get install -y clang libfontconfig-dev libwayland-dev libx11-xcb-dev \
  libxkbcommon-x11-dev libxkbcommon-dev libasound2-dev libvulkan1 \
  libwebkit2gtk-4.1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev
```

`.github/workflows/gpui-windows.yml` has the full list, including the packaging
tools (`dpkg-dev`, `rpm`, `squashfs-tools`).

Run the terminal client with:

```powershell
cargo run -p zest
```

The shared Rust library is in `crates/core`, the shared delegation coordinator
is in `crates/coordinator`, the terminal client and `zest serve` daemon are in
`crates/cli`, and the desktop application is in `crates/desktop`. The desktop
is a separate Cargo workspace with its own `Cargo.lock` because GPUI is pinned
to a Zed git revision. Build it with `--manifest-path crates/desktop/Cargo.toml`
or the `npm run gpui*` scripts; root `cargo` commands skip it. When a change
adds a dependency to `zest-core` or `zest-coordinator`, update
`crates/desktop/Cargo.lock` too.

`zest serve` is a coordinator-only process. It must not depend on the desktop
crate or WebKit. Linux CI builds and tests it without WebKitGTK. See
[docs/SERVE.md](docs/SERVE.md).

Optional desktop add-ons are separate processes and are not part of the normal
desktop build. Read [docs/PLUGINS.md](docs/PLUGINS.md) for the plugin standard,
folder layout, protocol, security model, and review checklist. Build and
install a sample add-on with:

```sh
npm run plugin:install -- wallpaper
npm run plugin:install -- now-playing
```

Open Customize > Add-ons, press Refresh, and turn it on. The desktop itself does
not need a Cargo feature for installed add-ons. A Rust plugin is built
separately with `cargo build -p <package> --release`, then installed with its
executable and `plugin.json` in one plugin folder.
Use [`docs/plugin.schema.json`](docs/plugin.schema.json) when validating a
manifest; the desktop loader remains the final authority for safe paths and
supported plugin kinds.

## Verification

Run the repository verification gate before opening a pull request:

```powershell
./scripts/release-verify.ps1
```

On Linux or macOS:

```bash
cargo install cargo-audit --locked
bash scripts/release-verify.sh
```

The gate checks formatting, linting, the root workspace's Rust tests, the
feature map, dependency advisories, and Git whitespace. Live provider checks
are separate: they require credentials and may consume real quota.

The desktop app has its own CI workflow (`gpui-windows.yml`). It runs the
desktop tests, including a scripted workspace session against an offline
provider, checks UI Automation names on Windows, and builds the installers.
Run its tests locally with:

```powershell
cargo test --locked --release --manifest-path crates/desktop/Cargo.toml
```

On Windows, `scripts/gpui-control.mjs` drives the running app one command at a
time (`start`, `send`, `click`, `inspect`, `errors`, `snapshot`, `stop`;
`help` lists them) through UI Automation, for debugging and for agents.

For a source-only check without dependency audits that also runs the desktop
tests:

```powershell
npm run verify
```

The same command works in Bash.

### While you are still working

`npm run verify` is a pre-commit gate, not a test command. It runs the feature
map check, `cargo fmt --check`, `clippy --workspace --all-targets`,
`cargo test --workspace --lib`, and the desktop tests. Clippy and the test
build have different fingerprints, so the root workspace is compiled twice, and
the desktop is a third, separate release build. That is minutes per run, and
almost none of it is spent running tests: a full `cargo test -p zest-core --lib`
executes in about 15 seconds once it is built.

Work the inner loop instead, and save the gate for the end:

```bash
cargo check -p zest-core                      # types only, no test build
cargo test -p zest-core --lib -- provider::   # one module's tests
cargo test -p zest-core --lib                 # the crate's tests
npm run verify                                # once, before you commit
```

Two things that waste the most time:

- Running `cargo fmt` while a build is in flight. The build reads files the
  formatter is rewriting and fails in ways that do not reproduce.
- Reaching for `cargo build` when `cargo check` answers the question. Only build
  when you are about to run something.

Tests that spawn child processes (`tools::external_agent`) carry real timeouts,
so they are the first to flake when the machine is already busy compiling. Rerun
them alone before believing a failure.

Add a focused regression test for behavior changes. For desktop changes, add a
unit test next to the code in `crates/desktop/src`, or extend the scripted
workspace scenario in `crates/desktop/src/workspace_test.rs` when the change is
about a whole flow.

## Feature map

[`docs/features/`](docs/features/README.md) has one entry per feature: expected
behavior, the path through the code, tests, and verify commands. It is
navigation for people and agents, so it has to stay true:

- Update a feature's entry in the same change that alters its behavior or moves
  its code. The pre-commit hook lists features whose code you changed without
  touching their entry.
- Add every new source file under `crates/` or `scripts/` to some feature's
  `paths` or `tests`. `npm run features:check` fails on unowned files and on
  entries that name files which no longer exist.
- `npm run features -- where <path>` and `npm run features -- show <slug>` look
  things up.

## Keep out of commits

Do not commit:

- API keys, `.env` files, credential-manager exports, or signing keys;
- generated `ui/dist` output; or
- generated image/audio/video media or files larger than 1 MiB under `outputs/`,
  unless explicitly allowlisted; or
- local signing configuration or personal `zest.toml` files.

Use [`zest.toml.example`](zest.toml.example) for shareable configuration
documentation.

## Code and UI conventions

- Keep provider-independent behavior in `crates/core`.
- Preserve approval and credential boundaries when changing execution paths.
- Keep user-facing copy actionable and free of debugging details.
- Reuse the existing design tokens and local UI primitives.
- Explain why non-obvious code exists, especially around process and security
  boundaries.

## Pull requests

Use [Conventional Commits](https://www.conventionalcommits.org/), such as:

```text
feat(provider): add compatible endpoint setup
fix(cli): handle streamed tool metadata
docs(release): explain beta installers
chore(deps): remove unused direct dependency
```

Keep each pull request scoped. Describe user-visible behavior, verification
performed, and any migration or release impact.

Repository maintainers should protect `master` by requiring the `Windows full
verify`, `Linux full verify`, and `zest serve without WebKit` checks, and by
disallowing direct pushes. CI can reject an artifact in a pushed commit, but
only branch protection prevents bypassing the PR checks.

Releases are tag-driven. Maintainers should read
[`docs/RELEASING.md`](docs/RELEASING.md) before pushing a `v*` tag. Plain
SemVer tags create stable releases; tags with a prerelease suffix create
prereleases. The workflow verifies both platforms, builds their packages, and
publishes only after all checks pass.

## Security reports

Do not open a public issue for a vulnerability. Follow the private reporting
process in [SECURITY.md](SECURITY.md).
