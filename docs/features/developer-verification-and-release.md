---
title: Verification gates and release tooling
summary: The local and CI gates (npm run verify, release-verify), the output-artifact and feature-map checks, git hooks, UI lint rules, and the release packaging scripts.
paths:
  - crates/desktop/build.rs
  - crates/desktop/windows-common-controls.rc
  - crates/desktop/windows-common-controls.manifest
  - scripts/dev-verify.mjs
  - scripts/release-verify.ps1
  - scripts/release-verify.sh
  - scripts/desktop-smoke.mjs
  - scripts/check-output-artifacts.mjs
  - scripts/feature-map.mjs
  - scripts/install-git-hooks.mjs
  - scripts/build-signed.ps1
  - scripts/release-checksums.mjs
  - scripts/release-checksums.ps1
  - scripts/check-release-version.mjs
  - scripts/make-installer-art.ps1
  - crates/desktop/ui/.oxlint-plugins/zest-boundaries.mjs
  - crates/desktop/ui/.oxlint-plugins/anti-slop.mjs
  - crates/desktop/ui/.oxlintrc.json
  - crates/desktop/tauri.signing.conf.json.example
  - crates/desktop/installer/
  - .githooks/pre-commit
  - .github/workflows/
tests:
  - crates/desktop/ui/src/lib/sourceMapSecurity.test.mjs
  - scripts/check-output-artifacts.test.mjs
  - scripts/feature-map.test.mjs
  - crates/desktop/ui/.oxlint-plugins/test.mjs
  - crates/desktop/ui/.oxlint-plugins/anti-slop.test.mjs
verify:
  - npm run output-policy:test
  - npm run features:test
  - npm run features:check
  - npm run ui:lint:plugins
  - npm run verify
---

# Verification gates and release tooling

## Behavior
- npm is pinned to 11.21.0 in the root manifest. CI verification and desktop packaging install that declared version after Node setup so overrides propagate through workspace links. Older npm releases can retain an unpatched transitive version or mark a valid override as invalid. The Mermaid KaTeX override is exercised by the UI security regression and the unchanged dependency-audit gate.
- The build tooling locks `source-map-js` to patched 1.2.2. Its regression test resolves the consumer through PostCSS, checks ordinary indexed mappings, rejects malformed or excessive section offsets, and bounds a large-offset conversion in a separate process so a regression cannot hang the test runner.
- The Rust lockfile uses `quinn-proto` 0.11.18 and `rustls` 0.23.46 to cover
  RUSTSEC-2026-0338 through 0342 and 0346 through 0349. The RustSec gate remains
  enabled with no new exclusions. Maintenance and yanked-package warnings are
  reported separately from blocking vulnerabilities.
