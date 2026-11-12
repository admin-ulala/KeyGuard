@echo off
rem KeyGuard build entry: wires Rust (F:\environment\rust) + MSVC (F:\environment\vs-buildtools)
rem usage: tools\build.cmd build --release
rem        tools\build.cmd run -p kg-agent -- --probe
setlocal

set "RUSTUP_HOME=F:\environment\rust\rustup"
set "CARGO_HOME=F:\environment\rust\cargo"
set "PATH=%CARGO_HOME%\bin;%PATH%"

call "F:\environment\vs-buildtools\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
if errorlevel 1 (
  echo [build] vcvars64.bat failed
  exit /b 1
)

cd /d "%~dp0.."
cargo %*
exit /b %errorlevel%
