# Build and bundle Parakit daemon flavors on native Windows.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/windows-cpu-build.ps1 [options]
#
# By default this builds a repo-local bundle, installs it to the per-user
# Windows app directory, and adds that directory to the User PATH.
#
# One accelerator flavor is supported per bundle. CUDA requires a local CUDA
# Toolkit; Vulkan requires the LunarG Vulkan SDK at build time.

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
$Flavor = "cpu"
$BundleCudaDlls = $false

if ($DebugPreference -ne "SilentlyContinue") {
    $Profile = "debug"
}

function Show-Usage {
    $scriptName = Split-Path -Leaf $PSCommandPath
    $entryPoint = $env:PARAKIT_WINDOWS_BUILD_COMMAND
    if ([string]::IsNullOrWhiteSpace($entryPoint)) {
        $entryPoint = "scripts\windows\$scriptName"
    }

    Write-Host "Build and bundle Parakit daemon flavors on native Windows."
    Write-Host ""
    Write-Host "Usage:"
    Write-Host "  $entryPoint [--cuda | --vulkan] [--bundle-cuda-dlls] [--release] [--debug] [--no-submodules] [--no-install] [--no-user-path] [--install-dir DIR]"
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  --cuda           Build a CUDA bundle. Requires NVIDIA CUDA Toolkit on this machine."
    Write-Host "  --vulkan         Build a Vulkan bundle. Requires LunarG Vulkan SDK and glslc."
    Write-Host "  --bundle-cuda-dlls"
    Write-Host "                   CUDA only: copy cublas64_*.dll and cublasLt64_*.dll into the bundle."
    Write-Host "  --release        Build target\release and bundle it. This is the default."
    Write-Host "  --debug          Build target\debug and bundle it into the same target bundle."
    Write-Host "  --no-submodules  Do not run git submodule update --init --recursive."
    Write-Host "  --no-install     Build the repo-local bundle without installing it."
    Write-Host "  --no-user-path   Install without adding the install directory to User PATH."
    Write-Host "  --install-dir    Install to DIR instead of `%LOCALAPPDATA`%\Programs\parakit."
    Write-Host "  -h, --help       Print this help."
}

function Set-BuildFlavor {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("cpu", "cuda", "vulkan")]
        [string]$Value
    )

    if ($Flavor -ne "cpu" -and $Flavor -ne $Value) {
        throw "Only one accelerator flavor can be selected per bundle. Choose either --cuda or --vulkan."
    }
    $script:Flavor = $Value
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
        '^(--cuda|-cuda|-Cuda)$' {
            Set-BuildFlavor "cuda"
        }
        '^(--vulkan|-vulkan|-Vulkan)$' {
            Set-BuildFlavor "vulkan"
        }
        '^(--bundle-cuda-dlls|-bundle-cuda-dlls|-BundleCudaDlls)$' {
            $BundleCudaDlls = $true
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

if ($BundleCudaDlls -and $Flavor -ne "cuda") {
    throw "--bundle-cuda-dlls is only valid with --cuda."
}

function Require-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$InstallHint
    )

    if (-not (Test-Command $Name)) {
        throw "$Name was not found. $InstallHint"
    }
}

function Test-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Assert-CudaBuildReady {
    Require-Command "nvcc" "Install the NVIDIA CUDA Toolkit and ensure nvcc is on PATH."

    if ([string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        throw "CUDA_PATH is not set. Install the NVIDIA CUDA Toolkit, or set CUDA_PATH to the toolkit root for this shell."
    }

    $cudaBin = Join-Path $env:CUDA_PATH "bin"
    if (-not (Test-Path -LiteralPath $cudaBin -PathType Container)) {
        throw "CUDA_PATH does not contain a bin directory: $env:CUDA_PATH"
    }

    if ([string]::IsNullOrWhiteSpace($env:CMAKE_GENERATOR) -and -not (Test-CudaVisualStudioIntegration)) {
        throw "CUDA Visual Studio integration was not found. Install the Visual Studio integration component from the CUDA Toolkit installer, or set CMAKE_GENERATOR=Ninja for this shell and ensure nvcc is on PATH."
    }

    Write-Host "CUDA: using toolkit at $env:CUDA_PATH"
    Write-Host "CUDA: ggml-cuda first build can take tens of minutes; native/default arch keeps it to this machine."
}

function Test-CudaVisualStudioIntegration {
    $roots = @()
    if (-not [string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        $roots += Join-Path $env:CUDA_PATH "extras\visual_studio_integration"
    }
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $roots += Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $roots += Join-Path $env:ProgramFiles "Microsoft Visual Studio"
    }
    $roots = $roots | Where-Object { Test-Path -LiteralPath $_ -PathType Container }

    foreach ($root in $roots) {
        $match = Get-ChildItem -LiteralPath $root -Recurse -Filter "CUDA *.props" -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $match) {
            return $true
        }
    }

    return $false
}

