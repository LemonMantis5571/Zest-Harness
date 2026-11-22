# Makes WiX Toolset 3 and NSIS 3 available to `npm run gpui:package` on a Windows CI runner.
# Installs only what is missing, then puts WiX's bin folder on the job PATH.
$ErrorActionPreference = 'Stop'

function Find-Candle {
    $command = Get-Command candle.exe -ErrorAction SilentlyContinue
    if ($command) { return Split-Path $command.Source }
    $roots = @($env:WIX, ${env:ProgramFiles(x86)}, $env:ProgramFiles) | Where-Object { $_ }
    foreach ($root in $roots) {
        $found = Get-ChildItem -Path $root -Filter candle.exe -Recurse -Depth 3 -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { return $found.DirectoryName }
    }
    return $null
}

$wix = Find-Candle
if (-not $wix) {
    choco install wixtoolset --no-progress -y
    if ($LASTEXITCODE -ne 0) { throw 'Installing WiX Toolset 3 failed.' }
    $wix = Find-Candle
}
if (-not $wix) { throw 'WiX Toolset 3 (candle.exe) is not available.' }

$nsis = Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'
if (-not (Get-Command makensis.exe -ErrorAction SilentlyContinue) -and -not (Test-Path $nsis)) {
    choco install nsis --no-progress -y
    if ($LASTEXITCODE -ne 0) { throw 'Installing NSIS failed.' }
}

if ($env:GITHUB_PATH) { Add-Content -Path $env:GITHUB_PATH -Value $wix }
"WiX: $wix"
