Start-Transcript -Path 'F:\development\win_use\uipi-test.log' -Force | Out-Null
$ErrorActionPreference = 'Continue'
$agent = 'D:\Program Files\KeyGuard\kg-agent.exe'

Get-Process kg-agent -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 900

Start-Process $agent -ArgumentList '--agent', '--allow-injected' -WindowStyle Minimized
Start-Sleep -Seconds 2

& $agent lock | Out-Null
Start-Sleep -Milliseconds 500

# 把前台切到提权 Notepad：这就是 P1 验收门第 2 条的场景
$np = Get-Process notepad -ErrorAction SilentlyContinue | Select-Object -First 1
if ($np) {
    (New-Object -ComObject WScript.Shell).AppActivate($np.Id) | Out-Null
    Start-Sleep -Milliseconds 900
} else {
    "WARN: 没有提权 notepad，改用当前前台"
}

"=== BEFORE ==="
& $agent status

& powershell -NoProfile -ExecutionPolicy Bypass -File 'F:\development\win_use\tools\sendinput.ps1' -Spec win -Repeat 3 -GapMs 150 | Out-Null
Start-Sleep -Milliseconds 800

"=== AFTER ==="
& $agent status

Stop-Transcript | Out-Null
