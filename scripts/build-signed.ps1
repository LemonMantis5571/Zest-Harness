#Requires -Version 5.1
# Build Zest's Windows installers with Authenticode signatures.
#
# Signing has to happen DURING the bundle, not after it. Signing only the
# finished .exe/.msi would leave the zest-desktop.exe inside them unsigned, and
# that inner binary is what Windows judges once the installer has run. Tauri
# signs the app, the sidecar and both installers when the certificate is
# configured, so this drives the real build rather than post-processing it.
#
# The thumbprint is a public fingerprint, not a secret. This script never asks
# for, reads, or passes a private key or password: signtool talks to the token
# or cloud HSM itself and prompts you directly if it needs to.
#
#   .\scripts\build-signed.ps1 -Thumbprint A1B2C3...
#   .\scripts\build-signed.ps1            # reads tauri.signing.conf.json
#   .\scripts\build-signed.ps1 -VerifyOnly
#
# ASCII only: Windows PowerShell 5.1 reads a BOM-less UTF-8 script as ANSI.

[CmdletBinding()]
param(
    # SHA1 thumbprint of a code signing certificate in your Windows cert store.
    [string]$Thumbprint,
    # Skip the build; just check what is already in target/release/bundle.
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root "crates\desktop"
$overlay = Join-Path $desktop "tauri.signing.conf.json"
$bundleDir = Join-Path $root "target\release\bundle"

function Get-SignTool {
    $kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (-not (Test-Path -LiteralPath $kits)) {
        throw "Windows SDK not found. Install the SDK to get signtool.exe."
    }
    $found = Get-ChildItem -Path $kits -Recurse -Filter "signtool.exe" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "\\x64\\" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if (-not $found) { throw "signtool.exe not found under $kits" }
    return $found.FullName
}

function Get-Artifacts {
    $paths = @(
        (Join-Path $bundleDir "nsis"),
        (Join-Path $bundleDir "msi")
    )
    $items = @()
    foreach ($p in $paths) {
        if (Test-Path -LiteralPath $p) {
            $items += Get-ChildItem -LiteralPath $p -File | Where-Object { $_.Extension -in ".exe", ".msi" }
        }
    }
    return $items
}

$signtool = Get-SignTool

if (-not $VerifyOnly) {
    # Resolve the thumbprint: the argument wins, then the local overlay.
    if (-not $Thumbprint) {
        if (-not (Test-Path -LiteralPath $overlay)) {
            Write-Host "No signing configuration found." -ForegroundColor Yellow
            Write-Host "  1. Copy crates\desktop\tauri.signing.conf.json.example to tauri.signing.conf.json"
            Write-Host "  2. Put your certificate thumbprint in it"
            Write-Host "  ...or pass -Thumbprint <value>."
            Write-Host ""
            Write-Host "Without a certificate, build unsigned with: npx tauri build"
            exit 1
        }
        $conf = Get-Content -LiteralPath $overlay -Raw | ConvertFrom-Json
        $Thumbprint = [string]$conf.bundle.windows.certificateThumbprint
    }

    $Thumbprint = ($Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
    if ($Thumbprint -notmatch '^[0-9A-F]{40}$') {
        throw "That is not a SHA1 certificate thumbprint (40 hex characters)."
    }

    # Fail before a long build rather than after it.
    $cert = Get-ChildItem -Path Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
        Where-Object { $_.Thumbprint -eq $Thumbprint } |
        Select-Object -First 1
    if (-not $cert) {
        throw "No certificate with thumbprint $Thumbprint is in your certificate store. Plug in the token, or import the cert."
    }
    if ($cert.NotAfter -lt (Get-Date)) {
        throw "That certificate expired on $($cert.NotAfter.ToString('yyyy-MM-dd'))."
    }
    Write-Host "Signing as: $($cert.Subject)"
    Write-Host "Expires   : $($cert.NotAfter.ToString('yyyy-MM-dd'))"

    # Write the overlay Tauri will merge over tauri.conf.json.
    $payload = [ordered]@{
        '$schema' = "https://schema.tauri.app/config/2"
        bundle    = [ordered]@{
            windows = [ordered]@{
                certificateThumbprint = $Thumbprint
                digestAlgorithm       = "sha256"
                # Without a timestamp the signature dies with the certificate;
                # with one it stays valid for the life of the timestamp.
                timestampUrl          = "http://timestamp.digicert.com"
            }
        }
    }
    $payload | ConvertTo-Json -Depth 6 | Out-File -LiteralPath $overlay -Encoding utf8

    # One entry point, on purpose.
    #
    # This used to build the UI here and then call `npx tauri build`, which runs
    # `beforeBuildCommand` and builds the same 4,400 modules a second time.
    # Going through the package script instead also restores the `postbuild`
    # hook — `npx tauri build` skips it, so signed releases were the one build
    # that never ran the bundle verifier.
    Write-Host "`nBuilding signed bundles (UI included) ..."
    Push-Location $desktop
    try {
        & npm run build -- --config tauri.signing.conf.json
        if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }
    } finally {
        Pop-Location
    }
}

$artifacts = Get-Artifacts
if (-not $artifacts) { throw "No installers found under $bundleDir" }

Write-Host "`n--- Signature check ---"
$unsigned = @()
foreach ($item in $artifacts) {
    # /pa uses the Authenticode policy, which is what Windows itself applies.
    # signtool prints a header even under /q, so discard its output entirely and
    # read only the exit code.
    & $signtool verify /pa /q $item.FullName *>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host ("  SIGNED   {0}" -f $item.Name) -ForegroundColor Green
    } else {
        Write-Host ("  UNSIGNED {0}" -f $item.Name) -ForegroundColor Yellow
        $unsigned += $item.Name
    }
}

if ($unsigned.Count -gt 0 -and -not $VerifyOnly) {
    throw "Build finished but these are unsigned: $($unsigned -join ', ')"
}

# Do not leak signtool's exit code as this script's own.
exit 0
