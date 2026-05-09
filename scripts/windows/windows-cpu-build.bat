@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "SCRIPT_NAME=%~nx0"
for %%I in ("%~dp0.") do set "SCRIPT_DIR=%%~fI"
set "PROFILE=release"
set "CARGO_PROFILE_FLAG=--release"
set "SKIP_DOCTOR=0"
set "RUN_SUBMODULES=1"
set "INSTALL=1"
set "UPDATE_USER_PATH=1"
set "INSTALL_DIR="
set "BUNDLE_NAME=parakit-windows-x86_64-cpu"

if /I "%~1"=="--help" goto usage
if /I "%~1"=="-h" goto usage

:parse_args
if "%~1"=="" goto args_done
if /I "%~1"=="--release" (
    set "PROFILE=release"
    set "CARGO_PROFILE_FLAG=--release"
    shift
    goto parse_args
)
if /I "%~1"=="--debug" (
    set "PROFILE=debug"
    set "CARGO_PROFILE_FLAG="
    shift
    goto parse_args
)
if /I "%~1"=="--skip-doctor" (
    set "SKIP_DOCTOR=1"
    shift
    goto parse_args
)
if /I "%~1"=="--no-doctor" (
    set "SKIP_DOCTOR=1"
    shift
    goto parse_args
)
if /I "%~1"=="--no-submodules" (
    set "RUN_SUBMODULES=0"
    shift
    goto parse_args
)
if /I "%~1"=="--no-install" (
    set "INSTALL=0"
    shift
    goto parse_args
)
if /I "%~1"=="--no-user-path" (
    set "UPDATE_USER_PATH=0"
    shift
    goto parse_args
)
if /I "%~1"=="--install-dir" (
    if "%~2"=="" (
        echo error: --install-dir requires a directory argument. 1>&2
        exit /b 2
    )
    set "INSTALL_DIR=%~2"
    shift
    shift
    goto parse_args
)

echo error: unknown option "%~1" 1>&2
echo.
goto usage_error

:args_done
if /I not "%OS%"=="Windows_NT" (
    echo error: this script must run on native Windows cmd.exe. 1>&2
    exit /b 1
)

if defined WSL_DISTRO_NAME (
    echo error: this script must run on native Windows, not inside WSL. 1>&2
    exit /b 1
)
if defined WSL_INTEROP (
    echo error: this script must run on native Windows, not inside WSL. 1>&2
    exit /b 1
)

call :require cargo "Install Rust with rustup using the MSVC toolchain."
if errorlevel 1 exit /b 1
call :require rustc "Install Rust with rustup using the MSVC toolchain."
if errorlevel 1 exit /b 1
call :require cmake "Install CMake and ensure it is on PATH."
if errorlevel 1 exit /b 1
call :require git "Install Git for Windows and ensure it is on PATH."
if errorlevel 1 exit /b 1
call :require powershell "Windows PowerShell was not found on PATH."
if errorlevel 1 exit /b 1

set "RUST_HOST="
for /f "tokens=2" %%H in ('rustc -vV ^| findstr /b /c:"host:"') do set "RUST_HOST=%%H"
if "%RUST_HOST%"=="" (
    echo error: could not determine rustc host target. 1>&2
    exit /b 1
)
echo %RUST_HOST% | findstr /i /c:"pc-windows-msvc" >nul
if errorlevel 1 (
    echo error: rustc host is %RUST_HOST%; use the Rust MSVC toolchain for this Windows bundle. 1>&2
    exit /b 1
)

set "REPO="
for /f "delims=" %%R in ('git -C "%SCRIPT_DIR%" rev-parse --show-toplevel 2^>nul') do set "REPO=%%R"
if "%REPO%"=="" (
    echo error: could not find the parakit git repository root from "%SCRIPT_DIR%". 1>&2
    exit /b 1
)
cd /d "%REPO%" || exit /b 1

if not exist "Cargo.toml" (
    echo error: Cargo.toml was not found at "%REPO%". 1>&2
    exit /b 1
)

if "%INSTALL%"=="1" if "%INSTALL_DIR%"=="" (
    if not defined LOCALAPPDATA (
        echo error: LOCALAPPDATA is not set; pass --install-dir or use --no-install. 1>&2
        exit /b 1
    )
    set "INSTALL_DIR=%LOCALAPPDATA%\Programs\parakit"
)

if "%RUN_SUBMODULES%"=="1" (
    echo Updating submodules
    git submodule update --init --recursive
    if errorlevel 1 exit /b 1
)

echo Building %PROFILE%
if "%CARGO_PROFILE_FLAG%"=="" (
    cargo build --locked
) else (
    cargo build %CARGO_PROFILE_FLAG% --locked
)
if errorlevel 1 exit /b 1

if not exist "target" mkdir "target"
if errorlevel 1 exit /b 1

for %%I in ("target") do set "TARGET_ROOT=%%~fI"
set "PROFILE_DIR=%TARGET_ROOT%\%PROFILE%"
set "EXE=%PROFILE_DIR%\parakit.exe"
set "BUNDLE_DIR=%TARGET_ROOT%\%BUNDLE_NAME%"

if not exist "%EXE%" (
    echo error: parakit.exe was not produced at "%EXE%". 1>&2
    exit /b 1
)

