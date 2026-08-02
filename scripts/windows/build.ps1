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
    # Keep one catch-all so direct PowerShell calls such as `-Profile debug`
    # and cmd-style calls such as `--backend vulkan` both reach the same parser.
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RawArgs
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $PSCommandPath
. (Join-Path $scriptDir "common.ps1")
. (Join-Path $scriptDir "toolchains.ps1")

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
    Write-Host "  $entryPoint [--backend cpu|cuda|vulkan] [--blas auto|off|openblas|mkl|generic] [--openblas-root DIR] [--bundle-cuda-dlls] [--release|-Profile release|debug] [--no-submodules] [--no-install] [--no-user-path] [--allow-backend-switch|--force] [--install-dir DIR]"
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
    Write-Host "  -Profile         Profile selector: release or debug. Use -Profile debug for target\debug."
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
            $Blas = $RawArgs[$i]
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

$repo = (Resolve-Path (Join-Path $scriptDir "..\..")).Path
Set-Location $repo
Set-BundleCargoTargetDir
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
$previousCmakeGenerator = $env:CMAKE_GENERATOR
try {
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
    $cargoTargetRoot = Get-CargoTargetRoot
    $cargoArgs = @("build", "--locked", "--target-dir", $cargoTargetRoot)
    if ($Profile -eq "release") {
        $cargoArgs += "--release"
    }
    if ($Backend -ne "cpu") {
        $cargoArgs += @("--features", $Backend)
    }
    Clear-StaleCMakePathAliasCaches
    Invoke-Checked "cargo" @cargoArgs
} finally {
    if ([string]::IsNullOrWhiteSpace($previousCmakeGenerator)) {
        Remove-Item Env:\CMAKE_GENERATOR -ErrorAction SilentlyContinue
    } else {
        $env:CMAKE_GENERATOR = $previousCmakeGenerator
    }
}

$profileDir = Join-Path $cargoTargetRoot $Profile
$exe = Join-Path $profileDir "parakit.exe"
$runtimeManifest = Join-Path $profileDir "parakit-runtime-manifest.json"

if (-not (Test-Path -LiteralPath $exe)) {
    throw "parakit.exe was not produced at $exe"
}

if (-not (Test-Path -LiteralPath $runtimeManifest -PathType Leaf)) {
    throw "Runtime manifest was not produced at $runtimeManifest"
}

$manifest = Get-Content -LiteralPath $runtimeManifest -Raw | ConvertFrom-Json
if ($manifest.accelerator -ine $Backend) {
    throw "Runtime manifest accelerator '$($manifest.accelerator)' does not match requested backend '$Backend'"
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
