# Build and bundle Parakit CPU daemon on native Windows.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/windows-cpu-build.ps1
#
# This script intentionally does not enable CUDA. Validate the native CPU
# daemon before adding GPU toolchain and runtime DLL complexity.

param(
    [switch]$SkipDoctor
)

$ErrorActionPreference = "Stop"

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

Write-Host "parakit: repo root = $repo"

Write-Host "parakit: initializing submodules"
Invoke-Checked "git" "submodule" "update" "--init" "--recursive"

Write-Host "parakit: building native Windows CPU release"
Invoke-Checked "cargo" "build" "--release" "--locked"

$targetRoot = Join-Path $repo "target"
$releaseDir = Join-Path $targetRoot "release"
$exe = Join-Path $releaseDir "parakit.exe"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "parakit.exe was not produced at $exe"
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

Write-Host "parakit: creating bundle at $bundleDir"
Copy-Item -LiteralPath $exe -Destination $bundleDir -Force

Get-ChildItem -LiteralPath $releaseDir -Filter "*.dll" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch '^ggml-cpu-.+\.dll$' } |
    ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $bundleDir -Force }

Copy-IfExists -Path (Join-Path $repo "LICENSE") -Destination $bundleDir

$bundleExe = Join-Path $bundleDir "parakit.exe"

Write-Host "parakit: bundle contents"
Get-ChildItem -LiteralPath $bundleDir | Select-Object Name, Length | Format-Table -AutoSize

if (-not $SkipDoctor) {
    Write-Host "parakit: running doctor"
    Invoke-Checked $bundleExe "doctor"
}

Write-Host ""
Write-Host "parakit: CPU Windows bundle ready:"
Write-Host "  $bundleDir"
Write-Host ""
$env:Path = "$bundleDir;$env:Path"
Write-Host "parakit: added bundle to PATH for this PowerShell process"
Write-Host ""
Write-Host "Next manual checks:"
Write-Host "  parakit doctor --deep"
Write-Host "  parakit"
