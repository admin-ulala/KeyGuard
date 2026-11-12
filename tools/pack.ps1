# 打安装包。顺序不能反：安装器里 File 指令指向 ..\target\release，
# 先 makensis 后 build 就会把上一版二进制装进新包（这就是「重装了还是老样子」的由来）。
#
#   powershell -ExecutionPolicy Bypass -File tools\pack.ps1              # 两个包都打
#   powershell -ExecutionPolicy Bypass -File tools\pack.ps1 -SkipBuild   # 只重打包（二进制没变时）
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

$env:KG_BUILD_STAMP = Get-Date -Format 'yyyyMMdd-HHmm'
Write-Host "构建标记: $env:KG_BUILD_STAMP"

if (-not $SkipBuild) {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败" }
}

foreach ($exe in @('kg-agent.exe', 'kg-ui.exe')) {
    $p = Join-Path $root "target\release\$exe"
    if (-not (Test-Path $p)) { throw "缺少 $p —— 先跑 cargo build --release" }
    $t = (Get-Item $p).LastWriteTime
    Write-Host ("  {0}  {1:yyyy-MM-dd HH:mm}" -f $exe, $t)
}

$makensis = 'F:\environment\nsis\makensis.exe'
if (-not (Test-Path $makensis)) { $makensis = 'makensis' }

Push-Location (Join-Path $root 'installer')
try {
    & $makensis keyguard.nsi
    if ($LASTEXITCODE -ne 0) { throw "makensis（默认版）失败" }
    & $makensis /DKEYGUARD_PER_USER keyguard.nsi
    if ($LASTEXITCODE -ne 0) { throw "makensis（per-user）失败" }
} finally {
    Pop-Location
}

Get-ChildItem (Join-Path $root 'dist') | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize
