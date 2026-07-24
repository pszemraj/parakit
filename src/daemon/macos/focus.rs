//! macOS frontmost-application-window focus snapshots.

use anyhow::{bail, Context, Result};
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use std::ffi::c_void;

type Boolean = u8;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFIndex = isize;
type CFNumberRef = *const c_void;
type CFStringRef = *const c_void;
type CFTypeRef = *const c_void;

const K_CG_NULL_WINDOW_ID: u32 = 0;
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const K_CF_NUMBER_SINT64_TYPE: i32 = 4;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, the_type: i32, value_ptr: *mut c_void) -> Boolean;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGWindowLayer: CFStringRef;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;

    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

/// Sendable representation of the focused macOS insertion target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MacOsFocusSnapshot {
    pid: libc::pid_t,
    bundle_identifier: Option<String>,
    window_id: u32,
}

impl MacOsFocusSnapshot {
    /// Capture the current frontmost application window.
    ///
    /// # Returns
    ///
    /// A snapshot suitable for comparison immediately before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when macOS reports no frontmost application window.
    pub(crate) fn capture() -> Result<Self> {
        frontmost_application_window().context("could not capture macOS frontmost window")
    }

    /// Return whether the current frontmost application window still matches.
    ///
    /// # Returns
    ///
    /// `Ok(true)` when PID, bundle identifier, and window id still match.
    ///
    /// # Errors
    ///
    /// Returns an error when the frontmost application window cannot be read.
    pub(crate) fn matches_current(&self) -> Result<bool> {
        let current =
            frontmost_application_window().context("could not read macOS frontmost window")?;
        Ok(self.same_target(&current))
    }

    /// Return the bundle identifier of the captured frontmost application,
    /// when macOS reported one.
    ///
    /// # Returns
    ///
    /// The bundle identifier string, or `None` when the frontmost
    /// application had no bundle identifier.
    pub(crate) fn bundle_id(&self) -> Option<&str> {
        self.bundle_identifier.as_deref()
    }

    fn same_target(&self, current: &Self) -> bool {
        current.pid == self.pid
            && current.bundle_identifier == self.bundle_identifier
            && current.window_id == self.window_id
    }
}

fn frontmost_application_window() -> Result<MacOsFocusSnapshot> {
    autoreleasepool(|_pool| {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace
            .frontmostApplication()
            .context("macOS reported no frontmost application")?;
        let pid: libc::pid_t = unsafe { msg_send![&*app, processIdentifier] };
        if pid <= 0 {
            bail!("macOS frontmost application has no process id");
        }
        let bundle_identifier = app
            .bundleIdentifier()
            .map(|bundle| bundle.to_string())
            .filter(|bundle| !bundle.is_empty());
        let window_id = frontmost_window_id_for_pid(pid)?;
        Ok(MacOsFocusSnapshot {
            pid,
            bundle_identifier,
            window_id,
        })
    })
}

fn frontmost_window_id_for_pid(pid: libc::pid_t) -> Result<u32> {
    let options =
        K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;
    let windows = unsafe { CGWindowListCopyWindowInfo(options, K_CG_NULL_WINDOW_ID) };
    if windows.is_null() {
        bail!("macOS did not return an on-screen window list");
    }

    let window_id = frontmost_window_id_in_list(windows, pid).with_context(|| {
        format!("macOS frontmost application pid {pid} has no visible layer-0 window")
    });
    unsafe {
        CFRelease(windows.cast());
    }
    window_id
}

fn frontmost_window_id_in_list(windows: CFArrayRef, pid: libc::pid_t) -> Option<u32> {
    let count = unsafe { CFArrayGetCount(windows) };
    for idx in 0..count {
        let window = unsafe { CFArrayGetValueAtIndex(windows, idx) };
        if window.is_null() {
            continue;
        }
        let window = window.cast();
        let Some(owner_pid) = cf_dictionary_i64(window, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        if owner_pid != i64::from(pid) {
            continue;
        }
        let Some(layer) = cf_dictionary_i64(window, unsafe { kCGWindowLayer }) else {
            continue;
        };
        if layer != 0 {
            continue;
        }
        let Some(window_id) = cf_dictionary_i64(window, unsafe { kCGWindowNumber }) else {
            continue;
        };
        if let Ok(window_id) = u32::try_from(window_id) {
            if window_id != 0 {
                return Some(window_id);
            }
        }
    }
    None
}

fn cf_dictionary_i64(dictionary: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    let value = unsafe { CFDictionaryGetValue(dictionary, key.cast()) };
    if value.is_null() {
        return None;
    }

    let mut out = 0_i64;
    let ok = unsafe {
        CFNumberGetValue(
            value.cast(),
            K_CF_NUMBER_SINT64_TYPE,
            (&mut out as *mut i64).cast(),
        ) != 0
    };
    ok.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        pid: libc::pid_t,
        bundle_identifier: Option<&str>,
        window_id: u32,
    ) -> MacOsFocusSnapshot {
        MacOsFocusSnapshot {
            pid,
            bundle_identifier: bundle_identifier.map(ToOwned::to_owned),
            window_id,
        }
    }

    #[test]
    fn focus_snapshot_requires_matching_window_id() {
        let original = snapshot(42, Some("com.example.App"), 1001);

        assert!(original.same_target(&snapshot(42, Some("com.example.App"), 1001)));
        assert!(!original.same_target(&snapshot(42, Some("com.example.App"), 1002)));
        assert!(!original.same_target(&snapshot(43, Some("com.example.App"), 1001)));
        assert!(!original.same_target(&snapshot(42, Some("com.example.Other"), 1001)));
    }
}
