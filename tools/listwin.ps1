Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public struct RECT { public int L, T, R, B; }
}
'@

$rows = New-Object System.Collections.ArrayList
$cb = [Win+EnumProc]{
  param($h, $p)
  $sb = New-Object System.Text.StringBuilder 256
  [void][Win]::GetWindowTextW($h, $sb, 256)
  $t = $sb.ToString()
  if ($t.Length -gt 0 -and $t -match 'Key') {
    $r = New-Object Win+RECT
    [void][Win]::GetWindowRect($h, [ref]$r)
    $procId = 0
    [void][Win]::GetWindowThreadProcessId($h, [ref]$procId)
    [void]$rows.Add([pscustomobject]@{
      hwnd = $h; pid = $procId; vis = [Win]::IsWindowVisible($h)
      w = ($r.R - $r.L); hgt = ($r.B - $r.T); title = $t
    })
  }
  return $true
}
[void][Win]::EnumWindows($cb, [IntPtr]::Zero)
$rows | Format-Table -AutoSize | Out-String -Width 200
