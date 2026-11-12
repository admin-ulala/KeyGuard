# 截一个窗口的图（WebView2 必须用 PW_RENDERFULLCONTENT = 2，否则是黑块）
param(
  [string]$Title = 'KeyGuard',
  [string]$Out = '.tmp/ui.png'
)

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr l);
public delegate bool EnumWindowsProc(IntPtr h, IntPtr l);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
'@

# 按标题精确匹配（FindWindowW 对自绘窗口时灵时不灵，枚举更稳）
$script:found = [IntPtr]::Zero
[void][W.U]::EnumWindows({ param($h,$l)
  if ([W.U]::IsWindowVisible($h)) {
    $sb = New-Object System.Text.StringBuilder 512
    [void][W.U]::GetWindowTextW($h, $sb, 512)
    if ($sb.ToString() -eq $Title) { $script:found = $h; return $false }
  }
  return $true }, [IntPtr]::Zero)
$h = $script:found
if ($h -eq [IntPtr]::Zero) { Write-Error "找不到窗口: $Title"; exit 1 }
[void][W.U]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 350

$r = New-Object W.U+RECT
[void][W.U]::GetWindowRect($h, [ref]$r)
$w = $r.R - $r.L; $hgt = $r.B - $r.T
if ($w -le 0 -or $hgt -le 0) { Write-Error "窗口尺寸异常 $w x $hgt"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap $w, $hgt
$g = [System.Drawing.Graphics]::FromImage($bmp)
$dc = $g.GetHdc()
$ok = [W.U]::PrintWindow($h, $dc, 2)
$g.ReleaseHdc($dc)
$g.Dispose()

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output ("{0}  {1}x{2}  ok={3}" -f $Out, $w, $hgt, $ok)
