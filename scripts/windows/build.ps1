# Build Parakit daemon backends on native Windows.
#
# Usage:
#   powershell -ExecutionPolicy RemoteSigned -File scripts/windows/build.ps1 [options]
#
# By default this builds a repo-local bundle, installs it to the per-user
# Windows app directory, and adds that directory to the User PATH.
#
# One compute backend is supported per bundle. CUDA requires a local CUDA
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
$Backend = "cpu"
$BackendExplicit = $false
$Blas = $null
$OpenBlasRoot = $null
$BundleCudaDlls = $false
$AllowBackendSwitch = $false

function Show-Usage {
    $entryPoint = "scripts\windows\build.ps1"

    Write-Host "Build Parakit daemon backends on native Windows."
    Write-Host ""
    Write-Host "Usage:"
    Write-Host "  $entryPoint [--backend cpu|cuda|vulkan] [--blas auto|off|openblas|mkl|generic] [--openblas-root DIR] [--bundle-cuda-dlls] [--release|--debug|-Profile release|debug] [--no-submodules] [--no-install] [--no-user-path] [--allow-backend-switch|--force] [--install-dir DIR]"
    Write-Host ""
    Write-Host "Options:"
    Write-Host "  --backend        Build backend: cpu, cuda, or vulkan. If omitted, an interactive selector opens; Enter selects CPU."
    Write-Host "  --cpu            Alias for --backend cpu."
    Write-Host "  --cuda           Alias for --backend cuda. Requires NVIDIA CUDA Toolkit on this machine."
    Write-Host "  --vulkan         Alias for --backend vulkan. Requires LunarG Vulkan SDK and glslc."
    Write-Host "  --blas           Override CPU BLAS selection for this build: auto, off, openblas, mkl, or generic."
    Write-Host "  --openblas-root  Windows OpenBLAS prefix containing include, lib, and bin. Sets PARAKIT_OPENBLAS_ROOT."
    Write-Host "  --bundle-cuda-dlls"
    Write-Host "                   CUDA only: copy cudart64_*.dll, cublas64_*.dll, and cublasLt64_*.dll into the bundle."
    Write-Host "  --release        Build target\release and bundle it. This is the default."
    Write-Host "  --debug          Build target\debug and bundle it into the same target bundle."
    Write-Host "  -Profile         PowerShell-compatible profile selector: release or debug."
    Write-Host "  --no-submodules  Do not run git submodule update --init --recursive."
    Write-Host "  --no-install     Build the repo-local bundle without installing it."
    Write-Host "  --no-user-path   Install without adding the install directory to User PATH."
    Write-Host "  --allow-backend-switch"
    Write-Host "  --force"
    Write-Host "                   Allow replacing an installed cpu/cuda/vulkan backend with a different backend."
    Write-Host "  --install-dir    Install to DIR instead of `%LOCALAPPDATA`%\Programs\parakit."
    Write-Host "  -h, --help       Print this help."
}

function Set-BuildBackend {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("cpu", "cuda", "vulkan")]
        [string]$Value
    )

    if ($BackendExplicit -and $Backend -ne $Value) {
        throw "Only one build backend can be selected per bundle. Choose cpu, cuda, or vulkan."
    }
    $script:Backend = $Value
    $script:BackendExplicit = $true
}

function Set-BlasMode {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("auto", "off", "openblas", "mkl", "generic")]
        [string]$Value
    )

    $script:Blas = $Value
}

function Get-BackendOptions {
    return @(
        [pscustomobject]@{
            Value = "cpu"
            Label = "CPU"
            Description = "native CPU build; BLAS auto-detected unless --blas overrides it"
        },
        [pscustomobject]@{
            Value = "cuda"
            Label = "CUDA"
            Description = "NVIDIA CUDA Toolkit backend"
        },
        [pscustomobject]@{
            Value = "vulkan"
            Label = "Vulkan"
            Description = "Vulkan GPU backend for NVIDIA, AMD, or Intel drivers"
        }
    )
}

