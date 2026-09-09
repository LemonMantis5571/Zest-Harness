#Requires -Version 5.1
# Release/CI verification gate. This intentionally includes sidecar fetching,
# audits, and generated-binding drift checks that are not part of local dev.
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
$BindingDir = "crates/desktop/ui/src/lib/generated"

function Normalize-BindingText([string]$text) {
  $clean = $text -replace "`r`n", "`n"
  $clean = [regex]::Replace($clean, '(?m)[ \t]+$', '')
  if (-not $clean.EndsWith("`n")) { $clean += "`n" }
  return $clean
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
  $bindingRoot = Join-Path $Root $BindingDir
  $snapshotRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("zest-bindings-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Path $snapshotRoot -Force | Out-Null

  try {
    if (Test-Path -LiteralPath $bindingRoot) {
      foreach ($child in Get-ChildItem -LiteralPath $bindingRoot -Force) {
        Copy-Item -LiteralPath $child.FullName -Destination $snapshotRoot -Recurse -Force
      }
    }

    cargo test -p zest-desktop --features export-bindings --lib export_bindings

    $before = @{}
    foreach ($file in @(Get-ChildItem -LiteralPath $snapshotRoot -Filter *.ts -File)) {
      $before[$file.Name] = [System.IO.File]::ReadAllText($file.FullName)
    }
    $after = @{}
    foreach ($file in @(Get-ChildItem -LiteralPath $bindingRoot -Filter *.ts -File)) {
      $after[$file.Name] = [System.IO.File]::ReadAllText($file.FullName)
    }

    $drifted = @()
    $names = @($before.Keys) + @($after.Keys) | Sort-Object -Unique
    foreach ($name in $names) {
      if (-not $before.ContainsKey($name) -or -not $after.ContainsKey($name)) {
        $drifted += $name
        continue
      }
      if ((Normalize-BindingText $before[$name]) -ne (Normalize-BindingText $after[$name])) {
        $drifted += $name
      }
    }

    if ($drifted.Count -gt 0) {
      Write-Host "Binding drift detected in:" -ForegroundColor Red
      Write-Host ($drifted -join "`n")
      throw "Generated bindings are stale. Commit the regenerated files."
    }

  } finally {
    if (Test-Path -LiteralPath $bindingRoot) {
      foreach ($child in @(Get-ChildItem -LiteralPath $bindingRoot -Force)) {
        Remove-Item -LiteralPath $child.FullName -Recurse -Force
      }
    } else {
      New-Item -ItemType Directory -Path $bindingRoot -Force | Out-Null
    }

    foreach ($child in @(Get-ChildItem -LiteralPath $snapshotRoot -Force)) {
      Copy-Item -LiteralPath $child.FullName -Destination $bindingRoot -Recurse -Force
    }
    Remove-Item -LiteralPath $snapshotRoot -Recurse -Force
  }
}

Step "ui test" {
  npm run ui:test
}

Step "ui lint (strict)" {
  npm run ui:lint
}

Step "ui plugin lint rules" {
  npm run ui:lint:plugins
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
  $ok = $false
  foreach ($attempt in 1..3) {
    $global:LASTEXITCODE = 0
    npm audit --omit=dev
    if ($LASTEXITCODE -eq 0) {
      $ok = $true
      break
    }
    Write-Host "npm audit attempt $attempt failed (exit $LASTEXITCODE); retrying"
    Start-Sleep -Seconds 5
  }
  if (-not $ok) {
    throw "npm audit failed after 3 attempts"
  }
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