call :assert_bundle_path "%BUNDLE_DIR%" "%TARGET_ROOT%"
if errorlevel 1 exit /b 1

if exist "%BUNDLE_DIR%" (
    rd /s /q "%BUNDLE_DIR%"
    if errorlevel 1 exit /b 1
)

mkdir "%BUNDLE_DIR%"
if errorlevel 1 exit /b 1

echo Bundle: %BUNDLE_DIR%
copy /y "%EXE%" "%BUNDLE_DIR%\" >nul
if errorlevel 1 exit /b 1

for %%F in ("%PROFILE_DIR%\*.dll") do (
    if exist "%%~fF" (
        call :copy_runtime_dll "%%~fF" "%%~nxF"
        if errorlevel 1 exit /b 1
    )
)

if exist "LICENSE" copy /y "LICENSE" "%BUNDLE_DIR%\" >nul

if not exist "%BUNDLE_DIR%\crispasr.dll" (
    echo error: crispasr.dll was not copied into the bundle. 1>&2
    exit /b 1
)
if not exist "%BUNDLE_DIR%\whisper.dll" (
    echo error: whisper.dll was not copied into the bundle. 1>&2
    exit /b 1
)
if not exist "%BUNDLE_DIR%\ggml.dll" (
    echo error: ggml.dll was not copied into the bundle. 1>&2
    exit /b 1
)

if "%SKIP_DOCTOR%"=="0" (
    echo Running doctor
    "%BUNDLE_DIR%\parakit.exe" doctor
    if errorlevel 1 exit /b 1
)

set "ACTIVE_DIR=%BUNDLE_DIR%"

if "%INSTALL%"=="1" (
    if "%UPDATE_USER_PATH%"=="1" (
        powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%\install-bundle.ps1" -BundleDir "%BUNDLE_DIR%" -InstallDir "%INSTALL_DIR%"
    ) else (
        powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%\install-bundle.ps1" -BundleDir "%BUNDLE_DIR%" -InstallDir "%INSTALL_DIR%" -NoUserPath
    )
    if errorlevel 1 exit /b 1
    for %%I in ("%INSTALL_DIR%") do set "ACTIVE_DIR=%%~fI"
)

if "%INSTALL%"=="0" echo Install: skipped
echo.
if "%INSTALL%"=="1" if "%UPDATE_USER_PATH%"=="1" goto print_path_commands
goto print_explicit_commands

:print_path_commands
echo Run now in this Command Prompt, or open a new terminal:
echo   parakit doctor --deep
echo   parakit
goto finish_print

:print_explicit_commands
echo Run:
echo   "%ACTIVE_DIR%\parakit.exe" doctor --deep
echo   "%ACTIVE_DIR%\parakit.exe"
goto finish_print

:finish_print
endlocal & set "PATH=%ACTIVE_DIR%;%PATH%"
exit /b 0

:require
where "%~1" >nul 2>nul
if errorlevel 1 (
    echo error: %~1 was not found. %~2 1>&2
    exit /b 1
)
exit /b 0

:copy_runtime_dll
set "DLL_PATH=%~1"
set "DLL_NAME=%~2"
if /I "%DLL_NAME:~0,9%"=="ggml-cpu-" exit /b 0
copy /y "%DLL_PATH%" "%BUNDLE_DIR%\" >nul
exit /b %ERRORLEVEL%

:assert_bundle_path
set "CHECK_BUNDLE=%~f1"
set "CHECK_TARGET=%~f2"
if "%CHECK_BUNDLE%"=="" (
    echo error: internal bundle path was empty. 1>&2
    exit /b 1
)
if /I "%CHECK_BUNDLE%"=="%CHECK_TARGET%" (
    echo error: refusing to use target root as the bundle directory. 1>&2
    exit /b 1
)
if /I not "%CHECK_BUNDLE%"=="%CHECK_TARGET%\%BUNDLE_NAME%" (
    echo error: refusing to operate outside the expected bundle directory. 1>&2
    echo        bundle: %CHECK_BUNDLE% 1>&2
    echo        target: %CHECK_TARGET% 1>&2
    exit /b 1
)
exit /b 0

:usage
echo Build and bundle Parakit CPU daemon on native Windows.
echo.
echo Usage:
echo   scripts\windows\%SCRIPT_NAME% [--release] [--debug] [--skip-doctor] [--no-submodules] [--no-install] [--no-user-path] [--install-dir DIR]
echo.
echo Options:
echo   --release        Build target\release and bundle it. This is the default.
echo   --debug          Build target\debug and bundle it into the same target bundle.
echo   --skip-doctor    Build and bundle without running parakit doctor.
echo   --no-submodules  Do not run git submodule update --init --recursive.
echo   --no-install     Build the repo-local bundle without installing it.
echo   --no-user-path   Install without adding the install directory to User PATH.
echo   --install-dir    Install to DIR instead of %%LOCALAPPDATA%%\Programs\parakit.
echo   -h, --help       Print this help.
echo.
echo The bundle is always recreated inside:
echo   target\%BUNDLE_NAME%
echo.
echo By default, the bundle is installed to:
echo   %%LOCALAPPDATA%%\Programs\parakit
exit /b 0

:usage_error
call :usage
exit /b 2
