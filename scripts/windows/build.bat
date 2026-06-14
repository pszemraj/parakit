@echo off
setlocal EnableExtensions DisableDelayedExpansion

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

"%POWERSHELL_EXE%" -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0build.ps1" %*
exit /b %ERRORLEVEL%
