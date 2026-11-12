# 打印进程的完整性级别（UIPI 判定的核心依据）
# 用法: powershell -NoProfile -ExecutionPolicy Bypass -File tools\il-check.ps1 [-Names explorer,Taskmgr]
param([string[]]$Names = @('explorer', 'svchost', 'dwm', 'lsass', 'notepad', 'VALORANT-Win64-Shipping', 'cs2'))
if ($Names.Count -eq 1 -and $Names[0] -like "*,*") { $Names = $Names[0].Split(",") }
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class KeyGuardIL
{
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    const uint TOKEN_QUERY = 0x0008;
    const int  TokenIntegrityLevel = 25;

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr h);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetTokenInformation(IntPtr token, int cls, byte[] buf, int len, out int needed);

    // 直接从 SID 二进制里读最后一个子权限，避免字符串编组带来的编码坑
    public static int Rid(int procId, out string err)
    {
        err = null;
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, procId);
        if (h == IntPtr.Zero) { err = "无法打开 Win32=" + Marshal.GetLastWin32Error(); return int.MinValue; }
        IntPtr tok = IntPtr.Zero;
        try
        {
            if (!OpenProcessToken(h, TOKEN_QUERY, out tok)) { err = "无令牌 Win32=" + Marshal.GetLastWin32Error(); return int.MinValue; }
            byte[] buf = new byte[68];
            int need;
            if (!GetTokenInformation(tok, TokenIntegrityLevel, buf, buf.Length, out need))
            { err = "查询失败 Win32=" + Marshal.GetLastWin32Error(); return int.MinValue; }
            IntPtr sid = (IntPtr)BitConverter.ToInt64(buf, 0);
            int count = Marshal.ReadByte(sid, 1);
            if (count < 1) { err = "SID 异常"; return int.MinValue; }
            return Marshal.ReadInt32(sid, 8 + (count - 1) * 4);
        }
        finally
        {
            if (tok != IntPtr.Zero) CloseHandle(tok);
            CloseHandle(h);
        }
    }

    public static string Name(int rid)
    {
        switch (rid)
        {
            case 16384: return "System     ";
            case 12288: return "High  提权 ";
            case 8192:  return "Medium     ";
            case 4096:  return "Low        ";
            case 8448:  return "AppContainer";
            default:    return "Unknown    ";
        }
    }
}
'@

function Show([string]$label, [int]$procId) {
    $err = $null
    $rid = [KeyGuardIL]::Rid($procId, [ref]$err)
    if ($rid -eq [int]::MinValue) { "  {0,-24} pid={1,-7} -- {2}" -f $label, $procId, $err }
    else { "  {0,-24} pid={1,-7} {2} (RID {3})" -f $label, $procId, [KeyGuardIL]::Name($rid), $rid }
}

$me = [System.Diagnostics.Process]::GetCurrentProcess()
$id = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$pr = New-Object System.Security.Principal.WindowsPrincipal($id)

"当前 PowerShell:"
Show "powershell.exe" $me.Id
"  在管理员组: " + $pr.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
"  登录名:     " + $id.Name
""
"目标进程:"
foreach ($n in $Names) {
    $ps = @(Get-Process -Name $n -ErrorAction SilentlyContinue)
    if ($ps.Count -eq 0) { "  {0,-24} 未运行" -f $n; continue }
    foreach ($p in $ps) { Show ("  " + $p.ProcessName) $p.Id }
}
