function Assert-NativeWindows {
    param(
        [string]$Context = "This script"
    )

    if ($env:WSL_DISTRO_NAME -or $env:WSL_INTEROP) {
        throw "$Context must run on native Windows, not inside WSL."
    }

    if ($PSVersionTable.PSEdition -eq "Core") {
        if (-not $IsWindows) {
            throw "$Context must run on Windows."
        }
        return
    }

    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "$Context must run on Windows."
    }
}
