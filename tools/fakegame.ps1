# 造一个「全屏游戏」窗口用来测全屏检测：无边框 + 覆盖整块显示器 + 置顶 + 强抢前台。
# 注意：
#   1) 用 Bounds 而不是 WindowState=Maximized —— 最大化会让出任务栏高度，检测层会判定「没盖满」。
#   2) 后台进程默认拿不到前台权（前台锁），必须先用一次 Alt 点击解锁再 SetForegroundWindow。
param([int]$Seconds = 6)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type -Namespace KG -Name Fore -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
'@

$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = 'None'
$form.StartPosition = 'Manual'
$form.Bounds = $screen
$form.TopMost = $true
$form.BackColor = [System.Drawing.Color]::FromArgb(8, 10, 14)
$form.ShowInTaskbar = $false
$form.Text = 'FAKE-FULLSCREEN'

$label = New-Object System.Windows.Forms.Label
$label.Text = 'FAKE FULLSCREEN GAME'
$label.ForeColor = [System.Drawing.Color]::FromArgb(70, 220, 180)
$label.Font = New-Object System.Drawing.Font('Segoe UI', 28, [System.Drawing.FontStyle]::Bold)
$label.AutoSize = $true
$form.Controls.Add($label)

$form.Add_Shown({
    $label.Left = [int](($form.ClientSize.Width - $label.Width) / 2)
    $label.Top = [int](($form.ClientSize.Height - $label.Height) / 2)
    # 解锁前台：Alt 按下 + 抬起，再抢一次
    [KG.Fore]::keybd_event(0x12, 0, 0, [IntPtr]::Zero)
    [KG.Fore]::keybd_event(0x12, 0, 2, [IntPtr]::Zero)
    [void][KG.Fore]::SetForegroundWindow($form.Handle)
    Write-Output ("fg-now=" + [KG.Fore]::GetForegroundWindow() + " me=" + $form.Handle)
})

$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = $Seconds * 1000
$timer.Add_Tick({ $timer.Stop(); $form.Close() })
$timer.Start()

[void]$form.ShowDialog()
Write-Output "fakegame closed after ${Seconds}s"