function Assert-VulkanBuildReady {
    if ([string]::IsNullOrWhiteSpace($env:VULKAN_SDK)) {
        $detected = Get-NewestVulkanSdk
        if ([string]::IsNullOrWhiteSpace($detected)) {
            throw "VULKAN_SDK is not set and no C:\VulkanSDK install was found. Install the LunarG Vulkan SDK from vulkan.lunarg.com, or use winget install KhronosGroup.VulkanSDK."
        }
        $env:VULKAN_SDK = $detected
        Write-Host "Vulkan: auto-detected SDK at $env:VULKAN_SDK"
    }

    if (-not (Test-Path -LiteralPath $env:VULKAN_SDK -PathType Container)) {
        throw "VULKAN_SDK does not point at a directory: $env:VULKAN_SDK"
    }

    $sdkBin = Join-Path $env:VULKAN_SDK "Bin"
    if (Test-Path -LiteralPath $sdkBin -PathType Container) {
        $env:Path = "$sdkBin;$env:Path"
    }

    Require-Command "glslc" "Install the LunarG Vulkan SDK and ensure its Bin directory is on PATH."
    Write-Host "Vulkan: using SDK at $env:VULKAN_SDK"
}

function Get-NewestVulkanSdk {
    $root = "C:\VulkanSDK"
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        return $null
    }

    $sdk = Get-ChildItem -LiteralPath $root -Directory |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $sdk) {
        return $null
    }
    return $sdk.FullName
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

function Test-CrispAsrSubmoduleReady {
    $manifest = Join-Path $repo "vendor\CrispASR\crispasr\Cargo.toml"
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
        return $false
    }

    if (-not (Test-Command "git")) {
        return $true
    }

    $status = & git -C $repo submodule status --recursive "vendor/CrispASR" 2>$null
    if ($LASTEXITCODE -ne 0 -or $null -eq $status) {
        return $true
    }

    foreach ($line in @($status)) {
        if ($line.StartsWith("-") -or $line.StartsWith("+") -or $line.StartsWith("U")) {
            return $false
        }
    }

    return $true
}

function Assert-CrispAsrSubmoduleReady {
    if (Test-CrispAsrSubmoduleReady) {
        return
    }

    throw "CrispASR submodule is missing or not at the pinned revision. Use a checkout/source archive with vendor\CrispASR populated, or run git submodule update --init --recursive on a network that can reach the submodule remote."
}

Assert-NativeWindows

Require-Command "cargo" "Install Rust with rustup using the MSVC toolchain."
Require-Command "rustc" "Install Rust with rustup using the MSVC toolchain."
Require-Command "cmake" "Install CMake and ensure it is on PATH."

$repo = Get-RepoRoot
Set-Location $repo

if ($NoSubmodules) {
    Assert-CrispAsrSubmoduleReady
    Write-Host "Submodules: using existing checkout"
} elseif (Test-CrispAsrSubmoduleReady) {
    Write-Host "Submodules: ready"
} else {
    Require-Command "git" "Install Git for Windows and ensure it is on PATH, or use --no-submodules with vendor\CrispASR already populated."
    Write-Host "Updating submodules"
    Invoke-Checked "git" "submodule" "update" "--init" "--recursive"
    Assert-CrispAsrSubmoduleReady
}

switch ($Flavor) {
    "cuda" {
        Assert-CudaBuildReady
        if ($BundleCudaDlls) {
            $env:PARAKIT_BUNDLE_CUDA_DLLS = "1"
            Write-Host "CUDA: cuBLAS runtime DLL bundling enabled"
        } else {
            Remove-Item Env:\PARAKIT_BUNDLE_CUDA_DLLS -ErrorAction SilentlyContinue
            Write-Host "CUDA: cuBLAS runtime DLLs expected from CUDA_PATH\bin or PATH at install/run time"
        }
    }
    "vulkan" {
        Assert-VulkanBuildReady
        Remove-Item Env:\PARAKIT_BUNDLE_CUDA_DLLS -ErrorAction SilentlyContinue
    }
    default {
        Remove-Item Env:\PARAKIT_BUNDLE_CUDA_DLLS -ErrorAction SilentlyContinue
    }
}

Write-Host "Building $Profile ($Flavor)"
$cargoArgs = @("build", "--locked")
if ($Profile -eq "release") {
    $cargoArgs += "--release"
}
if ($Flavor -ne "cpu") {
    $cargoArgs += @("--features", $Flavor)
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

$bundleDir = Join-Path $targetRoot "parakit-windows-x86_64-$Flavor"
Assert-ChildPath -Child $bundleDir -Parent $targetRoot

if (Test-Path -LiteralPath $bundleDir) {
    Remove-Item -LiteralPath $bundleDir -Recurse -Force
}
New-Item -ItemType Directory -Path $bundleDir | Out-Null

Write-Host "Bundle: $bundleDir"
Copy-Item -LiteralPath $runtimeManifest -Destination $bundleDir -Force

$manifest = Get-Content -LiteralPath $runtimeManifest -Raw | ConvertFrom-Json
foreach ($required in @($manifest.required_files)) {
    if (
        [string]::IsNullOrWhiteSpace($required) -or
        [System.IO.Path]::IsPathRooted($required) -or
        $required.Contains("/") -or
        $required.Contains("\") -or
        $required.Contains("..")
    ) {
        throw "Runtime manifest required file must be a flat file name: $required"
    }
    $source = Join-Path $profileDir $required
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Runtime manifest required file was not produced: $required"
    }
    Copy-Item -LiteralPath $source -Destination $bundleDir -Force
}

Copy-IfExists -Path (Join-Path $repo "LICENSE") -Destination $bundleDir
Copy-IfExists -Path (Join-Path $repo "README.md") -Destination $bundleDir

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

}

if ($NoInstall) {
    Write-Host "Install: skipped"
}
