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
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            bail!("no Windows foreground window while capturing focus");
        }

        let mut pid = 0_u32;
        let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
        if tid == 0 || pid == 0 {
            bail!("could not identify Windows foreground window owner");
        }

        Ok(Self {
            hwnd_raw: hwnd.0 as usize,
            pid,
            tid,
        })
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
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return Ok(false);
        }

        let mut pid = 0_u32;
        let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
        Ok(hwnd.0 as usize == self.hwnd_raw && pid == self.pid && tid == self.tid)
    }
}
