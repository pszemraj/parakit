# Install a built Parakit Windows bundle into a per-user app directory.

param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir,

    [switch]$NoUserPath
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $PSCommandPath
. (Join-Path $scriptDir "common.ps1")

function Assert-Bundle {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Bundle directory does not exist: $Path"
    }

    $manifestPath = Join-Path $Path "parakit-runtime-manifest.json"
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Bundle is missing required runtime manifest: parakit-runtime-manifest.json"
    }

    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    } catch {
        throw "Bundle runtime manifest is not valid JSON: $manifestPath"
    }

    if ($null -eq $manifest.required_files) {
        throw "Bundle runtime manifest is missing required_files"
    }

    $requiredFiles = @($manifest.required_files)
    if ($requiredFiles.Count -eq 0) {
        throw "Bundle runtime manifest required_files is empty"
    }

    if (-not ($requiredFiles -contains "parakit.exe")) {
        throw "Bundle runtime manifest required_files must include parakit.exe"
    }

    foreach ($required in $requiredFiles) {
        Assert-FlatBundleFileName -Name $required
        $candidate = Join-Path $Path $required
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Bundle is missing required runtime file: $required"
        }
    }

    return $manifest
}

function Assert-InstallDir {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $full = Get-FullPath $Path
    $trimmed = $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $root = [System.IO.Path]::GetPathRoot($full).TrimEnd([System.IO.Path]::DirectorySeparatorChar)

    if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed -eq $root) {
        throw "Refusing to install into an unsafe directory: $full"
    }

    $forbiddenTrees = @(
        $env:SystemRoot,
        $env:ProgramFiles,
        ${env:ProgramFiles(x86)}
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($entry in $forbiddenTrees) {
        $entryFull = (Get-FullPath $entry).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
        $prefix = $entryFull + [System.IO.Path]::DirectorySeparatorChar
        if (
            $trimmed.Equals($entryFull, [System.StringComparison]::OrdinalIgnoreCase) -or
            $trimmed.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
        ) {
            throw "Refusing to install into an admin/system directory: $full"
        }
    }

    $forbiddenExact = @(
        $env:USERPROFILE,
        $env:LOCALAPPDATA
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($entry in $forbiddenExact) {
        $entryFull = (Get-FullPath $entry).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
        if ($trimmed.Equals($entryFull, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to install into an unsafe directory: $full"
        }
    }
}

function Normalize-PathEntry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Entry
    )

    $expanded = [System.Environment]::ExpandEnvironmentVariables($Entry.Trim())
    if ([string]::IsNullOrWhiteSpace($expanded)) {
        return ""
    }

    try {
        return (Get-FullPath $expanded).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    } catch {
        return $expanded.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    }
}

function Add-UserPathEntry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $target = (Get-FullPath $Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $current = [System.Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @()
    if (-not [string]::IsNullOrWhiteSpace($current)) {
        $entries = $current -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    foreach ($entry in $entries) {
        $normalized = Normalize-PathEntry $entry
        if ($normalized.Equals($target, [System.StringComparison]::OrdinalIgnoreCase)) {
            return $false
        }
    }

    $newValue = if ([string]::IsNullOrWhiteSpace($current)) {
        $target
    } else {
        "$target;$current"
    }

    [System.Environment]::SetEnvironmentVariable("Path", $newValue, "User")
    return $true
}

function Get-SearchPathEntries {
    $entries = @()
    if (-not [string]::IsNullOrWhiteSpace($env:Path)) {
        $entries += $env:Path -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }
    return $entries
}

function Test-DllResolvable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [string[]]$ExtraDirs = @()
    )

    foreach ($dir in $ExtraDirs) {
        if ([string]::IsNullOrWhiteSpace($dir)) {
            continue
        }
        $candidate = Join-Path $dir $Name
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $true
        }
    }

    foreach ($dir in Get-SearchPathEntries) {
        $expanded = [System.Environment]::ExpandEnvironmentVariables($dir)
        if ([string]::IsNullOrWhiteSpace($expanded)) {
            continue
        }
        $candidate = Join-Path $expanded $Name
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $true
        }
    }

    return $false
}

function Assert-ExternalRuntimeDependencies {
    param(
        [Parameter(Mandatory = $true)]
        $Manifest
    )

    if ($null -ne $Manifest.cuda) {
        Assert-CudaExternalDlls $Manifest.cuda
    }
    if ($null -ne $Manifest.vulkan) {
        Assert-VulkanExternalDlls $Manifest.vulkan
    }
}

