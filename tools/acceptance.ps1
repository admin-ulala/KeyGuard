# P1 验收门自动化部分：注入 Win / Alt+Tab / Alt+F4，检查前台窗口有没有被切走。
# 物理键盘请手动再验一次（注入输入走的是同一条钩子链路，但系统对注入有额外处理）。
$ErrorActionPreference = 'Stop'
Add-Type -Namespace KG -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindowTextW(System.IntPtr h, System.Text.StringBuilder s, int n);
'@
function Get-Fg {
  $h = [KG.Win]::GetForegroundWindow()
  $c = New-Object System.Text.StringBuilder 256
  $t = New-Object System.Text.StringBuilder 256
  [void][KG.Win]::GetClassName($h, $c, 256)
  [void][KG.Win]::GetWindowTextW($h, $t, 256)
  return "$($c.ToString()) | $($t.ToString())"
}

$exe = Join-Path $PSScriptRoot '..\target\debug\kg-agent.exe'
$log = Join-Path $env:TEMP 'kg-acceptance.log'
Remove-Item $log -ErrorAction SilentlyContinue

$p = Start-Process -FilePath $exe -ArgumentList '--armed','--allow-injected' -RedirectStandardOutput $log -WindowStyle Hidden -PassThru
Start-Sleep -Milliseconds 1200

$before = Get-Fg
& (Join-Path $PSScriptRoot 'sendinput.ps1') -Spec 'win'      | Out-Null
& (Join-Path $PSScriptRoot 'sendinput.ps1') -Spec 'alt+tab'  | Out-Null
& (Join-Path $PSScriptRoot 'sendinput.ps1') -Spec 'alt+f4'   | Out-Null
Start-Sleep -Milliseconds 1200
$after = Get-Fg

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300

$blocked = (Select-String -Path $log -Pattern 'blocked=(\d+)' | Select-Object -Last 1).Matches.Groups[1].Value

Write-Output "前台(前): $before"
Write-Output "前台(后): $after"
Write-Output "拦截计数: $blocked"
if ($before -eq $after) { Write-Output "RESULT: PASS - 前台窗口未被切走，按键被吞" ; exit 0 }
else { Write-Output "RESULT: FAIL - 前台窗口发生变化，有按键漏过" ; exit 1 }
