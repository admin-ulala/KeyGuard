Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
'@

$h = [W.U]::FindWindowW('ConsoleWindowClass', 'KeyGuard Agent')
"console_hwnd : $h"
if ($h -ne [IntPtr]::Zero) { "visible      : " + [W.U]::IsWindowVisible($h) }
$fg = [W.U]::GetForegroundWindow()
$sb = New-Object System.Text.StringBuilder 256
[void][W.U]::GetClassNameW($fg, $sb, 256)
"foreground   : " + $sb.ToString()
