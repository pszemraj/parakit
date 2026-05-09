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

    foreach ($required in @("parakit.exe", "crispasr.dll", "whisper.dll", "ggml.dll")) {
        $candidate = Join-Path $Path $required
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Bundle is missing required runtime file: $required"
        }
    }
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

    $forbidden = @(
        $env:SystemRoot,
        $env:ProgramFiles,
        ${env:ProgramFiles(x86)},
        $env:USERPROFILE,
        $env:LOCALAPPDATA
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($entry in $forbidden) {
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
    Broadcast-EnvironmentChange
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

Assert-Bundle $bundleFull
Assert-InstallDir $installFull

Install-Bundle -Source $bundleFull -Destination $installFull
Write-Host "Installed: $installFull"

if ($NoUserPath) {
    Write-Host "User PATH: skipped"
} else {
    $added = Add-UserPathEntry $installFull
    if ($added) {
        Write-Host "User PATH: added $installFull"
    } else {
        Write-Host "User PATH: already contains $installFull"
    }
}
