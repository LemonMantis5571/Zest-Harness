#Requires -Version 5.1
# Release/CI verification gate for the root Cargo workspace, including audits
# that are not part of local dev. The desktop app is checked by gpui-windows.yml.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Step($name, $scriptBlock) {
  Write-Host ""
  Write-Host "==> $name" -ForegroundColor Cyan
  $global:LASTEXITCODE = 0
  try {
    & $scriptBlock
    if ($env:GITHUB_STEP_SUMMARY) {
      Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value "- [x] **$name**: PASSED"
    }
  } catch {
    Write-Host "==> [ERROR] in step '$name': $_" -ForegroundColor Red
    if ($env:GITHUB_STEP_SUMMARY) {
      Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value "- [ ] **$name**: FAILED`n  - Error: $_"
    }
    throw
  }
  if ($LASTEXITCODE -ne 0 -and $null -ne $LASTEXITCODE) {
    if ($env:GITHUB_STEP_SUMMARY) {
      Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value "- [ ] **$name**: FAILED (exit code $LASTEXITCODE)"
    }
    throw "Step failed: $name (exit $LASTEXITCODE)"
  }
}

$env:CARGO_TARGET_DIR = Join-Path $Root "target"

Step "toolchain check" {
  $rustc = (rustc --version)
  if ($rustc -notmatch "1\.97\.1") {
    Write-Warning "Expected rustc 1.97.1, got: $rustc"
  }
  $node = (node --version)
  if ($node -ne "v24.16.0") {
    Write-Warning "Expected node v24.16.0, got: $node"
  }
}

Step "output artifact policy tests" {
  node --test ./scripts/check-output-artifacts.test.mjs
}

Step "output artifact policy" {
  node ./scripts/check-output-artifacts.mjs
}

Step "feature map tests" {
  node --test ./scripts/feature-map.test.mjs
}

Step "feature map" {
  node ./scripts/feature-map.mjs check
}

Step "cargo fmt --check" {
  cargo fmt --all -- --check
}

Step "cargo clippy (strict)" {
  cargo clippy --workspace --all-targets -- -D warnings
}

Step "cargo test" {
  cargo test --workspace --all-targets
}

Step "RustSec (cargo audit)" {
  if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
    cargo audit
  } elseif (Get-Command cargo-deny -ErrorAction SilentlyContinue) {
    cargo deny check advisories
  } else {
    throw "cargo-audit (or cargo-deny) is required for the RustSec gate. Install: cargo install cargo-audit --locked"
  }
}

Step "git diff --check" {
  git diff --check --ignore-space-at-eol
  if ($LASTEXITCODE -ne 0) { throw "Unstaged whitespace errors found." }
  git diff --cached --check --ignore-space-at-eol
  if ($LASTEXITCODE -ne 0) { throw "Staged whitespace errors found." }
}

Write-Host ""
Write-Host "release-verify.ps1 passed" -ForegroundColor Green
Write-Host "Live doctor is opt-in: cargo run -p zest -- doctor --live"
Write-Host "(requires provider credentials; do not fake success without them)"
