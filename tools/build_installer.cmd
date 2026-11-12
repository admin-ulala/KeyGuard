@echo off
rem Build KeyGuard installers with NSIS (portable, no install needed).
rem Usage: tools\build_installer.cmd
setlocal
set NSIS=F:\environment\nsis\makensis.exe
if not exist "%NSIS%" (
  echo [x] makensis not found: %NSIS%
  exit /b 1
)
cd /d "%~dp0.."
if not exist "target\release\kg-agent.exe" (
  echo [x] build the binaries first: tools\build.cmd build --release
  exit /b 1
)
if not exist "dist" mkdir dist

echo [1/2] per-user build (self test, no UAC)
"%NSIS%" /INPUTCHARSET UTF8 /DKEYGUARD_PER_USER installer\keyguard.nsi || exit /b 1

echo [2/2] admin build (Program Files, creates the scheduled task)
"%NSIS%" /INPUTCHARSET UTF8 installer\keyguard.nsi || exit /b 1

echo.
dir /b dist\*.exe
endlocal
