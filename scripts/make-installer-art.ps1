#Requires -Version 5.1
# Generate the installer artwork from the app icon.
#
# The MSI was shipping WiX's default banner - the red "no entry" placeholder in
# the top-right of every page - because Tauri does not override it unless you
# supply one. NSIS has the same story with its header and sidebar.
#
# Generated rather than hand-drawn so the art tracks the icon: change
# icons/zest-icon-512.png, re-run this, rebuild.
#
# Sizes are fixed by the installers, not by us:
#   WiX  banner  493 x  58   text is drawn over the LEFT, so the mark goes right
#   WiX  dialog  493 x 312   text is drawn over the RIGHT, so the mark goes left
#   NSIS header  150 x  57
#   NSIS sidebar 164 x 314
#
# WiX paints its own labels - "Installing Zest", the welcome paragraph - in BLACK
# directly on top of these two bitmaps, and that colour cannot be changed without
# replacing the whole .wxs UI template. So the WiX art keeps a light field
# wherever text lands and confines the dark brand panel to the rest. NSIS has no
# such problem: it draws its text on its own white chrome beside the image, so
# those two can be dark throughout.
#
# Written as 24-bit BMP with no alpha. WiX and NSIS both predate sensible PNG
# support, and a 32-bit BMP with an alpha channel renders as a black box in the
# NSIS header on some Windows builds.
#
# ASCII only: Windows PowerShell 5.1 reads a BOM-less UTF-8 script as ANSI.

[CmdletBinding()]
param(
    [string]$Source,
    [string]$OutDir
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
if (-not $Source) { $Source = Join-Path $root "crates\desktop\icons\zest-icon-512.png" }
if (-not $OutDir) { $OutDir = Join-Path $root "crates\desktop\installer" }

if (-not (Test-Path -LiteralPath $Source)) { throw "Source icon not found: $Source" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# The app's own background, so the installer looks like the thing being installed.
$bg = [System.Drawing.Color]::FromArgb(12, 12, 14)
$fg = [System.Drawing.Color]::FromArgb(245, 245, 247)
$muted = [System.Drawing.Color]::FromArgb(150, 150, 160)
# White, not off-white: WiX composites its own dialog chrome against pure white,
# and anything near-but-not-quite leaves a visible seam.
$light = [System.Drawing.Color]::White
$mutedOnLight = [System.Drawing.Color]::FromArgb(96, 96, 104)

$icon = [System.Drawing.Image]::FromFile($Source)

function New-Art {
    param(
        [int]$Width,
        [int]$Height,
        [int]$MarkSize,
        [int]$MarkX,
        [int]$MarkY,
        [string]$Title,
        [string]$Subtitle,
        [int]$TextX,
        [int]$TextY,
        [int]$TitleSize,
        # Width of the dark brand panel measured from the left edge. The rest is
        # left light for the installer's own black text. 0 means all light,
        # $Width means all dark.
        [int]$PanelWidth,
        [string]$Path
    )

    $bmp = New-Object System.Drawing.Bitmap($Width, $Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit

        $g.Clear($light)
        if ($PanelWidth -gt 0) {
            $panel = New-Object System.Drawing.SolidBrush($bg)
            $g.FillRectangle($panel, 0, 0, $PanelWidth, $Height)
            $panel.Dispose()
        }

        $g.DrawImage($icon, $MarkX, $MarkY, $MarkSize, $MarkSize)

        # Any text we draw ourselves sits inside the dark panel, so it is light.
        $onPanel = ($TextX -lt $PanelWidth)
        if ($Title) {
            $titleFont = New-Object System.Drawing.Font("Segoe UI", $TitleSize, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
            $brush = New-Object System.Drawing.SolidBrush($(if ($onPanel) { $fg } else { $bg }))
            $g.DrawString($Title, $titleFont, $brush, $TextX, $TextY)
            $titleFont.Dispose(); $brush.Dispose()
        }
        if ($Subtitle) {
            $subFont = New-Object System.Drawing.Font("Segoe UI", 12, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
            $brush = New-Object System.Drawing.SolidBrush($(if ($onPanel) { $muted } else { $mutedOnLight }))
            $g.DrawString($Subtitle, $subFont, $brush, $TextX, ($TextY + $TitleSize + 8))
            $subFont.Dispose(); $brush.Dispose()
        }
    } finally {
        $g.Dispose()
    }

    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $bmp.Dispose()
    Write-Host ("  {0}  ({1}x{2})" -f (Split-Path -Leaf $Path), $Width, $Height)
}

Write-Host "Writing installer art to $OutDir"

# WiX banner: "Installing Zest" is printed over the left in black, so that side
# stays white. The mark goes hard right, exactly where the placeholder was.
New-Art -Width 493 -Height 58 -MarkSize 40 -MarkX 441 -MarkY 9 `
    -Title "" -Subtitle "" -TextX 0 -TextY 0 -TitleSize 0 -PanelWidth 0 `
    -Path (Join-Path $OutDir "wix-banner.bmp")

# WiX dialog: the welcome and exit paragraphs are drawn in black from about
# x=135 rightward, so the dark panel stops short of that.
New-Art -Width 493 -Height 312 -MarkSize 76 -MarkX 24 -MarkY 92 `
    -Title "Zest" -Subtitle "Coding harness" -TextX 24 -TextY 186 -TitleSize 26 -PanelWidth 128 `
    -Path (Join-Path $OutDir "wix-dialog.bmp")

# NSIS draws its text on its own chrome beside these, never over them, so both
# can carry the dark panel edge to edge.
New-Art -Width 150 -Height 57 -MarkSize 40 -MarkX 96 -MarkY 8 `
    -Title "" -Subtitle "" -TextX 0 -TextY 0 -TitleSize 0 -PanelWidth 150 `
    -Path (Join-Path $OutDir "nsis-header.bmp")

New-Art -Width 164 -Height 314 -MarkSize 88 -MarkX 38 -MarkY 88 `
    -Title "Zest" -Subtitle "" -TextX 40 -TextY 192 -TitleSize 26 -PanelWidth 164 `
    -Path (Join-Path $OutDir "nsis-sidebar.bmp")

$icon.Dispose()
Write-Host "Done. Rebuild with: npm run desktop:build"
