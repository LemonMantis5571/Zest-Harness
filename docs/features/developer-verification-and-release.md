---
title: Verification gates and release tooling
summary: The local and CI gates (npm run verify, release-verify), the output-artifact and feature-map checks, git hooks, and the desktop packaging and release scripts.
paths:
  - scripts/package-gpui.mjs
  - packaging/**
  - crates/desktop/build.rs
  - scripts/dev-verify.mjs
  - scripts/release-verify.ps1
  - scripts/release-verify.sh
  - scripts/check-output-artifacts.mjs
  - scripts/feature-map.mjs
  - scripts/install-git-hooks.mjs
  - scripts/release-checksums.mjs
  - scripts/release-checksums.ps1
  - scripts/check-release-version.mjs
  - scripts/make-installer-art.ps1
  - scripts/install-installer-tools.ps1
  - crates/desktop/installer/
  - .githooks/pre-commit
  - .github/workflows/
tests:
  - scripts/check-output-artifacts.test.mjs
  - scripts/feature-map.test.mjs
verify:
  - npm run output-policy:test
  - npm run features:test
  - npm run features:check
  - npm run verify
---

# Verification gates and release tooling

## Behavior
- The root `package.json` has no dependencies; it only names the Node and npm versions and the `npm run` scripts. Node runs the repository's `.mjs` tools with built-in modules.
- The desktop app (`crates/desktop`) is a separate Cargo workspace with its own `Cargo.lock`. The root gates (`cargo fmt`, `clippy`, `cargo test --workspace`, `cargo audit`) cover the root workspace only; the desktop is built and tested by `npm run verify` locally and by `gpui-windows.yml` in CI.
- The Rust lockfile uses `quinn-proto` 0.11.18 and `rustls` 0.23.46 to cover
  RUSTSEC-2026-0338 through 0342 and 0346 through 0349. The RustSec gate remains
  enabled with no new exclusions. Maintenance and yanked-package warnings are
  reported separately from blocking vulnerabilities.
- `npm run verify` (`scripts/dev-verify.mjs`) runs these steps in order and stops at the first failure: output-policy tests, output policy, feature-map tests, feature-map check, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace --lib`, the desktop tests (`cargo test --locked --release --manifest-path crates/desktop/Cargo.toml`), `git diff --check`.
- `scripts/release-verify.ps1` and `release-verify.sh` are the CI and release gate for the root workspace. They run the same checks in the same order (toolchain version print or warning; output policy tests and check; feature map tests and check; fmt; clippy; `cargo test --workspace --all-targets`; `cargo audit` or `cargo deny`; `git diff --check` for unstaged and staged changes).
- CI (`windows-verify.yml`, `linux-verify.yml`) runs the release gate on push to main/master and on PRs and first checks every commit in a PR with `check-output-artifacts.mjs --range`. Linux also builds and tests `zest serve` and the JSONL run protocol without WebKit, using local provider and worker fixtures, and fails if `zest` gains a dependency on WebKit or the desktop crate.
- `gpui-windows.yml` ("Native GPUI") runs on PRs that touch the desktop, `zest-core`, `zest-coordinator`, `zest-plugin-api`, the root `Cargo.toml`, packaging or its own scripts. Windows runs the desktop tests (and checks that the `native_workspace` and `delegation_board` scenarios printed PASS), launches the demo for `scripts/check-desktop-accessibility.ps1 -Check`, and builds the MSI and NSIS installers. Linux runs the tests and builds the DEB, RPM and AppImage. Both jobs time out after 45 minutes.
- CI cache and concurrency policy (both verify workflows): only runs on the default branch save the Rust cache (`Swatinem/rust-cache` with `save-if` comparing `github.ref_name` to the repository default branch, in all three jobs); PR runs restore it and never save. `cargo-audit` comes from `taiki-e/install-action` (a checksum-verified prebuilt binary; the step keeps its name) rather than `cargo install`, which compiled it from source for 2 to 6 minutes whenever the cache was cold. `concurrency` groups a PR by its number with `cancel-in-progress` on, so a newer push cancels that PR's in-flight run; a push to the default branch gets its own group (the run id) and is never cancelled or queued behind another. The job names `Linux full verify`, `Windows full verify` and `zest serve without WebKit` are required status checks in the "Protect master: PR and CI" ruleset; do not rename them. `release.yml` is untouched: tag runs still compile `cargo-audit` and keep their own per-tag concurrency.
- Output policy: under `outputs/`, image/audio/video files (by extension, any case) and files over 1 MiB are rejected unless their exact path is in `allowedOutputArtifacts` (empty by default). Modes: `--tracked` (default, HEAD), `--staged`, `--range <base> <head>` (full SHAs).
- Feature map: `feature-map.mjs check` fails when a doc path matches no file, a `.rs/.ts/.tsx/.mjs/.js/.ps1/.sh` file under `crates/` or `scripts/` has no owner, or the README index is stale. `check --staged` also prints, without failing, features whose code changed while their doc did not. Other commands: `where`, `show`, `list`, `index`, and `--json`.
- `npm run hooks:install` writes a pre-commit wrapper that runs `.githooks/pre-commit` (output policy `--staged`, feature map `--staged`). It refuses to overwrite an existing hook or override a set `core.hooksPath`.

## Release
- Tags `v*` drive `.github/workflows/release.yml` (see `docs/RELEASING.md`). `check-release-version.mjs <tag>` requires that the tag and the root `Cargo.toml` `version` match semver; `crates/desktop/build.rs` stamps that same version into the app and installers. Plain SemVer tags create stable GitHub Releases; tags with a prerelease suffix add `--prerelease`. The workflow publishes only after platform verification and all package jobs pass. Its desktop package jobs test and package the native GPUI app with `npm run gpui:package -- --out release-bundle`; on Windows, `install-installer-tools.ps1` first installs any missing WiX Toolset 3 or NSIS with Chocolatey and puts WiX on the job `PATH`.
- `release-checksums.mjs --root <dir> --out <file>` writes `sha256  name` lines for `.msi .exe .deb .rpm .appimage .dmg` and a bare `zest` binary. It fails when nothing is found or two artifacts share a basename. `release-checksums.ps1` wraps it for `target/gpui/bundle`, where `npm run gpui:package` writes by default.
- The installers are not code-signed yet. The former Tauri signing script was removed with the Tauri app.
- `make-installer-art.ps1` regenerates the 24-bit WiX/NSIS BMPs in `crates/desktop/installer/` from `icons/zest-icon-512.png`.

## Verify
- `npm run output-policy:test`, `npm run features:test`.
- `npm run gpui:package` on Windows (with WiX 3 and NSIS) or Linux builds the installers into `target/gpui/bundle/<version>/<os>`.
- `npm run features:check`; look things up with `npm run features -- where <path>`.

## Pitfalls
- Local `verify` runs only `--lib` Rust tests; integration tests under `crates/*/tests/` run only in the release gate (`--all-targets`).
- The release gate does not build the desktop crate. A change to `zest-core` that breaks the desktop shows up in `gpui-windows.yml` (or in local `npm run verify`), not in the verify workflows.
- In `release-verify.sh`, every step goes through `step`, which captures the exit code with `set +e`; an `if cmd; then` wrapper once let a failing `cargo test` pass.
- PR runs do not save Rust caches, so a PR cannot seed anything for the next one; the default branch's own push run does. A cache saved from `refs/pull/N/merge` is readable only by that PR, yet each Rust cache is 0.5 to 1.7 GB against a 10 GB repository quota that evicts least recently used first. Before `save-if`, one burst of PR runs on 2 Nov uploaded about 14 GB of such copies in 22 minutes, and master's freshly seeded Linux caches were gone when the next master run started, so it rebuilt from scratch. Do not re-enable PR saves to speed up one PR.
- A run is warm only on an exact `Cache cargo` hit; read the step log: `No cache found.` is cold (Linux about 29 min, Windows about 46 min), `Restored from cache key "..." full match: false` is a fallback restore (only changed crates rebuild), `full match: true` followed by `Cache up-to-date.` in the post step is warm (Linux about 9 min, Windows about 12 min). Cold starts still happen when: the key changes (the part before the lockfile hash covers the OS, the job and every rustc version installed on the runner, so a runner-image Rust bump such as 1.99.0 to 1.99.1 misses even the fallback prefix; a `Cargo.lock`, `Cargo.toml` or `rust-toolchain.toml` change only changes the last part); a PR starts before master's push run has finished seeding after a `Cargo.lock` change; the entry sat unused for 7 days; or the seeding run failed, because a failed job saves nothing (`CACHE_ON_FAILURE` is off), including a RustSec failure at the end of the run. A PR that changes `Cargo.lock` restores master's cache by prefix and rebuilds the changed crates on every push, since it cannot save its own.
- Caches already saved under `refs/pull/*` stay until evicted. List them with `gh api --paginate repos/<owner>/<repo>/actions/caches --jq '.actions_caches[] | select(.ref|startswith("refs/pull/")) | [.id,.size_in_bytes,.ref]'` and delete only those ids; `gh cache delete --all` would also delete master's seed. Measure CI with `gh api repos/<owner>/<repo>/actions/runs/<id>/jobs` (`created_at`, `started_at`, `completed_at`, per-step times); machine wait is only 2 to 22 s (median 3 s), so the time is in the job, not the queue.
- The desktop crate's `Cargo.lock` is separate. A dependency added to `zest-core` must also be added to `crates/desktop/Cargo.lock`, or the desktop jobs fail with "cannot update the lock file ... because --locked was passed".
