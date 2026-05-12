# Build and bundle Parakit CPU daemon on native Windows.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/windows-cpu-build.ps1 [options]
#
# By default this builds a repo-local bundle, installs it to the per-user
# Windows app directory, and adds that directory to the User PATH.
#
# This script intentionally does not enable CUDA. Validate the native CPU
# daemon before adding GPU toolchain and runtime DLL complexity.

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RawArgs
)

$ErrorActionPreference = "Stop"

$Profile = "release"
$NoInstall = $false
$NoUserPath = $false
$NoSubmodules = $false
$InstallDir = $null

if ($DebugPreference -ne "SilentlyContinue") {
    $Profile = "debug"
}

function Show-Usage {
    $scriptName = Split-Path -Leaf $PSCommandPath
    $entryPoint = $env:PARAKIT_WINDOWS_BUILD_COMMAND
    if ([string]::IsNullOrWhiteSpace($entryPoint)) {
        $entryPoint = "scripts\windows\$scriptName"
    }

    Write-Host "Build and bundle Parakit CPU daemon on native Windows."
    Write-Host ""
    Write-Host "Usage:"
    Write-Host "  $entryPoint [--release] [--debug] [--no-submodules] [--no-install] [--no-user-path] [--install-dir DIR]"
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  --release        Build target\release and bundle it. This is the default."
    Write-Host "  --debug          Build target\debug and bundle it into the same target bundle."
    Write-Host "  --no-submodules  Do not run git submodule update --init --recursive."
    Write-Host "  --no-install     Build the repo-local bundle without installing it."
    Write-Host "  --no-user-path   Install without adding the install directory to User PATH."
    Write-Host "  --install-dir    Install to DIR instead of `%LOCALAPPDATA`%\Programs\parakit."
    Write-Host "  -h, --help       Print this help."
}

for ($i = 0; $i -lt $RawArgs.Count; $i++) {
    switch -Regex ($RawArgs[$i]) {
        '^(--help|-h|-Help)$' {
            Show-Usage
            exit 0
        }
        '^(--release|-release|-Release)$' {
            $Profile = "release"
        }
        '^(--debug|-debug|-Profile|-profile)$' {
            if ($RawArgs[$i] -eq "-Profile") {
                $i++
                if ($i -ge $RawArgs.Count -or $RawArgs[$i] -notin @("release", "debug")) {
                    throw "-Profile requires 'release' or 'debug'"
                }
                $Profile = $RawArgs[$i]
            } else {
                $Profile = "debug"
            }
        }
        '^(--no-submodules|-no-submodules|-NoSubmodules)$' {
            $NoSubmodules = $true
        }
        '^(--no-install|-no-install|-NoInstall)$' {
            $NoInstall = $true
        }
        '^(--no-user-path|-no-user-path|-NoUserPath)$' {
            $NoUserPath = $true
        }
        '^(--install-dir|-install-dir|-InstallDir)$' {
            $i++
            if ($i -ge $RawArgs.Count -or [string]::IsNullOrWhiteSpace($RawArgs[$i])) {
                throw "$($RawArgs[$i - 1]) requires a directory argument"
            }
            $InstallDir = $RawArgs[$i]
        }
        default {
            throw "unknown option: $($RawArgs[$i])"
        }
    }
}

function Require-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$InstallHint
    )

    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        throw "$Name was not found. $InstallHint"
    }
}

function Assert-NativeWindows {
    if ($env:WSL_DISTRO_NAME -or $env:WSL_INTEROP) {
        throw "This script must run in native Windows PowerShell, not inside WSL. WSL builds Linux binaries by default."
    }

    if ($PSVersionTable.PSEdition -eq "Core") {
        if (-not $IsWindows) {
            throw "This script must run on Windows."
        }
        return
    }

    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "This script must run on Windows."
    }
}

