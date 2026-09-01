#!/usr/bin/env bash
# Release/CI verification gate. Same checks as release-verify.ps1, for shells
# that are not PowerShell. Requires cargo-audit (or cargo-deny) on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BINDING_DIR="crates/desktop/ui/src/lib/generated"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

step() {
  local name="$1"
  shift
  echo
  echo "==> $name"
  if "$@"; then
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      echo "- [x] **$name**: PASSED" >> "$GITHUB_STEP_SUMMARY"
    fi
    return 0
  fi
  local code=$?
  echo "==> [ERROR] in step '$name' (exit $code)" >&2
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    echo "- [ ] **$name**: FAILED (exit code $code)" >> "$GITHUB_STEP_SUMMARY"
  fi
  exit "$code"
}

normalize_binding() {
  python3 - "$1" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
text = text.replace("\r\n", "\n")
text = re.sub(r"[ \t]+$", "", text, flags=re.M)
if not text.endswith("\n"):
    text += "\n"
sys.stdout.write(text)
PY
}

check_toolchain() {
  rustc --version
  node --version
}

check_bindings() {
  local snapshot
  snapshot="$(mktemp -d)"
  cleanup() {
    rm -rf "$BINDING_DIR"
    mkdir -p "$BINDING_DIR"
    if [ -d "$snapshot" ]; then
      cp -a "$snapshot"/. "$BINDING_DIR"/ 2>/dev/null || true
      rm -rf "$snapshot"
    fi
  }
  trap cleanup EXIT
  mkdir -p "$BINDING_DIR"
  cp -a "$BINDING_DIR"/. "$snapshot"/ 2>/dev/null || true
  cargo test -p zest-desktop --features export-bindings --lib export_bindings
  local drifted=0 name before after
  local names
  names="$( {
    find "$snapshot" -maxdepth 1 -type f -name "*.ts" -printf "%f\n" 2>/dev/null || true
    find "$BINDING_DIR" -maxdepth 1 -type f -name "*.ts" -printf "%f\n" 2>/dev/null || true
  } | sort -u )"
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    before="$snapshot/$name"
    after="$BINDING_DIR/$name"
    if [ ! -f "$before" ] || [ ! -f "$after" ]; then
      echo "Binding drift detected in: $name" >&2
      drifted=1
      continue
    fi
    if [ "$(normalize_binding "$before")" != "$(normalize_binding "$after")" ]; then
      echo "Binding drift detected in: $name" >&2
      drifted=1
    fi
  done <<< "$names"
  if [ "$drifted" -ne 0 ]; then
    echo "Generated bindings are stale. Commit the regenerated files." >&2
    return 1
  fi
  trap - EXIT
  cleanup
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
step "npm ci" npm ci --no-fund --no-audit
step "binding drift (ts-rs)" check_bindings
step "ui test" npm run ui:test
step "ui lint (strict)" npm run ui:lint
step "ui plugin lint rules" npm run ui:lint:plugins
step "ui build" npm run ui:build
step "cargo fmt --check" cargo fmt --all -- --check
step "cargo clippy (strict)" cargo clippy --workspace --all-targets -- -D warnings
step "cargo test" cargo test --workspace --all-targets
step "npm audit" npm audit --omit=dev
step "RustSec (cargo audit)" check_audit
step "git diff --check" check_whitespace

echo
echo "release-verify.sh passed"
echo "Live doctor is opt-in: cargo run -p zest -- doctor --live"
echo "(requires provider credentials; do not fake success without them)"
