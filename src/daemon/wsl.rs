//! WSL environment detection for diagnostics.

#![cfg(target_os = "linux")]

/// Return whether this Linux process appears to run under WSL.
///
/// # Returns
///
/// `true` when the kernel release contains common Microsoft/WSL markers.
pub(crate) fn running_under_wsl() -> bool {
    let Ok(version) = std::fs::read_to_string("/proc/sys/kernel/osrelease") else {
        return false;
    };

    let version = version.to_ascii_lowercase();
    version.contains("microsoft") || version.contains("wsl")
}

/// Return the standard WSL diagnostic note.
///
/// # Returns
///
/// A static warning string for doctor output.
pub(crate) fn warning() -> &'static str {
    "running inside WSL: this is a Linux binary, not a native Windows daemon. Use native Windows PowerShell to validate Windows hotkeys, foreground-window focus, and paste insertion."
}
