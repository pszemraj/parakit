# Native compiler, BLAS, CUDA, and Vulkan setup used by build.ps1.

function Configure-GpuBuildGenerator {
    if ($Backend -eq "cpu") {
        return
    }

    if ($env:CMAKE_GENERATOR -ne "Ninja") {
        $env:CMAKE_GENERATOR = "Ninja"
        Write-Host "${Backend}: using CMAKE_GENERATOR=Ninja"
    }

    Ensure-MsvcBuildEnvironment
    Ensure-NinjaAvailable
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

function Configure-BlasSelection {
    $openBlasRootApplies = [string]::IsNullOrWhiteSpace($Blas) -or $Blas -in @("auto", "openblas")
    if ($openBlasRootApplies -and -not [string]::IsNullOrWhiteSpace($OpenBlasRoot)) {
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
    if ([string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        Require-Command "nvcc" "Install the NVIDIA CUDA Toolkit and ensure nvcc is on PATH."
        Set-CudaPathFromNvccIfMissing
    }

    if ([string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        throw "CUDA_PATH is not set. Install the NVIDIA CUDA Toolkit, or set CUDA_PATH to the toolkit root for this shell."
    }

    $cudaBin = Join-Path $env:CUDA_PATH "bin"
    if (-not (Test-Path -LiteralPath $cudaBin -PathType Container)) {
        throw "CUDA_PATH does not contain a bin directory: $env:CUDA_PATH"
    }

    $currentPath = [string]$env:Path
    if (-not (($currentPath -split ";") -contains $cudaBin)) {
        $env:Path = if ([string]::IsNullOrWhiteSpace($currentPath)) {
            $cudaBin
        } else {
            "$cudaBin;$currentPath"
        }
    }
    Require-Command "nvcc" "CUDA_PATH does not expose nvcc.exe under its bin directory: $env:CUDA_PATH"

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

function Assert-VulkanBuildReady {
    if ([string]::IsNullOrWhiteSpace($env:VULKAN_SDK)) {
        $detected = Get-VulkanSdkFromGlslc
        if ([string]::IsNullOrWhiteSpace($detected)) {
            $detected = Get-NewestVulkanSdk
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
}

function Get-NewestVulkanSdk {
    $root = "C:\VulkanSDK"
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        return $null
    }

    $sdk = Get-ChildItem -LiteralPath $root -Directory |
        Where-Object { $null -ne ($_.Name -as [version]) } |
        Sort-Object { [version]$_.Name } -Descending |
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
    if (-not [string]::IsNullOrWhiteSpace($script:BundleCargoTargetRoot)) {
        return [System.IO.Path]::GetFullPath($script:BundleCargoTargetRoot)
    }

    if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
        return (Join-Path $repo "target")
    }

    if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        return [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    }

    return [System.IO.Path]::GetFullPath((Join-Path $repo $env:CARGO_TARGET_DIR))
}

function Set-BundleCargoTargetDir {
    if ($Backend -eq "vulkan") {
        Set-DefaultVulkanCargoTargetDirIfNeeded
    }

    $baseTargetRoot = Get-CargoTargetRoot
    $script:BundleCargoTargetRoot = [System.IO.Path]::GetFullPath((Join-Path $baseTargetRoot $Backend))
    Write-Host "Cargo target: $script:BundleCargoTargetRoot"
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

function Assert-VulkanBuildPathLength {
    if ($Backend -ne "vulkan") {
        return
    }

    Set-DefaultVulkanCargoTargetDirIfNeeded

    $sample = Get-VulkanShaderObjectPathSample
    if ($sample.Length -lt 250) {
        return
    }

    throw "Vulkan shader build paths are estimated at $($sample.Length) characters, which exceeds CMake's practical MSVC object path limit. Set CARGO_TARGET_DIR to a shorter absolute user-writable path, or clone/build from a shorter path, then rerun the build. The script does not map temporary drive letters automatically because managed Windows environments can block that behavior."
}

function Set-DefaultVulkanCargoTargetDirIfNeeded {
    if (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR) -or
        -not [string]::IsNullOrWhiteSpace($script:BundleCargoTargetRoot)) {
        return
    }

    $repoTargetSample = Get-VulkanShaderObjectPathSample
    if ($repoTargetSample.Length -lt 250) {
        return
    }

    $defaultTarget = Get-DefaultVulkanCargoTargetDir
    $script:BundleCargoTargetRoot = $defaultTarget
    Write-Host "Vulkan: CARGO_TARGET_DIR was not set; using short target dir $script:BundleCargoTargetRoot"
}

function Get-DefaultVulkanCargoTargetDir {
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return [System.IO.Path]::GetFullPath((Join-Path $env:USERPROFILE "parakit-target"))
    }

    return [System.IO.Path]::GetFullPath((Join-Path $repo "target"))
}

function Get-VulkanShaderObjectPathSample {
    $targetRoot = Get-CargoTargetRoot

    $samplePath = Join-Path $targetRoot "$Profile\build\parakit-0000000000000000\out\build\ggml\src\ggml-vulkan\vulkan-shaders-gen-prefix\src\vulkan-shaders-gen-build\CMakeFiles\CMakeScratch\TryCompile-000000\CMakeFiles\cmTC_00000.dir\testCCompiler.c.obj"
    return [pscustomobject]@{
        Path = $samplePath
        Length = $samplePath.Length
    }
}
