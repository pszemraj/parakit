# Build and bundle Parakit daemon flavors on native Windows.
#
# Usage:
#   powershell -ExecutionPolicy RemoteSigned -File scripts/windows/windows-cpu-build.ps1 [options]
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

$scriptDir = Split-Path -Parent $PSCommandPath
. (Join-Path $scriptDir "common.ps1")

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

function Test-NinjaGenerator {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Generator
    )

    return $Generator -like "Ninja*"
}

function Configure-GpuBuildGenerator {
    if ($Flavor -eq "cpu") {
        return
    }

    if ([string]::IsNullOrWhiteSpace($env:CMAKE_GENERATOR)) {
        $env:CMAKE_GENERATOR = "Ninja"
        Write-Host "${Flavor}: CMAKE_GENERATOR was not set; defaulting to Ninja"
    } else {
        Write-Host "${Flavor}: using CMAKE_GENERATOR=$env:CMAKE_GENERATOR"
    }

    if (Test-NinjaGenerator $env:CMAKE_GENERATOR) {
        Ensure-MsvcBuildEnvironment
        Ensure-NinjaAvailable
    }

    Configure-CompilerCache
}

function Ensure-MsvcBuildEnvironment {
    if ((Test-Command "cl.exe") -and (Test-Command "link.exe")) {
        Write-Host "MSVC: using active developer environment"
        return
    }

    $vsInstall = Get-VisualStudioInstallPath
    if ([string]::IsNullOrWhiteSpace($vsInstall)) {
        throw "Visual Studio C++ tools were not found. Install Visual Studio 2022 with the Desktop development with C++ workload."
    }

    $script:VisualStudioInstallPath = $vsInstall
    $launchDevShell = Join-Path $vsInstall "Common7\Tools\Launch-VsDevShell.ps1"
    $vcvars64 = Join-Path $vsInstall "VC\Auxiliary\Build\vcvars64.bat"
    $currentLocation = Get-Location
    $activated = $false

    try {
        if (Test-Path -LiteralPath $vcvars64 -PathType Leaf) {
            Import-EnvironmentFromBatch $vcvars64
            if ((Test-Command "cl.exe") -and (Test-Command "link.exe")) {
                $activated = $true
            } else {
                Write-Warning "MSVC: vcvars64.bat completed without exposing cl.exe and link.exe; falling back to Launch-VsDevShell.ps1."
            }
        }

        if (-not $activated -and (Test-Path -LiteralPath $launchDevShell -PathType Leaf)) {
            try {
                . $launchDevShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
                if ((Test-Command "cl.exe") -and (Test-Command "link.exe")) {
                    $activated = $true
                } else {
                    Write-Warning "MSVC: Launch-VsDevShell.ps1 completed without exposing cl.exe and link.exe."
                }
            } catch {
                Write-Warning "MSVC: Launch-VsDevShell.ps1 failed. $($_.Exception.Message)"
            }
        }

        if (-not $activated) {
            throw "Visual Studio install is missing usable Launch-VsDevShell.ps1 and vcvars64.bat: $vsInstall"
        }
    } finally {
        Set-Location $currentLocation
    }

    Add-VisualStudioNinjaToPath $vsInstall

    if (-not (Test-Command "cl.exe") -or -not (Test-Command "link.exe")) {
        throw "Visual Studio environment activation did not expose cl.exe and link.exe. Run from an x64 Native Tools shell or repair the Visual Studio C++ workload."
    }

    Write-Host "MSVC: activated amd64 developer environment from $vsInstall"
}

function Ensure-NinjaAvailable {
    if (Test-Command "ninja") {
        Write-Host "Ninja: using $(Get-Command ninja | Select-Object -ExpandProperty Source -First 1)"
        return
    }

    if ([string]::IsNullOrWhiteSpace($script:VisualStudioInstallPath)) {
        $script:VisualStudioInstallPath = Get-VisualStudioInstallPath
    }

    if (-not [string]::IsNullOrWhiteSpace($script:VisualStudioInstallPath)) {
        Add-VisualStudioNinjaToPath $script:VisualStudioInstallPath
    }

    Require-Command "ninja" "Install Ninja, or install Visual Studio's CMake tools so its bundled Ninja is available."
    Write-Host "Ninja: using $(Get-Command ninja | Select-Object -ExpandProperty Source -First 1)"
}

