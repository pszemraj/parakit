@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "PARAKIT_WINDOWS_BUILD_COMMAND=scripts\windows\windows-vulkan-build.bat"
set "PARAKIT_WINDOWS_BUILD_FLAVOR_ARG=--vulkan"

call "%~dp0run-windows-build.bat" %*
exit /b %ERRORLEVEL%