- `npm run verify` (`scripts/dev-verify.mjs`) runs these steps in order and stops at the first failure: output-policy tests, output policy, feature-map tests, feature-map check, control CLI tests, UI unit tests, UI lint, lint-plugin tests, UI build, Playwright e2e (`npm run ui:e2e`), the control CLI smoke test (`zest-control.mjs check`, see [ui-control-cli](ui-control-cli.md)), `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace --lib`, `git diff --check`.
- `scripts/release-verify.ps1` and `release-verify.sh` are the CI and release gate. They run the same checks in the same order (toolchain version print or warning; output policy tests and check; feature map tests and check; control CLI tests; `npm ci`; ts-rs binding drift; UI test, lint, plugin-rule tests, build; Playwright e2e with `npx playwright install chromium`, plus `--with-deps` on Linux CI; the control CLI smoke test; fmt; clippy; `cargo test --workspace --all-targets`; `npm audit --omit=dev` with 3 tries; `cargo audit` or `cargo deny`; `git diff --check` for unstaged and staged changes).
- CI (`windows-verify.yml`, `linux-verify.yml`) runs the release gate on push to main/master and on PRs, first checks every commit in a PR with `check-output-artifacts.mjs --range`, and uploads `crates/desktop/ui/test-results/` as `playwright-traces-windows` / `playwright-traces-linux` on failure. Linux also builds and tests `zest serve` and the JSONL run protocol without WebKit, using local provider and worker fixtures.
- CI cache and concurrency policy (both verify workflows): only runs on the default branch save the Rust cache (`Swatinem/rust-cache` with `save-if` comparing `github.ref_name` to the repository default branch, in all three jobs); PR runs restore it and never save. `cargo-audit` comes from `taiki-e/install-action` (a checksum-verified prebuilt binary; the step keeps its name) rather than `cargo install`, which compiled it from source for 2 to 6 minutes whenever the cache was cold. `concurrency` groups a PR by its number with `cancel-in-progress` on, so a newer push cancels that PR's in-flight run; a push to the default branch gets its own group (the run id) and is never cancelled or queued behind another. The job names `Linux full verify`, `Windows full verify` and `zest serve without WebKit` are required status checks in the "Protect master: PR and CI" ruleset; do not rename them. `release.yml` is untouched: tag runs still compile `cargo-audit` and keep their own per-tag concurrency.
- Output policy: under `outputs/`, image/audio/video files (by extension, any case) and files over 1 MiB are rejected unless their exact path is in `allowedOutputArtifacts` (empty by default). Modes: `--tracked` (default, HEAD), `--staged`, `--range <base> <head>` (full SHAs).
- Feature map: `feature-map.mjs check` fails when a doc path matches no file, a `.rs/.ts/.tsx/.mjs/.js/.ps1/.sh` file under `crates/` or `scripts/` has no owner (`lib/generated/` is exempt), or the README index is stale. `check --staged` also prints, without failing, features whose code changed while their doc did not. Other commands: `where`, `show`, `list`, `index`, and `--json`.
- `npm run hooks:install` writes a pre-commit wrapper that runs `.githooks/pre-commit` (output policy `--staged`, feature map `--staged`). It refuses to overwrite an existing hook or override a set `core.hooksPath`.
- UI lint (`oxlint --deny-warnings` plus the `zest` JS plugin), all errors: `no-unvalidated-persisted-json` (parsed JSON stays `unknown`/`JsonValue` until a type guard checks it), `no-secret-persistence-or-sink` (no token/secret/password-like names in `localStorage`/`sessionStorage` or `console`), `require-safe-html-provenance` (`dangerouslySetInnerHTML` only through `markTrustedHtml(...)`), `no-unowned-background-rejection` (no empty or constant `.catch`), `no-object-url-leak` (every `URL.createObjectURL` revoked in the module).
- UI lint also runs six rules vendored from [anti-slop](https://github.com/dmmulroy/anti-slop) (`.oxlint-plugins/anti-slop.mjs`, MIT, ported from TypeScript to plain ESM), all errors: `no-chained-type-assertions` (no `as unknown as T`; type the source or parse at the boundary), `no-widen-then-assert` (no widening a known value to `unknown`/`object` and asserting it back), `no-conditional-empty-object-spread` (build the object, then add optional fields), `no-array-filter-map` (one `flatMap` instead of adjacent `filter`/`map` passes), `no-reduce-accumulator-copy` and the built-in `oxc/no-accumulating-spread` (no copying a reducer accumulator per step), and `no-module-mocking` (no `vi.mock`/`jest.mock`). The upstream rule tests run in `anti-slop.test.mjs`. anti-slop is meant to be vendored and edited, so adjust the rules in place; its noisier rules (`no-runtime-typeof`, `no-unknown-*`, `require-safety-comment-for-type-assertion`) are deliberately not enabled.
- `npm run desktop:smoke` is opt-in (`ZEST_DESKTOP_SMOKE=1`): it starts `target/debug/zest-desktop` (or `ZEST_DESKTOP_PROFILE=release`, or `ZEST_DESKTOP_BINARY`), expects it alive after 4 s and responding on Windows, then kills it. It skips without the variable or binary and is in no gate.

## Release
- Tags `v*` drive `.github/workflows/release.yml` (see `docs/RELEASING.md`). `check-release-version.mjs <tag>` requires that the tag, `Cargo.toml` `version`, and `crates/desktop/tauri.conf.json` `version` match semver. Plain SemVer tags create stable GitHub Releases; tags with a prerelease suffix add `--prerelease`. The workflow publishes only after platform verification and all package jobs pass.
- `release-checksums.mjs --root <dir> --out <file>` writes `sha256  name` lines for `.msi .exe .deb .rpm .appimage .dmg` and a bare `zest` binary. It fails when nothing is found or two artifacts share a basename. `release-checksums.ps1` wraps it for `target/release/bundle`.
- `build-signed.ps1 [-Thumbprint X] [-VerifyOnly]` checks that the certificate exists and has not expired, writes the ignored overlay `crates/desktop/tauri.signing.conf.json` (sha256, DigiCert timestamp), builds through `npm run build -- --config tauri.signing.conf.json`, then runs `signtool verify /pa` on every `.exe` and `.msi`. It never handles a private key.
- `make-installer-art.ps1` regenerates the 24-bit WiX/NSIS BMPs in `crates/desktop/installer/` from `icons/zest-icon-512.png`.

## Verify
- `npm run output-policy:test`, `npm run features:test`, `npm run ui:lint:plugins` (RuleTester cases in `.oxlint-plugins/test.mjs`).
- `npm run features:check`; look things up with `npm run features -- where <path>`.

## Pitfalls
- Local `verify` runs only `--lib` Rust tests; integration tests under `crates/*/tests/` run only in the release gate (`--all-targets`).
- Binding drift is checked only in release-verify; regenerate with `cargo test -p zest-desktop --features export-bindings --lib export_bindings`.
- Windows library tests embed the app's Common Controls dependency too,
  so tests that drive the real turn and compaction handlers can start normally.
  A shared resource is linked into every artifact on both MSVC and GNU targets;
  Tauri's separate binary resource still supplies icons and version metadata.
- In `release-verify.sh`, every step goes through `step`, which captures the exit code with `set +e`; an `if cmd; then` wrapper once let a failing `cargo test` pass.
- Playwright reuses a dev server already on its port (`ZEST_UI_PORT`, default 1420) outside CI, but only one from the same checkout.
- PR runs do not save Rust caches, so a PR cannot seed anything for the next one; the default branch's own push run does. A cache saved from `refs/pull/N/merge` is readable only by that PR, yet each Rust cache is 0.5 to 1.7 GB against a 10 GB repository quota that evicts least recently used first. Before `save-if`, one burst of PR runs on 2 Nov uploaded about 14 GB of such copies in 22 minutes, and master's freshly seeded Linux caches were gone when the next master run started, so it rebuilt from scratch. Do not re-enable PR saves to speed up one PR.
- A run is warm only on an exact `Cache cargo` hit; read the step log: `No cache found.` is cold (Linux about 29 min, Windows about 46 min), `Restored from cache key "..." full match: false` is a fallback restore (only changed crates rebuild), `full match: true` followed by `Cache up-to-date.` in the post step is warm (Linux about 9 min, Windows about 12 min). Cold starts still happen when: the key changes (the part before the lockfile hash covers the OS, the job and every rustc version installed on the runner, so a runner-image Rust bump such as 1.99.0 to 1.99.1 misses even the fallback prefix; a `Cargo.lock`, `Cargo.toml` or `rust-toolchain.toml` change only changes the last part); a PR starts before master's push run has finished seeding after a `Cargo.lock` change; the entry sat unused for 7 days; or the seeding run failed, because a failed job saves nothing (`CACHE_ON_FAILURE` is off), including a RustSec or npm audit failure at the end of the run. A PR that changes `Cargo.lock` restores master's cache by prefix and rebuilds the changed crates on every push, since it cannot save its own.
- Caches already saved under `refs/pull/*` stay until evicted. List them with `gh api --paginate repos/<owner>/<repo>/actions/caches --jq '.actions_caches[] | select(.ref|startswith("refs/pull/")) | [.id,.size_in_bytes,.ref]'` and delete only those ids; `gh cache delete --all` would also delete master's seed. Measure CI with `gh api repos/<owner>/<repo>/actions/runs/<id>/jobs` (`created_at`, `started_at`, `completed_at`, per-step times); machine wait is only 2 to 22 s (median 3 s), so the time is in the job, not the queue.
- The npm cache from `actions/setup-node` (about 100 MB) still saves from PRs that change `package-lock.json`: the action has no `save-if`, and the volume does not justify replacing it with separate restore and save steps.
- Stale comments: the `release-verify.ps1` header mentions sidecar fetching (there is no sidecar step), and `build-signed.ps1` mentions a `postbuild` hook that `crates/desktop/package.json` does not define.
