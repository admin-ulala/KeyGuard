param([string]$Match = 'FakeGame')
$p = Get-Process | Where-Object { $_.MainWindowTitle -like "*$Match*" } | Select-Object -First 1
if (-not $p) { "no window matching $Match"; exit 1 }
$sh = New-Object -ComObject WScript.Shell
[void]$sh.AppActivate($p.Id)
"activated: $($p.MainWindowTitle)"