function Add-VisualStudioNinjaToPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$VsInstall
    )

    $ninjaDir = Join-Path $VsInstall "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja"
    $ninjaExe = Join-Path $ninjaDir "ninja.exe"
    if ((Test-Path -LiteralPath $ninjaExe -PathType Leaf) -and -not ($env:Path.Split(";") -contains $ninjaDir)) {
        $env:Path = "$ninjaDir;$env:Path"
    }
}

function Get-VisualStudioInstallPath {
    $vswhere = $null
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidate = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $vswhere = $candidate
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($vswhere)) {
        $path = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null |
            Select-Object -First 1
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($path)) {
            return $path
        }
    }

    $roots = @()
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $roots += Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\2022"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $roots += Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022"
    }

    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) {
            continue
        }

        $candidate = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName "VC\Auxiliary\Build\vcvars64.bat") -PathType Leaf } |
            Select-Object -First 1
        if ($null -ne $candidate) {
            return $candidate.FullName
        }
    }

    return $null
}

function Import-EnvironmentFromBatch {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BatchPath
    )

    $command = "`"$BatchPath`" >nul && set"
    $environment = & $env:ComSpec /d /s /c $command
    if ($LASTEXITCODE -ne 0) {
        throw "$BatchPath failed with exit code $LASTEXITCODE"
    }

    $values = New-Object "System.Collections.Generic.Dictionary[string,string]" ([System.StringComparer]::OrdinalIgnoreCase)
    $pathValues = @()

    foreach ($line in @($environment)) {
        if ($line -match '^([^=]+)=(.*)$') {
            $name = $matches[1]
            $value = $matches[2]
            if ($name -ieq "PATH") {
                $pathValues += $value
            } else {
                $values[$name] = $value
            }
        }
    }

    foreach ($name in $values.Keys) {
        [System.Environment]::SetEnvironmentVariable($name, $values[$name], "Process")
    }

    if ($pathValues.Count -gt 0) {
        $pathValue = $pathValues |
            Where-Object { $_ -like "*\VC\Tools\MSVC\*HostX64*x64*" } |
            Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($pathValue)) {
            $pathValue = $pathValues |
                Sort-Object { $_.Length } -Descending |
                Select-Object -First 1
        }

        [System.Environment]::SetEnvironmentVariable("Path", $pathValue, "Process")
        $env:Path = $pathValue
    }
}

function Configure-CompilerCache {
    if (Test-Command "ccache") {
        if ([string]::IsNullOrWhiteSpace($env:CCACHE_DIR)) {
            $env:CCACHE_DIR = Join-Path $repo "target\tmp\ccache"
            New-Item -ItemType Directory -Force -Path $env:CCACHE_DIR | Out-Null
            Write-Host "ccache: using repo-local cache at $env:CCACHE_DIR"
        } else {
            Write-Host "ccache: using CCACHE_DIR=$env:CCACHE_DIR"
        }

        if ([string]::IsNullOrWhiteSpace($env:CCACHE_TEMPDIR)) {
            $env:CCACHE_TEMPDIR = Join-Path $repo "target\tmp\ccache-tmp"
            New-Item -ItemType Directory -Force -Path $env:CCACHE_TEMPDIR | Out-Null
        }

        if ([string]::IsNullOrWhiteSpace($env:CCACHE_BASEDIR)) {
            $env:CCACHE_BASEDIR = $repo
        }
    }
}

function Assert-CudaBuildReady {
    Require-Command "nvcc" "Install the NVIDIA CUDA Toolkit and ensure nvcc is on PATH."
    Set-CudaPathFromNvccIfMissing

    if ([string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        throw "CUDA_PATH is not set. Install the NVIDIA CUDA Toolkit, or set CUDA_PATH to the toolkit root for this shell."
    }

    $cudaBin = Join-Path $env:CUDA_PATH "bin"
    if (-not (Test-Path -LiteralPath $cudaBin -PathType Container)) {
        throw "CUDA_PATH does not contain a bin directory: $env:CUDA_PATH"
    }

    if (-not (Test-NinjaGenerator $env:CMAKE_GENERATOR)) {
        Assert-CudaVisualStudioToolset
    }

    Write-Host "CUDA: using toolkit at $env:CUDA_PATH"
    Write-Host "CUDA: ggml-cuda first build can take tens of minutes; native/default arch keeps it to this machine."
}

function Set-CudaPathFromNvccIfMissing {
    if (-not [string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        return
    }

    $nvcc = Get-Command "nvcc" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $nvcc -or [string]::IsNullOrWhiteSpace($nvcc.Source)) {
        return
    }

    $nvccPath = $nvcc.Source
    if (-not (Test-Path -LiteralPath $nvccPath -PathType Leaf)) {
        return
    }

    $binDir = Split-Path -Parent $nvccPath
    if ((Split-Path -Leaf $binDir) -ine "bin") {
        return
    }

    $toolkitRoot = Split-Path -Parent $binDir
    if ([string]::IsNullOrWhiteSpace($toolkitRoot) -or -not (Test-Path -LiteralPath $toolkitRoot -PathType Container)) {
        return
    }

    $env:CUDA_PATH = $toolkitRoot
    Write-Host "CUDA: inferred CUDA_PATH=$env:CUDA_PATH from nvcc on PATH"
}

function Assert-CudaVisualStudioToolset {
    if (-not [string]::IsNullOrWhiteSpace($env:CMAKE_GENERATOR_TOOLSET) -and $env:CMAKE_GENERATOR_TOOLSET -match '(^|,)cuda=') {
        Write-Host "CUDA: using CMAKE_GENERATOR_TOOLSET=$env:CMAKE_GENERATOR_TOOLSET"
        return
    }

    $nvccVersion = Get-NvccReleaseVersion
    $integrationVersions = @(Get-CudaVisualStudioIntegrationVersions)

    if ($integrationVersions.Count -eq 0) {
        throw "CUDA Visual Studio integration was not found. Use the default Ninja generator, install the CUDA Visual Studio integration component, or set CMAKE_GENERATOR_TOOLSET=cuda=<toolkit-path>."
    }

    if ([string]::IsNullOrWhiteSpace($nvccVersion)) {
        Write-Warning "CUDA: could not parse nvcc release version; Visual Studio generator may select a different CUDA BuildCustomizations version."
        return
    }

    if ($integrationVersions -notcontains $nvccVersion) {
        $available = $integrationVersions -join ", "
        throw "CUDA: nvcc reports $nvccVersion, but Visual Studio CUDA BuildCustomizations contain [$available]. Use Ninja, remove stale CUDA *.props/*.targets, reinstall the matching CUDA Visual Studio integration, or set CMAKE_GENERATOR_TOOLSET=cuda=$env:CUDA_PATH."
    }

    $specificEnvName = "CUDA_PATH_V$($nvccVersion.Replace('.', '_'))"
    $specificEnvValue = [System.Environment]::GetEnvironmentVariable($specificEnvName, "Process")
    if ([string]::IsNullOrWhiteSpace($specificEnvValue)) {
        Write-Warning "CUDA: $specificEnvName is not set. Visual Studio CUDA targets may resolve an empty CudaToolkitDir; prefer Ninja or set CMAKE_GENERATOR_TOOLSET=cuda=$env:CUDA_PATH."
    }
}

function Get-NvccReleaseVersion {
    $nvcc = & nvcc --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    foreach ($line in @($nvcc)) {
        if ($line -match 'release\s+([0-9]+\.[0-9]+)') {
            return $matches[1]
        }
    }

    return $null
}

function Get-CudaVisualStudioIntegrationVersions {
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

    $versions = @()
    foreach ($root in $roots) {
        $propsFiles = Get-ChildItem -LiteralPath $root -Recurse -Filter "CUDA *.props" -ErrorAction SilentlyContinue
        foreach ($propsFile in @($propsFiles)) {
            if ($propsFile.BaseName -match '^CUDA\s+([0-9]+\.[0-9]+)') {
                $versions += $Matches[1]
            }
        }
    }

    return $versions | Sort-Object -Unique
}

function Assert-VulkanBuildReady {
    if ([string]::IsNullOrWhiteSpace($env:VULKAN_SDK)) {
        $detected = Get-NewestVulkanSdk
        if ([string]::IsNullOrWhiteSpace($detected)) {
            $detected = Get-VulkanSdkFromGlslc
        }
        if ([string]::IsNullOrWhiteSpace($detected)) {
            throw "VULKAN_SDK is not set and no Vulkan SDK install was found. Install the LunarG Vulkan SDK from vulkan.lunarg.com, use winget install KhronosGroup.VulkanSDK, or put glslc from a complete Vulkan SDK on PATH."
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
    Write-VulkanPathLengthWarnings
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

function Get-VulkanSdkFromGlslc {
    $glslc = Get-Command "glslc" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $glslc -or [string]::IsNullOrWhiteSpace($glslc.Source)) {
        return $null
    }

    $glslcPath = $glslc.Source
    if (-not (Test-Path -LiteralPath $glslcPath -PathType Leaf)) {
        return $null
    }

    $binDir = Split-Path -Parent $glslcPath
    if ((Split-Path -Leaf $binDir) -ine "bin") {
        return $null
    }

    $sdkRoot = Split-Path -Parent $binDir
    $header = Join-Path $sdkRoot "Include\vulkan\vulkan.h"
    $importLib = Join-Path $sdkRoot "Lib\vulkan-1.lib"
    if (
        (Test-Path -LiteralPath $header -PathType Leaf) -and
        (Test-Path -LiteralPath $importLib -PathType Leaf)
    ) {
        return $sdkRoot
    }

    return $null
}

function Get-CargoTargetRoot {
    if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        return (Join-Path $repo "target")
    }

    if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        return [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    }

    return [System.IO.Path]::GetFullPath((Join-Path $repo $env:CARGO_TARGET_DIR))
}

function Clear-StaleCMakePathAliasCaches {
    $targetRoot = Get-CargoTargetRoot
    $profileBuildRoot = Join-Path $targetRoot "$Profile\build"
    if (-not (Test-Path -LiteralPath $profileBuildRoot -PathType Container)) {
        return
    }

    Get-ChildItem -LiteralPath $profileBuildRoot -Directory -Filter "parakit-*" | ForEach-Object {
        $outDir = Join-Path $_.FullName "out"
        $buildDir = Join-Path $outDir "build"
        $cachePath = Join-Path $buildDir "CMakeCache.txt"
        if (-not (Test-Path -LiteralPath $cachePath -PathType Leaf)) {
            return
        }

        $cachedDir = Get-Content -LiteralPath $cachePath |
            Where-Object { $_ -like "CMAKE_CACHEFILE_DIR:INTERNAL=*" } |
            Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($cachedDir)) {
            return
        }

        $cachedDir = ConvertTo-ComparablePath $cachedDir.Substring("CMAKE_CACHEFILE_DIR:INTERNAL=".Length)
        $expectedDir = ConvertTo-ComparablePath ([System.IO.Path]::GetFullPath($buildDir))
        if ([string]::Equals($cachedDir, $expectedDir, [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }

        Assert-ChildPath -Child $outDir -Parent $targetRoot
        Write-Host "CMake: removing stale build cache from $outDir (cached path was $cachedDir)"
        Remove-Item -LiteralPath $outDir -Recurse -Force
    }
}

function ConvertTo-ComparablePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return $Path.Trim().Replace("/", "\").TrimEnd("\")
}

function Get-LongPathsEnabled {
    try {
        $value = Get-ItemPropertyValue -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name "LongPathsEnabled" -ErrorAction Stop
        return [int]$value
    } catch {
        return $null
    }
}

function Write-VulkanPathLengthWarnings {
    $targetRoot = Get-CargoTargetRoot
    $samplePath = Join-Path $targetRoot "$Profile\build\parakit-0000000000000000\out\build\vendor\CrispASR\ggml\src\ggml-vulkan\CMakeFiles\vulkan-shaders-gen.dir\vulkan-shaders\matmul_id_subgroup_q6_k_f32_f16acc_aligned_c00.cxx.obj"
    if ($samplePath.Length -ge 240) {
        Write-Warning "Vulkan: estimated shader build object path is $($samplePath.Length) characters. ggml-vulkan can exceed Windows path limits from deep checkouts; set CARGO_TARGET_DIR to a short absolute user-writable path such as `$env:USERPROFILE\parakit-target, or build from a shorter checkout such as C:\src\parakit."
    }

    $longPathsEnabled = Get-LongPathsEnabled
    if ($null -eq $longPathsEnabled) {
        Write-Warning "Vulkan: could not read LongPathsEnabled. If shader generation fails with path or PDB errors, enable Windows long paths or shorten CARGO_TARGET_DIR."
    } elseif ($longPathsEnabled -ne 1) {
        Write-Warning "Vulkan: Windows long paths are disabled. If shader generation fails with path or PDB errors, enable LongPathsEnabled or shorten CARGO_TARGET_DIR."
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

function Assert-VulkanBuildPathLength {
    if ($Flavor -ne "vulkan") {
        return
    }

    Set-DefaultVulkanCargoTargetDirIfNeeded

    $estimatedLength = Get-VulkanShaderObjectPathEstimate -RepoRoot $repo
    if ($estimatedLength -lt 250) {
        return
    }

    throw "Vulkan shader build paths are estimated at $estimatedLength characters, which exceeds CMake's practical MSVC object path limit. Set CARGO_TARGET_DIR to a shorter absolute user-writable path, or clone/build from a shorter path, then rerun the build. The script does not map temporary drive letters automatically because managed Windows environments can block that behavior."
}

function Set-DefaultVulkanCargoTargetDirIfNeeded {
    if (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        return
    }

    $repoTargetEstimate = Get-VulkanShaderObjectPathEstimate -RepoRoot $repo
    if ($repoTargetEstimate -lt 250) {
        return
    }

    $defaultTarget = Get-DefaultVulkanCargoTargetDir
    $env:CARGO_TARGET_DIR = $defaultTarget
    Write-Host "Vulkan: CARGO_TARGET_DIR was not set; using short target dir $env:CARGO_TARGET_DIR"
}

function Get-DefaultVulkanCargoTargetDir {
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return [System.IO.Path]::GetFullPath((Join-Path $env:USERPROFILE "parakit-target"))
    }

    return [System.IO.Path]::GetFullPath((Join-Path $repo "target"))
}

