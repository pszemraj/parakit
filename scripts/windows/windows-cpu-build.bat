@echo off
setlocal EnableExtensions DisableDelayedExpansion

for %%I in ("%~dp0.") do set "SCRIPT_DIR=%%~fI"
set "PS_SCRIPT=%SCRIPT_DIR%\windows-cpu-build.ps1"

where powershell >nul 2>nul
if errorlevel 1 (
    echo error: Windows PowerShell was not found on PATH. 1>&2
    exit /b 1
)

powershell -NoProfile -ExecutionPolicy Bypass -File "%PS_SCRIPT%" %*
exit /b %ERRORLEVEL%
