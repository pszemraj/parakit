# Install a built Parakit Windows bundle into a per-user app directory.

param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir,

    [switch]$NoUserPath
)

$ErrorActionPreference = "Stop"

function Assert-NativeWindows {
    if ($env:WSL_DISTRO_NAME -or $env:WSL_INTEROP) {
        throw "This installer must run on native Windows, not inside WSL."
    }

    if ($PSVersionTable.PSEdition -eq "Core") {
        if (-not $IsWindows) {
            throw "This installer must run on Windows."
        }
        return
    }

    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "This installer must run on Windows."
    }
}

function Get-FullPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return [System.IO.Path]::GetFullPath($Path)
}

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
        if ([string]::IsNullOrWhiteSpace($required)) {
            throw "Bundle runtime manifest contains an empty required file entry"
        }
        if (
            [System.IO.Path]::IsPathRooted($required) -or
            $required.Contains("/") -or
            $required.Contains("\") -or
            $required.Contains("..")
        ) {
            throw "Bundle runtime manifest required file must be a flat file name: $required"
        }
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
    try {
        Broadcast-EnvironmentChange
    } catch {
        # Updating the persistent User PATH is the required operation. The
        # shell broadcast only helps already-running Windows processes notice.
    }
    return $true
}

function Broadcast-EnvironmentChange {
    $type = @"
using System;
using System.Runtime.InteropServices;

public static class ParakitEnvironmentBroadcast {
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(
        IntPtr hWnd,
        uint Msg,
        UIntPtr wParam,
        string lParam,
        uint fuFlags,
        uint uTimeout,
        out UIntPtr lpdwResult);
}
"@

    if (-not ("ParakitEnvironmentBroadcast" -as [type])) {
        Add-Type -TypeDefinition $type
    }

    $result = [UIntPtr]::Zero
    $hwndBroadcast = [IntPtr]0xffff
    $wmSettingChange = 0x001a
    $smtoAbortIfHung = 0x0002
    [void][ParakitEnvironmentBroadcast]::SendMessageTimeout(
        $hwndBroadcast,
        $wmSettingChange,
        [UIntPtr]::Zero,
        "Environment",
        $smtoAbortIfHung,
        5000,
        [ref]$result)
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
        Warn-VulkanExternalDlls $Manifest.vulkan
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
        $extraDirs += Join-Path $env:CUDA_PATH "bin"
    }

    $missing = @()
    foreach ($dll in $dlls) {
        if (-not (Test-DllResolvable -Name $dll -ExtraDirs $extraDirs)) {
            $missing += $dll
        }
    }

    if ($missing.Count -gt 0) {
        $version = if ([string]::IsNullOrWhiteSpace($Cuda.toolkit_version)) { "the build" } else { $Cuda.toolkit_version }
        throw "CUDA bundle expects external runtime DLLs that were not found: $($missing -join ', '). Install the CUDA Toolkit matching $version so CUDA_PATH\bin is available, add the DLL directory to PATH, or rebuild with --bundle-cuda-dlls."
    }
}

function Warn-VulkanExternalDlls {
    param(
        [Parameter(Mandatory = $true)]
        $Vulkan
    )

    $dlls = @($Vulkan.external_dlls) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    if ($dlls.Count -eq 0) {
        return
    }

    $systemDir = [System.Environment]::SystemDirectory
    foreach ($dll in $dlls) {
        if (-not (Test-DllResolvable -Name $dll -ExtraDirs @($systemDir))) {
            Write-Warning "Vulkan loader $dll was not found in System32 or PATH. Install a current NVIDIA, AMD, or Intel GPU driver before running the Vulkan bundle."
        }
    }
}

function Invoke-InstallSmoke {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $exe = Join-Path $Path "parakit.exe"
    try {
        & $exe --quiet doctor
        $code = $LASTEXITCODE
    } catch {
        throw "Installed parakit.exe failed to start. This usually means a runtime DLL is missing. $($_.Exception.Message)"
    }

    if (Test-StatusDllNotFoundExitCode $code) {
        throw "Installed parakit.exe failed to start with 0xC0000135 (STATUS_DLL_NOT_FOUND). A required runtime DLL is missing; check the bundle manifest external_dlls entries."
    }
    if ($code -ne 0) {
        throw "Installed parakit doctor smoke test failed with exit code $code"
    }

    Write-Host "Smoke: parakit --quiet doctor OK"
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

Assert-NativeWindows

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
