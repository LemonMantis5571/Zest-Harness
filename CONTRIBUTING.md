# Contributing

Thanks for helping improve Zest. Small, focused changes are easier to review
and safer to release than broad refactors.

## Before you start

Read the [README](README.md) for the product workflow and supported platforms.
For release work, also read [docs/RELEASING.md](docs/RELEASING.md).

Use the toolchain versions pinned in `rust-toolchain.toml`, `.nvmrc`, and
`package.json`:

- Rust 1.97.1
- Node.js 24.16.0+
- npm 11.13.0+

## Local development

From the repository root:

```powershell
npm ci
npm run ui:build
npm run desktop:dev
```

On Linux, install the desktop packages listed in the Linux verification
workflow, then run the same npm commands from Bash. PowerShell is not
required.

For Ubuntu 24.04, the CI-equivalent prerequisites are:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev cmake \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libdbus-1-dev libxdo-dev curl wget file tar patchelf libfuse2t64
```

Run the terminal client with:

```powershell
cargo run -p zest
```

The shared Rust library is in `crates/core`, the shared delegation coordinator
is in `crates/coordinator`, the terminal client and `zest serve` daemon are in
`crates/cli`, and the desktop application is in `crates/desktop`. The desktop
web UI lives in `crates/desktop/ui`.

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

Open Customize > Extras, press Refresh, and turn it on. The desktop itself does
not need a Cargo feature for installed add-ons. A Rust plugin is built
separately with `cargo build -p <package> --release`, then installed with its
executable and `plugin.json` in one plugin folder.
Use [`docs/plugin.schema.json`](docs/plugin.schema.json) when validating a
manifest; the desktop loader remains the final authority for safe paths and
supported plugin kinds.

When changing desktop data types, regenerate TypeScript bindings using the
existing ts-rs test workflow rather than editing generated files by hand.

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

The gate checks formatting, linting, Rust and UI tests, generated bindings,
dependency advisories, and Git whitespace. Live provider checks are separate:
they require credentials and may consume real quota.

For a source-only check without dependency audits or generated-binding drift:

```powershell
npm run verify
```

The same command works in Bash.

Add a focused regression test for behavior changes. For UI changes, update the
relevant characterization tests under `crates/desktop/ui/src`.

## Keep out of commits

Do not commit:

- API keys, `.env` files, credential-manager exports, or signing keys;
- generated `ui/dist` output; or
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

The first beta is tag-driven. Maintainers should read
[`docs/RELEASING.md`](docs/RELEASING.md) before pushing a `v*` tag; the release
workflow verifies both platforms, builds their packages, and publishes the
GitHub Release only after both package jobs pass.

## Security reports

Do not open a public issue for a vulnerability. Follow the private reporting
process in [SECURITY.md](SECURITY.md).
