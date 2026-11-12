param([int]$X = 1110, [int]$Y = 27, [string]$Out = '.tmp/_close.png')
# 点窗口右上角的关闭按钮，然后截图 + 报告进程是否退出（验证不会再弹「请求格式不对」）
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public class K {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, int d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  public struct RECT { public int L, T, R, B; }
}
'@
$h = [IntPtr]::Zero
Get-Process | Where-Object { $_.MainWindowTitle -eq 'KeyGuard' } | ForEach-Object { $h = $_.MainWindowHandle }
if ($h -eq [IntPtr]::Zero) { Write-Error 'no window'; exit 1 }
[void][K]::SetWindowPos($h, (New-Object IntPtr(-1)), 0, 0, 0, 0, 0x43)
Start-Sleep -Milliseconds 500
$r = New-Object K+RECT; [void][K]::GetWindowRect($h, [ref]$r)

# 先截一张，确认点之前没有错误横幅
$bmp = New-Object System.Drawing.Bitmap -ArgumentList @(($r.R - $r.L), ($r.B - $r.T))
$g = [System.Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
[void][K]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()

[void][K]::SetCursorPos($r.L + $X, $r.T + $Y)
Start-Sleep -Milliseconds 200
[K]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 70
[K]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1500

$still = @(Get-Process -Name kg-ui -ErrorAction SilentlyContinue)
Write-Host ("窗口数量: " + $still.Count)