function Get-RepoRoot {
    $scriptPath = $PSCommandPath
    if (-not $scriptPath) {
        $scriptPath = $MyInvocation.MyCommand.Path
    }
    $scriptDir = Split-Path -Parent $scriptPath
    return (Resolve-Path (Join-Path $scriptDir "..\..")).Path
}

function Get-DefaultInstallDir {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return (Join-Path $env:USERPROFILE "AppData\Local\Programs\parakit")
    }

    return (Join-Path $env:LOCALAPPDATA "Programs\parakit")
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Child,

        [Parameter(Mandatory = $true)]
        [string]$Parent
    )

    $childFull = [System.IO.Path]::GetFullPath($Child)
    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $prefix = $parentFull + [System.IO.Path]::DirectorySeparatorChar
    if (-not $childFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to operate outside expected directory. Path: $childFull Parent: $parentFull"
    }
}

function Copy-IfExists {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Destination
    )

    if (Test-Path -LiteralPath $Path) {
        Copy-Item -LiteralPath $Path -Destination $Destination -Force
    }
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,

        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

Assert-NativeWindows

Require-Command "cargo" "Install Rust with rustup using the MSVC toolchain."
Require-Command "rustc" "Install Rust with rustup using the MSVC toolchain."
Require-Command "cmake" "Install CMake and ensure it is on PATH."
Require-Command "git" "Install Git for Windows and ensure it is on PATH."

$repo = Get-RepoRoot
Set-Location $repo

if (-not $NoSubmodules) {
    Write-Host "Updating submodules"
    Invoke-Checked "git" "submodule" "update" "--init" "--recursive"
}

Write-Host "Building $Profile"
$cargoArgs = @("build", "--locked")
if ($Profile -eq "release") {
    $cargoArgs += "--release"
}
Invoke-Checked "cargo" @cargoArgs

$targetRoot = Join-Path $repo "target"
$profileDir = Join-Path $targetRoot $Profile
$exe = Join-Path $profileDir "parakit.exe"
$runtimeManifest = Join-Path $profileDir "parakit-runtime-manifest.json"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "parakit.exe was not produced at $exe"
}

if (-not (Test-Path -LiteralPath $runtimeManifest -PathType Leaf)) {
    throw "Runtime manifest was not produced at $runtimeManifest"
}

if (-not (Test-Path -LiteralPath $targetRoot)) {
    New-Item -ItemType Directory -Path $targetRoot | Out-Null
}

$bundleDir = Join-Path $targetRoot "parakit-windows-x86_64-cpu"
Assert-ChildPath -Child $bundleDir -Parent $targetRoot

if (Test-Path -LiteralPath $bundleDir) {
    Remove-Item -LiteralPath $bundleDir -Recurse -Force
}
New-Item -ItemType Directory -Path $bundleDir | Out-Null

Write-Host "Bundle: $bundleDir"
Copy-Item -LiteralPath $exe -Destination $bundleDir -Force
Copy-Item -LiteralPath $runtimeManifest -Destination $bundleDir -Force

Get-ChildItem -LiteralPath $profileDir -Filter "*.dll" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch '^ggml-cpu-.+\.dll$' } |
    ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $bundleDir -Force }

Copy-IfExists -Path (Join-Path $repo "LICENSE") -Destination $bundleDir
Copy-IfExists -Path (Join-Path $repo "README.md") -Destination $bundleDir

$activeDir = $bundleDir

if (-not $NoInstall) {
    if ([string]::IsNullOrWhiteSpace($InstallDir)) {
        $InstallDir = Get-DefaultInstallDir
    }

    $installer = Join-Path $repo "scripts\windows\install-bundle.ps1"

    if ($NoUserPath) {
        & $installer -BundleDir $bundleDir -InstallDir $InstallDir -NoUserPath
    } else {
        & $installer -BundleDir $bundleDir -InstallDir $InstallDir
    }
    if (-not $?) {
        throw "Windows bundle install failed"
    }

    $activeDir = [System.IO.Path]::GetFullPath($InstallDir)
}

if ($NoInstall) {
    Write-Host "Install: skipped"
}
