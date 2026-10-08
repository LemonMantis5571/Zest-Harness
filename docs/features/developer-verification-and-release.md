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
- `npm run verify` (`scripts/dev-verify.mjs`) runs these steps in order and stops at the first failure: output-policy tests, output policy, feature-map tests, feature-map check, control CLI tests, UI unit tests, UI lint, lint-plugin tests, UI build, Playwright e2e (`npm run ui:e2e`), the control CLI smoke test (`zest-control.mjs check`, see [ui-control-cli](ui-control-cli.md)), `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace --lib`, `git diff --check`.
- `scripts/release-verify.ps1` and `release-verify.sh` are the CI and release gate. They run the same checks in the same order (toolchain version print or warning; output policy tests and check; feature map tests and check; control CLI tests; `npm ci`; ts-rs binding drift; UI test, lint, plugin-rule tests, build; Playwright e2e with `npx playwright install chromium`, plus `--with-deps` on Linux CI; the control CLI smoke test; fmt; clippy; `cargo test --workspace --all-targets`; `npm audit --omit=dev` with 3 tries; `cargo audit` or `cargo deny`; `git diff --check` for unstaged and staged changes).
- CI (`windows-verify.yml`, `linux-verify.yml`) runs the release gate on push to main/master and on PRs, first checks every commit in a PR with `check-output-artifacts.mjs --range`, and uploads `crates/desktop/ui/test-results/` as `playwright-traces-windows` / `playwright-traces-linux` on failure. Linux also builds and tests `zest serve` and the JSONL run protocol without WebKit, using local provider and worker fixtures.
- Output policy: under `outputs/`, image/audio/video files (by extension, any case) and files over 1 MiB are rejected unless their exact path is in `allowedOutputArtifacts` (empty by default). Modes: `--tracked` (default, HEAD), `--staged`, `--range <base> <head>` (full SHAs).
- Feature map: `feature-map.mjs check` fails when a doc path matches no file, a `.rs/.ts/.tsx/.mjs/.js/.ps1/.sh` file under `crates/` or `scripts/` has no owner (`lib/generated/` is exempt), or the README index is stale. `check --staged` also prints, without failing, features whose code changed while their doc did not. Other commands: `where`, `show`, `list`, `index`, and `--json`.
- `npm run hooks:install` writes a pre-commit wrapper that runs `.githooks/pre-commit` (output policy `--staged`, feature map `--staged`). It refuses to overwrite an existing hook or override a set `core.hooksPath`.
- UI lint (`oxlint --deny-warnings` plus the `zest` JS plugin), all errors: `no-unvalidated-persisted-json` (parsed JSON stays `unknown`/`JsonValue` until a type guard checks it), `no-secret-persistence-or-sink` (no token/secret/password-like names in `localStorage`/`sessionStorage` or `console`), `require-safe-html-provenance` (`dangerouslySetInnerHTML` only through `markTrustedHtml(...)`), `no-unowned-background-rejection` (no empty or constant `.catch`), `no-object-url-leak` (every `URL.createObjectURL` revoked in the module).
- UI lint also runs six rules vendored from [anti-slop](https://github.com/dmmulroy/anti-slop) (`.oxlint-plugins/anti-slop.mjs`, MIT, ported from TypeScript to plain ESM), all errors: `no-chained-type-assertions` (no `as unknown as T`; type the source or parse at the boundary), `no-widen-then-assert` (no widening a known value to `unknown`/`object` and asserting it back), `no-conditional-empty-object-spread` (build the object, then add optional fields), `no-array-filter-map` (one `flatMap` instead of adjacent `filter`/`map` passes), `no-reduce-accumulator-copy` and the built-in `oxc/no-accumulating-spread` (no copying a reducer accumulator per step), and `no-module-mocking` (no `vi.mock`/`jest.mock`). The upstream rule tests run in `anti-slop.test.mjs`. anti-slop is meant to be vendored and edited, so adjust the rules in place; its noisier rules (`no-runtime-typeof`, `no-unknown-*`, `require-safety-comment-for-type-assertion`) are deliberately not enabled.
- `npm run desktop:smoke` is opt-in (`ZEST_DESKTOP_SMOKE=1`): it starts `target/debug/zest-desktop` (or `ZEST_DESKTOP_PROFILE=release`, or `ZEST_DESKTOP_BINARY`), expects it alive after 4 s and responding on Windows, then kills it. It skips without the variable or binary and is in no gate.

## Release
- Tags `v*` drive `.github/workflows/release.yml` (see `docs/RELEASING.md`). `check-release-version.mjs <tag>` requires that the tag, `Cargo.toml` `version`, and `crates/desktop/tauri.conf.json` `version` match semver.
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
- Playwright reuses a dev server already on port 1420 outside CI.
- Stale comments: the `release-verify.ps1` header mentions sidecar fetching (there is no sidecar step), and `build-signed.ps1` mentions a `postbuild` hook that `crates/desktop/package.json` does not define.
