# 注入按键（测试用）。BOM 必须有，否则 PS 5.1 按 GBK 解析中文注释会炸。
# 例：sendinput.ps1 -Spec alt+tab
#     sendinput.ps1 -Spec a -Repeat 60 -GapMs 5
param(
    [string]$Spec = 'win',
    [int]$Repeat = 1,
    [int]$GapMs = 3
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace KG -Name Snd -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
[DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint mapType);
'@

$KEYUP = 0x0002
$map = @{
    'win' = 0x5B; 'lwin' = 0x5B; 'rwin' = 0x5C
    'alt' = 0x12; 'ctrl' = 0x11; 'shift' = 0x10
    'tab' = 0x09; 'esc' = 0x1B; 'space' = 0x20
    'f4' = 0x73; 'f8' = 0x77; 'f12' = 0x7B
    'prtsc' = 0x2C; 'g' = 0x47; 'd' = 0x44; 'a' = 0x41
}

function Tap([byte]$vk) {
    $scan = [byte][KG.Snd]::MapVirtualKeyW($vk, 0)
    [KG.Snd]::keybd_event($vk, $scan, 0, [IntPtr]::Zero)
    [KG.Snd]::keybd_event($vk, $scan, $KEYUP, [IntPtr]::Zero)
}

for ($n = 0; $n -lt $Repeat; $n++) {
    $keys = $Spec.Split('+')
    foreach ($k in $keys) {
        $name = $k.Trim().ToLower()
        if (-not $map.ContainsKey($name)) { throw "未知按键: $name" }
        $scan = [byte][KG.Snd]::MapVirtualKeyW($map[$name], 0)
        [KG.Snd]::keybd_event([byte]$map[$name], $scan, 0, [IntPtr]::Zero)
    }
    [array]::Reverse($keys)
    foreach ($k in $keys) {
        $name = $k.Trim().ToLower()
        $scan = [byte][KG.Snd]::MapVirtualKeyW($map[$name], 0)
        [KG.Snd]::keybd_event([byte]$map[$name], $scan, $KEYUP, [IntPtr]::Zero)
    }
    if ($Repeat -gt 1) { Start-Sleep -Milliseconds $GapMs }
}

Write-Output "sent: $Spec x$Repeat"
