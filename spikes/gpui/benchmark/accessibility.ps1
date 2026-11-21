param([Parameter(Mandatory)][int]$AppProcessId, [switch]$Check)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$handle = (Get-Process -Id $AppProcessId).MainWindowHandle
if ($handle -eq 0) { throw 'The desktop window has no handle.' }
$window = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
$scope = [System.Windows.Automation.TreeScope]::Descendants
$condition = [System.Windows.Automation.Condition]::TrueCondition
# AccessKit and the WebView2 accessibility tree may initialize on the first client query.
$null = $window.FindAll($scope, $condition)
Start-Sleep -Seconds 2
$nodes = $window.FindAll($scope, $condition)
$buttons = 0
$namedButtons = 0
$message = $false
$textPattern = $false
$valuePattern = $false
$editRoundTrip = $false
$editMilliseconds = $null
$textRoundTrip = $false
$retried = $false
foreach ($node in $nodes) {
    $role = $node.Current.ControlType.ProgrammaticName
    $name = $node.Current.Name
    if ($role -eq 'ControlType.Button') {
        $buttons++
        if (-not [string]::IsNullOrWhiteSpace($name)) { $namedButtons++ }
    }
    $composer = $name -eq 'Message' -or $node.Current.AutomationId -eq 'zest-composer-input' -or $name -like 'Ask about this project*'
    if ($role -eq 'ControlType.Edit' -and $composer) {
        $message = $true
        $patterns = $node.GetSupportedPatterns().ProgrammaticName
        $textPattern = $patterns -contains 'TextPatternIdentifiers.Pattern'
        $valuePattern = $patterns -contains 'ValuePatternIdentifiers.Pattern'
        if ($valuePattern) {
            $value = $node.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            if (-not $value.Current.IsReadOnly) {
                $previous = $value.Current.Value
                try {
                    $timer = [Diagnostics.Stopwatch]::StartNew()
                    $value.SetValue('Zest benchmark input')
                    # A cold start can still be building its accessibility tree; retry once.
                    while ($timer.ElapsedMilliseconds -lt 3000 -and $value.Current.Value -ne 'Zest benchmark input') {
                        if ($timer.ElapsedMilliseconds -gt 1500 -and -not $retried) { $retried = $true; $value.SetValue('Zest benchmark input') }
                        Start-Sleep -Milliseconds 10
                    }
                    $editRoundTrip = $value.Current.Value -eq 'Zest benchmark input'
                    if ($editRoundTrip) { $editMilliseconds = $timer.Elapsed.TotalMilliseconds }
                    if ($editRoundTrip -and $textPattern) {
                        $text = $node.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
                        $textRoundTrip = $text.DocumentRange.GetText(-1) -eq 'Zest benchmark input'
                    }
                } catch [System.InvalidOperationException] {
                    $editRoundTrip = $false
                } finally {
                    if ($editRoundTrip) { $value.SetValue($previous) }
                }
            }
        }
    }
}
[ordered]@{ elements=$nodes.Count; buttons=$buttons; namedButtons=$namedButtons; messageEdit=$message; messageTextPattern=$textPattern; messageValuePattern=$valuePattern; editRoundTrip=$editRoundTrip; editMilliseconds=$editMilliseconds; textRoundTrip=$textRoundTrip } | ConvertTo-Json -Compress
if ($Check -and (!$buttons -or $buttons -ne $namedButtons -or !$editRoundTrip -or !$textRoundTrip)) {
    throw 'The workspace must expose named buttons and an editable Message field with readable text to Windows UI Automation.'
}