function Assert-CudaExternalDlls {
    param(
        [Parameter(Mandatory = $true)]
        $Cuda
    )

    if ($null -ne $Cuda.external_dlls_bundled -and [bool]$Cuda.external_dlls_bundled) {
        return
    }

    $dlls = @($Cuda.external_dlls) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    if ($dlls.Count -eq 0) {
        return
    }

    $extraDirs = @()
    if (-not [string]::IsNullOrWhiteSpace($env:CUDA_PATH)) {
        $cudaBin = Join-Path $env:CUDA_PATH "bin"
        $extraDirs += $cudaBin
        $extraDirs += Join-Path $cudaBin "x64"
    }

    $missing = @()
    foreach ($dll in $dlls) {
        if (-not (Test-DllResolvable -Name $dll -ExtraDirs $extraDirs)) {
            $missing += $dll
        }
    }

    if ($missing.Count -gt 0) {
        $version = if ([string]::IsNullOrWhiteSpace($Cuda.toolkit_version)) { "the build" } else { $Cuda.toolkit_version }
        throw "CUDA bundle expects external runtime DLLs that were not found: $($missing -join ', '). Install the CUDA Toolkit matching $version so the CUDA runtime DLL directory is available, add the DLL directory to PATH, or rebuild with --bundle-cuda-dlls."
    }
}

function Assert-VulkanExternalDlls {
    param(
        [Parameter(Mandatory = $true)]
        $Vulkan
    )

    $dlls = @($Vulkan.external_dlls) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    if ($dlls.Count -eq 0) {
        return
    }

    $systemDir = [System.Environment]::SystemDirectory
    $missing = @()
    foreach ($dll in $dlls) {
        if (-not (Test-DllResolvable -Name $dll -ExtraDirs @($systemDir))) {
            $missing += $dll
        }
    }

    if ($missing.Count -gt 0) {
        throw "Vulkan bundle expects the driver-provided loader DLLs that were not found: $($missing -join ', '). Install or update the NVIDIA, AMD, or Intel GPU driver, or install the CPU bundle on machines without a Vulkan-capable driver."
    }
}

function Invoke-InstallSmoke {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $exe = Join-Path $Path "parakit.exe"
    try {
        & $exe --version > $null
        $code = $LASTEXITCODE
    } catch {
        throw "Installed parakit.exe failed to start. This usually means a runtime DLL is missing. $($_.Exception.Message)"
    }

    if (Test-StatusDllNotFoundExitCode $code) {
        throw "Installed parakit.exe failed to start with 0xC0000135 (STATUS_DLL_NOT_FOUND). A required runtime DLL is missing; check the bundle manifest external_dlls entries."
    }
    if ($code -ne 0) {
        throw "Installed parakit loader smoke test failed with exit code $code"
    }

    Write-Host "Smoke: parakit --version OK"
}

function Test-StatusDllNotFoundExitCode {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Code
    )

    return $Code -eq -1073741515 -or ([uint32]$Code) -eq 0xC0000135
}

function Install-Bundle {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Source,

        [Parameter(Mandatory = $true)]
        [string]$Destination
    )

    $marker = Join-Path $Destination ".parakit-install"

    if (Test-Path -LiteralPath $Destination -PathType Container) {
        if (Test-Path -LiteralPath $marker -PathType Leaf) {
            Remove-Item -LiteralPath $Destination -Recurse -Force
        } else {
            Write-Host "Install warning: existing unmarked directory; overwriting files only"
        }
    }

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    Get-ChildItem -LiteralPath $Source -Force |
        ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
        }

    Set-Content -LiteralPath $marker -Value "parakit windows install" -Encoding ascii
}

Assert-NativeWindows "This installer"

$bundleFull = (Resolve-Path -LiteralPath $BundleDir).Path
$installFull = Get-FullPath $InstallDir

$manifest = Assert-Bundle $bundleFull
Assert-InstallDir $installFull
Assert-ExternalRuntimeDependencies $manifest

Install-Bundle -Source $bundleFull -Destination $installFull
Write-Host "Installed: $installFull"
Invoke-InstallSmoke $installFull

if ($NoUserPath) {
    Write-Host "User PATH: skipped"
} else {
    try {
        $added = Add-UserPathEntry $installFull
        if ($added) {
            Write-Host "User PATH: added $installFull"
        } else {
            Write-Host "User PATH: already contains $installFull"
        }
    } catch {
        Write-Warning "User PATH update failed: $($_.Exception.Message)"
        Write-Host "User PATH: not changed"
        Write-Host "Run directly: $(Join-Path $installFull 'parakit.exe')"
    }
}
