$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$pluginSource = Join-Path $workspaceRoot "crates\plugins\now-playing"
$localAppData = [Environment]::GetFolderPath("LocalApplicationData")
if ([string]::IsNullOrWhiteSpace($localAppData)) {
    throw "Windows could not find the local app folder."
}

$pluginTarget = Join-Path $localAppData "Zest\plugins\now-playing"

Push-Location $workspaceRoot
try {
    cargo build -p zest-now-playing-plugin --release
    New-Item -ItemType Directory -Path $pluginTarget -Force | Out-Null
    Copy-Item (Join-Path $workspaceRoot "target\release\zest-now-playing.exe") `
        (Join-Path $pluginTarget "zest-now-playing.exe") -Force
    Copy-Item (Join-Path $pluginSource "plugin.json") `
        (Join-Path $pluginTarget "plugin.json") -Force
    Write-Host "Now Playing is installed. In Zest, open Settings > Extras, press Refresh, then turn it on."
} finally {
    Pop-Location
}
