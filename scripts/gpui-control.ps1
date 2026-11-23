# Windows UI Automation driver for scripts/gpui-control.mjs. It reads and drives the native
# GPUI window through accessibility patterns only, never simulated keystrokes, so nothing can
# land in another window. Prints one JSON object.
param(
    [Parameter(Mandatory)][string]$Command,
    [Parameter(Mandatory)][int]$AppPid,
    [string]$Name = '',
    [string]$Text = '',
    [string]$Out = '',
    [int]$Nth = 0,
    [int]$TimeoutSeconds = 60
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class ZestWin {
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int s);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  public struct RECT { public int L, T, R, B; }
}
'@
[ZestWin]::SetProcessDPIAware() | Out-Null
$A = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]

function Get-Root {
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        $h = (Get-Process -Id $AppPid -ErrorAction Stop).MainWindowHandle
        if ($h -ne 0) { return $A::FromHandle($h) }
        Start-Sleep -Milliseconds 200
    }
    throw 'The Zest window did not appear.'
}

function Get-All($root) {
    # AccessKit builds its tree on the first client query.
    $root.FindAll($Scope::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
}

function Describe($el) {
    $c = $el.Current
    $node = [ordered]@{ role = $c.ControlType.ProgrammaticName -replace '^ControlType\.', ''; name = $c.Name }
    $patterns = $el.GetSupportedPatterns()
    foreach ($p in $patterns) {
        switch ($p.ProgrammaticName) {
            'ValuePatternIdentifiers.Pattern' { $node.value = $el.GetCurrentPattern($p).Current.Value }
            'TogglePatternIdentifiers.Pattern' { $node.toggled = "$($el.GetCurrentPattern($p).Current.ToggleState)" }
            'SelectionItemPatternIdentifiers.Pattern' { $node.selected = $el.GetCurrentPattern($p).Current.IsSelected }
            'ExpandCollapsePatternIdentifiers.Pattern' { $node.expanded = "$($el.GetCurrentPattern($p).Current.ExpandCollapseState)" }
        }
    }
    if (-not $c.IsEnabled) { $node.disabled = $true }
    $node
}

function Find-Named($root, [string]$name, [int]$nth) {
    $deadline = (Get-Date).AddSeconds(5)
    while ((Get-Date) -lt $deadline) {
        $matches = @(Get-All $root | Where-Object { $_.Current.Name -eq $name })
        if ($matches.Count -gt $nth) { return $matches[$nth] }
        Start-Sleep -Milliseconds 250
    }
    $buttons = @(Get-All $root | Where-Object { $_.Current.ControlType.ProgrammaticName -match 'Button|TabItem|RadioButton|CheckBox' } |
        ForEach-Object { $_.Current.Name } | Where-Object { $_ } | Select-Object -Unique)
    throw "No element named '$name'. Controls on screen: $($buttons -join ', ')"
}

function Activate($el) {
    $patterns = $el.GetSupportedPatterns().ProgrammaticName
    if ($patterns -contains 'InvokePatternIdentifiers.Pattern') {
        $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); return 'invoke'
    }
    if ($patterns -contains 'SelectionItemPatternIdentifiers.Pattern') {
        $el.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select(); return 'select'
    }
    if ($patterns -contains 'TogglePatternIdentifiers.Pattern') {
        $el.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle(); return 'toggle'
    }
    throw "'$($el.Current.Name)' cannot be activated through UI Automation."
}

function Get-Composer($root) {
    $edit = Get-All $root | Where-Object {
        $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Edit' -and ($_.Current.Name -eq 'Message' -or $_.Current.Name -like 'Ask about this project*')
    } | Select-Object -First 1
    if (-not $edit) { throw 'The message box is not on screen. Open a chat first.' }
    $edit
}

function Get-State($root) {
    $all = @(Get-All $root)
    $rows = @($all | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.ListItem' } | ForEach-Object { $_.Current.Name })
    $names = @($all | ForEach-Object { $_.Current.Name })
    $composer = $all | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Edit' -and $_.Current.Name -eq 'Message' } | Select-Object -First 1
    [ordered]@{
        turnRunning = $names -contains 'Stop response'
        needsInput = ($names -contains 'Allow once') -or ($names -contains 'Dismiss question')
        messageCount = $rows.Count
        lastMessage = if ($rows.Count) { $rows[-1] } else { $null }
        composer = if ($composer) { $composer.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value } else { $null }
        tabs = @($all | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.TabItem' } | ForEach-Object { $_.Current.Name })
    }
}

$root = Get-Root
$null = Get-All $root
$result = [ordered]@{ ok = $true; command = $Command }
switch ($Command) {
    'snapshot' {
        $result.nodes = @(Get-All $root | ForEach-Object { Describe $_ } | Where-Object { $_.name })
    }
    'inspect' { $result.state = Get-State $root }
    'messages' {
        $result.messages = @(Get-All $root | Where-Object { $_.Current.ControlType.ProgrammaticName -eq 'ControlType.ListItem' } | ForEach-Object { $_.Current.Name })
    }
    'click' { $result.action = Activate (Find-Named $root $Name $Nth); Start-Sleep -Milliseconds 400; $result.state = Get-State $root }
    'type' {
        # With -Name, type into that field (matched by accessible name) instead of the message box.
        $field = if ($Name) { Find-Named $root $Name $Nth } else { Get-Composer $root }
        $field.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Text)
        $result.state = Get-State $root
    }
    'send' {
        $before = (Get-State $root).messageCount
        (Get-Composer $root).GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Text)
        Start-Sleep -Milliseconds 300
        $null = Activate (Find-Named $root 'Send' 0)
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        do {
            Start-Sleep -Milliseconds 400
            $state = Get-State $root
            $settled = -not $state.turnRunning -and $state.messageCount -gt $before
        } until ($settled -or $state.needsInput -or (Get-Date) -gt $deadline)
        $result.state = $state
        if (-not $settled -and -not $state.needsInput) { $result.ok = $false; $result.error = "The turn did not finish within $TimeoutSeconds s." }
    }
    'wait-settle' {
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        do { Start-Sleep -Milliseconds 400; $state = Get-State $root } until (-not $state.turnRunning -or (Get-Date) -gt $deadline)
        $result.state = $state
        if ($state.turnRunning) { $result.ok = $false; $result.error = "Still working after $TimeoutSeconds s." }
    }
    'screenshot' {
        $h = (Get-Process -Id $AppPid).MainWindowHandle
        $r = New-Object ZestWin+RECT
        [ZestWin]::DwmGetWindowAttribute($h, 9, [ref]$r, 16) | Out-Null
        $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
        # PrintWindow renders Zest's own window, so other windows on top of it never appear in
        # the image. PW_RENDERFULLCONTENT (2) includes GPU-drawn content.
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $dc = $g.GetHdc()
        $ok = [ZestWin]::PrintWindow($h, $dc, 2)
        $g.ReleaseHdc($dc)
        $g.Dispose()
        if (-not $ok) { throw 'PrintWindow could not render the Zest window.' }
        $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
        $result.file = $Out
    }
    default { throw "Unknown command: $Command" }
}
$result | ConvertTo-Json -Depth 6 -Compress
