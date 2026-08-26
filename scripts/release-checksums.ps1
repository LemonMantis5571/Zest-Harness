#Requires -Version 5.1
# Emit SHA256 checksums for the built Windows installers.
#
# Until the installers are signed, this is what lets someone verify that the
# file they downloaded is the file you built. It is not a substitute for a
# signature - it proves integrity, not identity, and only if they get the
# checksum from somewhere you control rather than from beside the download.
#
#   .\scripts\release-checksums.ps1
#   .\scripts\release-checksums.ps1 -OutFile SHA256SUMS.txt
#
# ASCII only: Windows PowerShell 5.1 reads a BOM-less UTF-8 script as ANSI.

[CmdletBinding()]
param(
    [string]$OutFile
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$bundleDir = Join-Path (Join-Path (Join-Path $root "target") "release") "bundle"

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) { throw "Node.js is required to create release checksums." }

$script = Join-Path $root "scripts\release-checksums.mjs"
$arguments = @($script, "--root", $bundleDir)
if ($OutFile) {
    $path = if ([System.IO.Path]::IsPathRooted($OutFile)) { $OutFile } else { Join-Path $root $OutFile }
    $arguments += @("--out", $path)
}

& $node.Source @arguments
if ($LASTEXITCODE -ne 0) { throw "Release checksum generation failed (exit $LASTEXITCODE)." }
