#Requires -Version 5.1
# Release/CI verification gate. This intentionally includes sidecar fetching,
# audits, and generated-binding drift checks that are not part of local dev.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Step($name, $scriptBlock) {
  Write-Host ""
  Write-Host "==> $name" -ForegroundColor Cyan
  & $scriptBlock
  if ($LASTEXITCODE -ne 0 -and $null -ne $LASTEXITCODE) {
    throw "Step failed: $name (exit $LASTEXITCODE)"
  }
}

$env:CARGO_TARGET_DIR = Join-Path $Root "target"
$BindingDir = "crates/desktop/ui/src/lib/generated"

function Normalize-BindingWhitespace {
  $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
  $dir = Join-Path $Root $BindingDir
  foreach ($file in Get-ChildItem -Path $dir -Filter *.ts -File) {
    $text = [System.IO.File]::ReadAllText($file.FullName)
    $clean = $text -replace "`r`n", "`n"
    $clean = [regex]::Replace($clean, '(?m)[ \t]+$', '')
    if (-not $clean.EndsWith("`n")) { $clean += "`n" }
    if ($clean -ne $text) {
      [System.IO.File]::WriteAllText($file.FullName, $clean, $utf8NoBom)
    }
  }
}

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

Step "npm ci" {
  npm ci --no-fund --no-audit
}

Step "binding drift (ts-rs)" {
  cargo test -p zest-desktop --features export-bindings --lib export_bindings
  Normalize-BindingWhitespace
  git diff --exit-code -- $BindingDir
  if ($LASTEXITCODE -ne 0) { throw "Generated bindings are stale. Commit the regenerated files." }

  $untracked = git ls-files --others --exclude-standard -- $BindingDir
  if ($untracked) {
    throw "New generated bindings are not committed:`n$($untracked -join "`n")"
  }
}

Step "ui test" {
  npm run ui:test
}

Step "ui lint (strict)" {
  npm run ui:lint
}

Step "ui build" {
  npm run ui:build
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

Step "npm audit" {
  npm audit --omit=dev
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
