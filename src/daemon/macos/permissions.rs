//! macOS TCC permission diagnostics: Accessibility, Microphone, and Input Monitoring.

use super::cgevent_ffi::{Boolean, CFAllocatorRef, CFIndex, CFRelease, CFStringRef, CFTypeRef};
use anyhow::{bail, Result};
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::ptr;

type CFDictionaryRef = *const c_void;

const AV_AUTH_NOT_DETERMINED: isize = 0;
const AV_AUTH_RESTRICTED: isize = 1;
const AV_AUTH_DENIED: isize = 2;
const AV_AUTH_AUTHORIZED: isize = 3;

const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
const K_IOHID_ACCESS_TYPE_GRANTED: i32 = 0;
const K_IOHID_ACCESS_TYPE_DENIED: i32 = 1;
const K_IOHID_ACCESS_TYPE_UNKNOWN: i32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrusted() -> Boolean;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *const NSString;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;

    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request_type: u32) -> i32;
}

/// macOS permission state shown by doctor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PermissionStatus {
    Granted,
    Denied,
    Restricted,
    NotDetermined,
    Unknown(i64),
}

impl PermissionStatus {
    /// Return a short status label.
    ///
    /// # Returns
    ///
    /// A stable diagnostic label for doctor output.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Granted => "granted".to_string(),
            Self::Denied => "denied".to_string(),
            Self::Restricted => "restricted".to_string(),
            Self::NotDetermined => "not determined".to_string(),
            Self::Unknown(value) => format!("unknown ({value})"),
        }
    }

    /// Return whether this status permits use of the protected API.
    ///
    /// # Returns
    ///
    /// `true` when the permission has been granted.
    pub(crate) fn granted(self) -> bool {
        self == Self::Granted
    }

    fn blocking_for_microphone(self) -> bool {
        matches!(self, Self::Denied | Self::Restricted)
    }
}

/// macOS permission snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PermissionReport {
    pub(crate) accessibility: PermissionStatus,
    pub(crate) microphone: PermissionStatus,
    pub(crate) input_monitoring: PermissionStatus,
}

impl PermissionReport {
    /// Return whether the CoreGraphics session event tap has both permissions
    /// it requires.
    ///
    /// # Returns
    ///
    /// `true` when Accessibility and Input Monitoring are both granted.
    pub(crate) fn event_tap_ready(&self) -> bool {
        self.accessibility.granted() && self.input_monitoring.granted()
    }
}

/// Return the current permission snapshot.
///
/// # Arguments
///
/// * `prompt_accessibility` - Trigger the macOS Accessibility prompt when the
///   process is not trusted.
///
/// # Returns
///
/// The effective TCC states for the responsible terminal process.
pub(crate) fn permission_report(prompt_accessibility: bool) -> PermissionReport {
    PermissionReport {
        accessibility: accessibility_permission_status(prompt_accessibility),
        microphone: microphone_permission_status(),
        input_monitoring: input_monitoring_permission_status(),
    }
}