function Get-VulkanShaderObjectPathEstimate {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepoRoot
    )

    $targetRoot = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        Join-Path $RepoRoot "target"
    } elseif ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        Join-Path $RepoRoot $env:CARGO_TARGET_DIR
    }

    $samplePath = Join-Path $targetRoot "$Profile\build\parakit-0000000000000000\out\build\ggml\src\ggml-vulkan\vulkan-shaders-gen-prefix\src\vulkan-shaders-gen-build\CMakeFiles\CMakeScratch\TryCompile-000000\CMakeFiles\cmTC_00000.dir\testCCompiler.c.obj"
    return $samplePath.Length
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

    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $status = & git -C $repo submodule status --recursive "vendor/CrispASR" 2>$null
        if ($LASTEXITCODE -ne 0 -or $null -eq $status) {
            return $true
        }
    } catch {
        return $true
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
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

function Invoke-GitSubmoduleUpdate {
    $previousGitPrompt = $env:GIT_TERMINAL_PROMPT
    $previousGcmInteractive = $env:GCM_INTERACTIVE
    try {
        $env:GIT_TERMINAL_PROMPT = "0"
        $env:GCM_INTERACTIVE = "Never"
        Invoke-Checked "git" "submodule" "update" "--init" "--recursive"
    } finally {
        if ([string]::IsNullOrWhiteSpace($previousGitPrompt)) {
            Remove-Item Env:\GIT_TERMINAL_PROMPT -ErrorAction SilentlyContinue
        } else {
            $env:GIT_TERMINAL_PROMPT = $previousGitPrompt
        }
        if ([string]::IsNullOrWhiteSpace($previousGcmInteractive)) {
            Remove-Item Env:\GCM_INTERACTIVE -ErrorAction SilentlyContinue
        } else {
            $env:GCM_INTERACTIVE = $previousGcmInteractive
        }
    }
}

Assert-NativeWindows

Require-Command "cargo" "Install Rust with rustup using the MSVC toolchain."
Require-Command "rustc" "Install Rust with rustup using the MSVC toolchain."
Require-Command "cmake" "Install CMake and ensure it is on PATH."

$repo = Get-RepoRoot
Set-Location $repo
Assert-VulkanBuildPathLength

if ($NoSubmodules) {
    Assert-CrispAsrSubmoduleReady
    Write-Host "Submodules: using existing checkout"
} elseif (Test-CrispAsrSubmoduleReady) {
    Write-Host "Submodules: ready"
} else {
    Require-Command "git" "Install Git for Windows and ensure it is on PATH, or use --no-submodules with vendor\CrispASR already populated."
    Write-Host "Updating submodules (non-interactive)"
    Invoke-GitSubmoduleUpdate
    Assert-CrispAsrSubmoduleReady
}

Configure-GpuBuildGenerator

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
Clear-StaleCMakePathAliasCaches
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