function ConvertTo-BackendSelection {
    param(
        [AllowNull()]
        [string]$Selection,

        [Parameter(Mandatory = $true)]
        [object[]]$Options
    )

    if ([string]::IsNullOrWhiteSpace($Selection)) {
        return "cpu"
    }

    $normalized = $Selection.Trim().ToLowerInvariant()
    if ($normalized -match '^[1-3]$') {
        $index = [int]$normalized - 1
        return $Options[$index].Value
    }

    foreach ($option in $Options) {
        if ($normalized -eq $option.Value -or $normalized -eq $option.Label.ToLowerInvariant()) {
            return $option.Value
        }
    }

    throw "Invalid build backend selection: $Selection. Choose 1, 2, 3, cpu, cuda, or vulkan."
}

function Write-BackendMenuLine {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Text,

        [switch]$Selected
    )

    $line = $Text
    try {
        $width = [Math]::Max(1, [Console]::BufferWidth - 1)
        if ($line.Length -gt $width) {
            $line = $line.Substring(0, $width)
        } else {
            $line = $line.PadRight($width)
        }
    } catch {
        $line = $Text
    }

    if ($Selected) {
        Write-Host $line -ForegroundColor Black -BackgroundColor Gray
    } else {
        Write-Host $line
    }
}

function Show-BackendMenu {
    $options = Get-BackendOptions

    if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) {
        Write-Host "Select Windows build backend"
        for ($index = 0; $index -lt $options.Count; $index++) {
            $option = $options[$index]
            Write-Host ("  {0}. {1,-6} {2}" -f ($index + 1), $option.Label, $option.Description)
        }
        Write-Host "Backend [1=CPU default, 2=CUDA, 3=Vulkan]: " -NoNewline
        return ConvertTo-BackendSelection -Selection ([Console]::In.ReadLine()) -Options $options
    }

    $selected = 0
    $top = [Console]::CursorTop
    while ($true) {
        [Console]::SetCursorPosition(0, $top)
        Write-BackendMenuLine "Select Windows build backend"
        Write-BackendMenuLine ""
        for ($index = 0; $index -lt $options.Count; $index++) {
            $option = $options[$index]
            $marker = if ($index -eq $selected) { ">" } else { " " }
            $line = (" {0} {1}. {2,-6} {3}" -f $marker, ($index + 1), $option.Label, $option.Description)
            Write-BackendMenuLine $line -Selected:($index -eq $selected)
        }
        Write-BackendMenuLine ""
        Write-BackendMenuLine "Enter = selected (CPU default). Up/Down = move. 1-3 = select. Esc = cancel."

        $key = [Console]::ReadKey($true)
        switch ($key.Key) {
            "UpArrow" {
                $selected = ($selected + $options.Count - 1) % $options.Count
            }
            "DownArrow" {
                $selected = ($selected + 1) % $options.Count
            }
            "Enter" {
                Write-Host ""
                return $options[$selected].Value
            }
            "Escape" {
                throw "Build backend selection cancelled."
            }
            default {
                if ($key.KeyChar -match '^[1-3]$') {
                    Write-Host ""
                    return ConvertTo-BackendSelection -Selection ([string]$key.KeyChar) -Options $options
                }
            }
        }
    }
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
        '^(--backend|-backend|-Backend)$' {
            $i++
            if ($i -ge $RawArgs.Count -or $RawArgs[$i] -notin @("cpu", "cuda", "vulkan")) {
                throw "$($RawArgs[$i - 1]) requires one of: cpu, cuda, vulkan"
            }
            Set-BuildBackend $RawArgs[$i]
        }
        '^(--cpu|-cpu|-Cpu)$' {
            Set-BuildBackend "cpu"
        }
        '^(--cuda|-cuda|-Cuda)$' {
            Set-BuildBackend "cuda"
        }
        '^(--vulkan|-vulkan|-Vulkan)$' {
            Set-BuildBackend "vulkan"
        }
        '^(--blas|-blas|-Blas)$' {
            $i++
            if ($i -ge $RawArgs.Count -or $RawArgs[$i] -notin @("auto", "off", "openblas", "mkl", "generic")) {
                throw "$($RawArgs[$i - 1]) requires one of: auto, off, openblas, mkl, generic"
            }
            Set-BlasMode $RawArgs[$i]
        }
        '^(--openblas-root|-openblas-root|-OpenBlasRoot)$' {
            $i++
            if ($i -ge $RawArgs.Count -or [string]::IsNullOrWhiteSpace($RawArgs[$i])) {
                throw "$($RawArgs[$i - 1]) requires a directory argument"
            }
            $OpenBlasRoot = $RawArgs[$i]
        }
        '^(--bundle-cuda-dlls|-bundle-cuda-dlls|-BundleCudaDlls)$' {
            $BundleCudaDlls = $true
        }
        '^(--debug|-debug|-Profile|-profile)$' {
            if ($RawArgs[$i] -ieq "-Profile") {
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
        '^(--allow-backend-switch|-allow-backend-switch|-AllowBackendSwitch|--force|-force|-Force)$' {
            $AllowBackendSwitch = $true
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

if (-not $BackendExplicit) {
    $Backend = Show-BackendMenu
}
Write-Host "Backend: $Backend"

if ($BundleCudaDlls -and $Backend -ne "cuda") {
    throw "--bundle-cuda-dlls is only valid with --backend cuda or --cuda."
}

if ($NoInstall -and ($NoUserPath -or $AllowBackendSwitch -or -not [string]::IsNullOrWhiteSpace($InstallDir))) {
    throw "--no-user-path, --allow-backend-switch/--force, and --install-dir only apply when installing. Remove them when using --no-install."
}

if (-not [string]::IsNullOrWhiteSpace($OpenBlasRoot) -and
    -not [string]::IsNullOrWhiteSpace($Blas) -and
    $Blas -notin @("auto", "openblas")) {
    throw "--openblas-root only applies with --blas auto or --blas openblas."
}

if (-not [string]::IsNullOrWhiteSpace($env:CRISPASR_LIB_DIR)) {
    throw "Windows builds from this script require the bundled CrispASR staging path so runtime DLLs and parakit-runtime-manifest.json are produced. Unset CRISPASR_LIB_DIR before running this script."
}

function Test-NinjaGenerator {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Generator
    )

    return $Generator -like "Ninja*"
}

function Configure-GpuBuildGenerator {
    if ($Backend -eq "cpu") {
        return
    }

    if ([string]::IsNullOrWhiteSpace($env:CMAKE_GENERATOR)) {
        $env:CMAKE_GENERATOR = "Ninja"
        Write-Host "${Backend}: CMAKE_GENERATOR was not set; defaulting to Ninja"
    } else {
        Write-Host "${Backend}: using CMAKE_GENERATOR=$env:CMAKE_GENERATOR"
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

function Configure-BlasSelection {
    if (-not [string]::IsNullOrWhiteSpace($OpenBlasRoot)) {
        $root = Get-FullPath $OpenBlasRoot
        if (-not (Test-Path -LiteralPath $root -PathType Container)) {
            throw "--openblas-root does not point to a directory: $root"
        }
        $env:PARAKIT_OPENBLAS_ROOT = $root
        Write-Host "BLAS: PARAKIT_OPENBLAS_ROOT=$env:PARAKIT_OPENBLAS_ROOT"
    } elseif (-not [string]::IsNullOrWhiteSpace($env:PARAKIT_OPENBLAS_ROOT)) {
        Write-Host "BLAS: using PARAKIT_OPENBLAS_ROOT=$env:PARAKIT_OPENBLAS_ROOT"
    }

    if (-not [string]::IsNullOrWhiteSpace($Blas)) {
        $env:PARAKIT_BLAS = $Blas
        Write-Host "BLAS: PARAKIT_BLAS=$env:PARAKIT_BLAS"
    } elseif (-not [string]::IsNullOrWhiteSpace($env:PARAKIT_BLAS)) {
        Write-Host "BLAS: using PARAKIT_BLAS=$env:PARAKIT_BLAS"
    } else {
        Write-Host "BLAS: auto-detecting; pass --blas to override"
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
    $sample = Get-VulkanShaderObjectPathSample -RepoRoot $repo
    if ($sample.Length -ge 240) {
        Write-Warning "Vulkan: estimated shader build object path is $($sample.Length) characters. ggml-vulkan can exceed Windows path limits from deep checkouts; set CARGO_TARGET_DIR to a short absolute user-writable path such as `$env:USERPROFILE\parakit-target, or build from a shorter checkout such as C:\src\parakit."
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
    if ($Backend -ne "vulkan") {
        return
    }

    Set-DefaultVulkanCargoTargetDirIfNeeded

    $sample = Get-VulkanShaderObjectPathSample -RepoRoot $repo
    if ($sample.Length -lt 250) {
        return
    }

    throw "Vulkan shader build paths are estimated at $($sample.Length) characters, which exceeds CMake's practical MSVC object path limit. Set CARGO_TARGET_DIR to a shorter absolute user-writable path, or clone/build from a shorter path, then rerun the build. The script does not map temporary drive letters automatically because managed Windows environments can block that behavior."
}

function Set-DefaultVulkanCargoTargetDirIfNeeded {
    if (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        return
    }

    $repoTargetSample = Get-VulkanShaderObjectPathSample -RepoRoot $repo
    if ($repoTargetSample.Length -lt 250) {
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

function Get-VulkanShaderObjectPathSample {
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
    return [pscustomobject]@{
        Path = $samplePath
        Length = $samplePath.Length
    }
}

function Get-DefaultInstallDir {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return (Join-Path $env:USERPROFILE "AppData\Local\Programs\parakit")
    }

    return (Join-Path $env:LOCALAPPDATA "Programs\parakit")
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

Configure-BlasSelection
Configure-GpuBuildGenerator

switch ($Backend) {
    "cuda" {
        Assert-CudaBuildReady
        if ($BundleCudaDlls) {
            $env:PARAKIT_BUNDLE_CUDA_DLLS = "1"
            Write-Host "CUDA: runtime DLL bundling enabled"
        } else {
            Remove-Item Env:\PARAKIT_BUNDLE_CUDA_DLLS -ErrorAction SilentlyContinue
            Write-Host "CUDA: runtime DLLs expected from the installed app directory or PATH at install/run time"
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

Write-Host "Building $Profile ($Backend)"
$cargoArgs = @("build", "--locked")
if ($Profile -eq "release") {
    $cargoArgs += "--release"
}
if ($Backend -ne "cpu") {
    $cargoArgs += @("--features", $Backend)
}
Clear-StaleCMakePathAliasCaches
Invoke-Checked "cargo" @cargoArgs

$cargoTargetRoot = Get-CargoTargetRoot
$profileDir = Join-Path $cargoTargetRoot $Profile
$exe = Join-Path $profileDir "parakit.exe"
$runtimeManifest = Join-Path $profileDir "parakit-runtime-manifest.json"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "parakit.exe was not produced at $exe"
}

if (-not (Test-Path -LiteralPath $runtimeManifest -PathType Leaf)) {
    throw "Runtime manifest was not produced at $runtimeManifest"
}

$bundleRoot = Join-Path $repo "target"
if (-not (Test-Path -LiteralPath $bundleRoot)) {
    New-Item -ItemType Directory -Path $bundleRoot | Out-Null
}

$bundleDir = Join-Path $bundleRoot "parakit-windows-x86_64-$Backend"
Assert-ChildPath -Child $bundleDir -Parent $bundleRoot

if (Test-Path -LiteralPath $bundleDir) {
    Remove-Item -LiteralPath $bundleDir -Recurse -Force
}
New-Item -ItemType Directory -Path $bundleDir | Out-Null

Write-Host "Bundle: $bundleDir"
Copy-Item -LiteralPath $runtimeManifest -Destination $bundleDir -Force

$manifest = Get-Content -LiteralPath $runtimeManifest -Raw | ConvertFrom-Json
foreach ($required in @($manifest.required_files)) {
    Assert-FlatBundleFileName -Name $required -Context "Runtime manifest required file"
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

    $installer = Join-Path $repo "scripts\windows\install.ps1"

    & $installer `
        -BundleDir $bundleDir `
        -InstallDir $InstallDir `
        -NoUserPath:$NoUserPath `
        -AllowBackendSwitch:$AllowBackendSwitch
    if (-not $?) {
        throw "Windows install failed"
    }

}

if ($NoInstall) {
    Write-Host "Install: skipped"
}
