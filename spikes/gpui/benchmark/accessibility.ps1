param([Parameter(Mandatory)][int]$AppProcessId)

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
                    while ($timer.ElapsedMilliseconds -lt 1000 -and $value.Current.Value -ne 'Zest benchmark input') {
                        Start-Sleep -Milliseconds 10
                    }
                    $editRoundTrip = $value.Current.Value -eq 'Zest benchmark input'
                    if ($editRoundTrip) { $editMilliseconds = $timer.Elapsed.TotalMilliseconds }
                } catch [System.InvalidOperationException] {
                    $editRoundTrip = $false
                } finally {
                    if ($editRoundTrip) { $value.SetValue($previous) }
                }
            }
        }
    }
}
[ordered]@{ elements=$nodes.Count; buttons=$buttons; namedButtons=$namedButtons; messageEdit=$message; messageTextPattern=$textPattern; messageValuePattern=$valuePattern; editRoundTrip=$editRoundTrip; editMilliseconds=$editMilliseconds } | ConvertTo-Json -Compress
