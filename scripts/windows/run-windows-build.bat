@echo off
setlocal EnableExtensions DisableDelayedExpansion

for %%I in ("%~dp0.") do set "SCRIPT_DIR=%%~fI"
set "PS_SCRIPT=%SCRIPT_DIR%\windows-cpu-build.ps1"

set "POWERSHELL_EXE=powershell.exe"
where "%POWERSHELL_EXE%" >nul 2>nul
if errorlevel 1 (
    if exist "%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" (
        set "POWERSHELL_EXE=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
    ) else (
        echo error: Windows PowerShell was not found. 1>&2
        exit /b 1
    )
)

"%POWERSHELL_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%PS_SCRIPT%" %PARAKIT_WINDOWS_BUILD_FLAVOR_ARG% %*
exit /b %ERRORLEVEL%
