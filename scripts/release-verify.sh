#!/usr/bin/env bash
# Release/CI verification gate. Same checks as release-verify.ps1, for shells
# that are not PowerShell. Requires cargo-audit (or cargo-deny) on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

step() {
  local name="$1"
  shift
  echo
  echo "==> $name"
  # `if cmd; then` is a success even when cmd fails, so `$?` after `fi` is 0.
  # That is what let `cargo test` fail and the gate still exit 0.
  set +e
  "$@"
  local code=$?
  set -e
  if [ "$code" -eq 0 ]; then
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      echo "- [x] **$name**: PASSED" >> "$GITHUB_STEP_SUMMARY"
    fi
    return 0
  fi
  echo "==> [ERROR] in step '$name' (exit $code)" >&2
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "- [ ] **$name**: FAILED (exit code $code)" >> "$GITHUB_STEP_SUMMARY"
  fi
  exit "$code"
}

check_toolchain() {
  rustc --version
  node --version
}


check_audit() {
  if command -v cargo-audit >/dev/null 2>&1; then
    cargo audit
  elif command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check advisories
  else
    echo "cargo-audit (or cargo-deny) is required for the RustSec gate. Install: cargo install cargo-audit --locked" >&2
    return 1
  fi
}

check_whitespace() {
  git diff --check --ignore-space-at-eol
  git diff --cached --check --ignore-space-at-eol
}

step "toolchain check" check_toolchain
step "output artifact policy tests" node --test ./scripts/check-output-artifacts.test.mjs
step "output artifact policy" node ./scripts/check-output-artifacts.mjs
step "feature map tests" node --test ./scripts/feature-map.test.mjs
step "feature map" node ./scripts/feature-map.mjs check
step "cargo fmt --check" cargo fmt --all -- --check
step "cargo clippy (strict)" cargo clippy --workspace --all-targets -- -D warnings
step "cargo test" cargo test --workspace --all-targets
step "RustSec (cargo audit)" check_audit
step "git diff --check" check_whitespace

echo
echo "release-verify.sh passed"
echo "Live doctor is opt-in: cargo run -p zest -- doctor --live"
echo "(requires provider credentials; do not fake success without them)"
