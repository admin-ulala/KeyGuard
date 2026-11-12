# 把四条无障碍快捷键的标志位恢复成 Windows 默认形态（热键位按当前拦截状态）
# 只走 SystemParametersInfoW，直接写注册表没用——SPI 读的是运行时副本。
Add-Type @'
using System; using System.Runtime.InteropServices;
public class SPI {
  [StructLayout(LayoutKind.Sequential)] public struct SK { public uint cbSize; public uint flags; }
  [StructLayout(LayoutKind.Sequential)] public struct TK { public uint cbSize; public uint flags; }
  [StructLayout(LayoutKind.Sequential)] public struct FK {
    public uint cbSize; public uint flags; public uint wait; public uint delay; public uint repeat; public uint bounce;
  }
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SystemParametersInfoW(uint a, uint u, ref SK p, uint f);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SystemParametersInfoW(uint a, uint u, ref TK p, uint f);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SystemParametersInfoW(uint a, uint u, ref FK p, uint f);
}
'@
# SPIF_UPDATEINIFILE | SPIF_SENDCHANGE = 0x3
$f = 0x3
$sk = New-Object SPI+SK; $sk.cbSize = 8; $sk.flags = 506   # 0x1FA：可用、热键关、确认/提示/三态/双键关都在
$ok1 = [SPI]::SystemParametersInfoW(0x003B, 8, [ref]$sk, $f)
$tk = New-Object SPI+TK; $tk.cbSize = 8; $tk.flags = 58    # 0x3A：可用、热键关
$ok2 = [SPI]::SystemParametersInfoW(0x0035, 8, [ref]$tk, $f)
$fk = New-Object SPI+FK; $fk.cbSize = 24; $fk.flags = 122  # 0x7A：可用、热键关
$ok3 = [SPI]::SystemParametersInfoW(0x0033, 24, [ref]$fk, $f)
"sticky=$ok1 toggle=$ok2 filter=$ok3"