/// Return the Accessibility permission state.
///
/// # Arguments
///
/// * `prompt` - Request that macOS shows its Accessibility prompt if needed.
///
/// # Returns
///
/// The effective Accessibility trust state.
pub(crate) fn accessibility_permission_status(prompt: bool) -> PermissionStatus {
    let trusted = if prompt {
        accessibility_trusted_with_prompt()
    } else {
        unsafe { AXIsProcessTrusted() != 0 }
    };
    if trusted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

/// Fail when Accessibility is not granted.
///
/// # Returns
///
/// `Ok(())` when synthetic desktop input is permitted.
///
/// # Errors
///
/// Returns an actionable macOS permission error when Accessibility is missing.
pub(crate) fn accessibility_preflight() -> Result<()> {
    if accessibility_permission_status(false).granted() {
        return Ok(());
    }

    bail!(
        "macOS Accessibility permission is not granted; grant Accessibility to your terminal in System Settings > Privacy & Security > Accessibility, then rerun parakit"
    )
}

/// Fail when the macOS event-tap permissions required for hotkey capture are missing.
///
/// # Returns
///
/// `Ok(())` when the launching terminal can create the session event tap used
/// for `Left Control+Space`.
///
/// # Errors
///
/// Returns an actionable macOS permission error when Accessibility or Input
/// Monitoring is missing.
pub(crate) fn event_tap_preflight() -> Result<()> {
    let permissions = permission_report(false);
    if permissions.event_tap_ready() {
        return Ok(());
    }

    bail!(
        "macOS hotkey capture requires Accessibility and Input Monitoring for the terminal that launched parakit; grant both in System Settings > Privacy & Security, restart parakit, then rerun parakit doctor"
    )
}

/// Fail when Microphone has been explicitly denied.
///
/// # Returns
///
/// `Ok(())` when microphone capture may proceed or may still prompt.
///
/// # Errors
///
/// Returns an actionable macOS permission error when Microphone is denied or
/// restricted.
pub(crate) fn microphone_preflight() -> Result<()> {
    let status = microphone_permission_status();
    if status.blocking_for_microphone() {
        bail!(
            "macOS Microphone permission is {}; grant Microphone to your terminal in System Settings > Privacy & Security > Microphone, then rerun parakit",
            status.label()
        );
    }
    Ok(())
}

/// Return the Microphone authorization status.
///
/// # Returns
///
/// The AVFoundation microphone authorization state.
pub(crate) fn microphone_permission_status() -> PermissionStatus {
    let status: isize = unsafe {
        msg_send![
            class!(AVCaptureDevice),
            authorizationStatusForMediaType: AVMediaTypeAudio
        ]
    };
    match status {
        AV_AUTH_NOT_DETERMINED => PermissionStatus::NotDetermined,
        AV_AUTH_RESTRICTED => PermissionStatus::Restricted,
        AV_AUTH_DENIED => PermissionStatus::Denied,
        AV_AUTH_AUTHORIZED => PermissionStatus::Granted,
        other => PermissionStatus::Unknown(other as i64),
    }
}

/// Return the Input Monitoring status required by CoreGraphics session event taps.
///
/// # Returns
///
/// The listen-event access state.
pub(crate) fn input_monitoring_permission_status() -> PermissionStatus {
    let status = unsafe { IOHIDCheckAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) };
    match status {
        K_IOHID_ACCESS_TYPE_GRANTED => PermissionStatus::Granted,
        K_IOHID_ACCESS_TYPE_DENIED => PermissionStatus::Denied,
        K_IOHID_ACCESS_TYPE_UNKNOWN => PermissionStatus::NotDetermined,
        other => PermissionStatus::Unknown(i64::from(other)),
    }
}

/// Return whether this process is translated by Rosetta 2.
///
/// # Returns
///
/// `Some(true)` under Rosetta, `Some(false)` for native execution when the
/// sysctl is available, and `None` when the sysctl is unavailable.
pub(crate) fn rosetta_translated() -> Option<bool> {
    let mut translated = 0_i32;
    let mut size = std::mem::size_of::<i32>();
    let name = b"sysctl.proc_translated\0";
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut translated as *mut i32).cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    (rc == 0).then_some(translated != 0)
}

#[derive(Clone, Copy)]
struct ArchitectureIssue {
    warning: &'static str,
    no_gpu_hint: &'static str,
}

fn architecture_issue() -> Option<ArchitectureIssue> {
    if rosetta_translated() == Some(true) {
        Some(ArchitectureIssue {
            warning:
                "warning: running under Rosetta; build/install for aarch64-apple-darwin to use Metal",
            no_gpu_hint:
                "this process appears to be running under Rosetta; rebuild/reinstall for aarch64-apple-darwin",
        })
    } else if cfg!(target_arch = "aarch64") {
        None
    } else {
        Some(ArchitectureIssue {
            warning:
                "warning: this macOS build target is not aarch64-apple-darwin; Apple Silicon is the supported macOS target",
            no_gpu_hint:
                "this macOS build is not aarch64-apple-darwin; Apple Silicon is the supported Metal target",
        })
    }
}

/// Return macOS architecture warnings for doctor output.
///
/// # Returns
///
/// Lines describing unsupported or translated macOS execution.
pub(crate) fn architecture_warning_lines() -> Vec<String> {
    architecture_issue()
        .map(|issue| issue.warning)
        .map(str::to_owned)
        .into_iter()
        .collect()
}

/// Return a concise no-GPU hint for macOS device errors.
///
/// # Returns
///
/// A Rosetta/toolchain hint when applicable.
pub(crate) fn no_gpu_hint() -> Option<&'static str> {
    architecture_issue().map(|issue| issue.no_gpu_hint)
}

fn accessibility_trusted_with_prompt() -> bool {
    let keys = [unsafe { kAXTrustedCheckOptionPrompt.cast() }];
    let values = [unsafe { kCFBooleanTrue.cast() }];
    let options = unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
        )
    };
    if options.is_null() {
        return unsafe { AXIsProcessTrusted() != 0 };
    }

    let trusted = unsafe { AXIsProcessTrustedWithOptions(options) != 0 };
    unsafe {
        CFRelease(options.cast());
    }
    trusted
}
