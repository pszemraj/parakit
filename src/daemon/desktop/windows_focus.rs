//! Windows foreground-window focus snapshots.

#![cfg(target_os = "windows")]

use anyhow::{bail, Result};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Sendable representation of the foreground window captured at recording start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowsFocusSnapshot {
    hwnd_raw: usize,
    pid: u32,
    tid: u32,
}

impl WindowsFocusSnapshot {
    /// Capture the current foreground window and owner identifiers.
    ///
    /// # Returns
    ///
    /// A snapshot suitable for comparison immediately before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when Windows has no foreground window or the window
    /// owner cannot be identified.
    pub(crate) fn capture() -> Result<Self> {
        let Some((hwnd_raw, pid, tid)) = foreground_window_owner() else {
            bail!("no Windows foreground window while capturing focus");
        };
        if tid == 0 || pid == 0 {
            bail!("could not identify Windows foreground window owner");
        }

        Ok(Self { hwnd_raw, pid, tid })
    }

    /// Return whether the current foreground window still matches this snapshot.
    ///
    /// # Returns
    ///
    /// `Ok(true)` when the foreground HWND, process id, and thread id match.
    ///
    /// # Errors
    ///
    /// Returns an error when foreground-window owner metadata cannot be read.
    pub(crate) fn matches_current(self) -> Result<bool> {
        let Some((hwnd_raw, pid, tid)) = foreground_window_owner() else {
            return Ok(false);
        };
        Ok(hwnd_raw == self.hwnd_raw && pid == self.pid && tid == self.tid)
    }
}

/// Read the current Windows foreground window along with its owning process
/// and thread identifiers.
///
/// # Returns
///
/// `None` when there is no foreground window; otherwise the window handle
/// (as `usize`), process id, and thread id, in that order. The process id
/// and thread id may still be `0` if Windows could not resolve an owner.
fn foreground_window_owner() -> Option<(usize, u32, u32)> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut pid = 0_u32;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    Some((hwnd.0 as usize, pid, tid))
}
