//! macOS desktop permission, focus, and diagnostic helpers.

#![cfg(target_os = "macos")]

use anyhow::{bail, Context, Result};
use objc2::rc::autoreleasepool;
use objc2::{class, msg_send};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

type Boolean = u8;
type CFAllocatorRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFIndex = isize;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CFTypeRef = *const c_void;
type CFMachPortRef = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CGEventTapCallBack =
    extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

const AV_AUTH_NOT_DETERMINED: isize = 0;
const AV_AUTH_RESTRICTED: isize = 1;
const AV_AUTH_DENIED: isize = 2;
const AV_AUTH_AUTHORIZED: isize = 3;

const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
const K_IOHID_ACCESS_TYPE_GRANTED: i32 = 0;
const K_IOHID_ACCESS_TYPE_DENIED: i32 = 1;
const K_IOHID_ACCESS_TYPE_UNKNOWN: i32 = 2;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;

const SMOKE_TIMEOUT: Duration = Duration::from_millis(750);
const SMOKE_POLL: Duration = Duration::from_millis(20);

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
    static kCFRunLoopDefaultMode: CFStringRef;

    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: f64,
        return_after_source_handled: Boolean,
    ) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: Boolean);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request_type: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
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

/// Sendable representation of the frontmost macOS application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MacOsFocusSnapshot {
    pid: libc::pid_t,
    bundle_identifier: Option<String>,
}

impl MacOsFocusSnapshot {
    /// Capture the current frontmost application.
    ///
    /// # Returns
    ///
    /// A snapshot suitable for comparison immediately before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when macOS reports no frontmost application.
    pub(crate) fn capture() -> Result<Self> {
        frontmost_application().context("could not capture macOS frontmost application")
    }

    /// Return whether the current frontmost application still matches.
    ///
    /// # Returns
    ///
    /// `Ok(true)` when PID and bundle identifier still match.
    ///
    /// # Errors
    ///
    /// Returns an error when the frontmost application cannot be read.
    pub(crate) fn matches_current(&self) -> Result<bool> {
        let current =
            frontmost_application().context("could not read macOS frontmost application")?;
        Ok(current.pid == self.pid && current.bundle_identifier == self.bundle_identifier)
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

/// Return the diagnostic Input Monitoring status.
///
/// # Returns
///
/// The listen-event access state. This is informational; Accessibility remains
/// the required permission for parakit's active hotkey tap.
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

/// Return macOS architecture warnings for doctor output.
///
/// # Returns
///
/// Lines describing unsupported or translated macOS execution.
pub(crate) fn architecture_warning_lines() -> Vec<String> {
    let mut lines = Vec::new();
    if rosetta_translated() == Some(true) {
        lines.push(
            "warning: running under Rosetta; build/install for aarch64-apple-darwin to use Metal"
                .to_string(),
        );
    } else if !cfg!(target_arch = "aarch64") {
        lines.push(
            "warning: this macOS build target is not aarch64-apple-darwin; Apple Silicon is the supported macOS target"
                .to_string(),
        );
    }
    lines
}

/// Return a concise no-GPU hint for macOS device errors.
///
/// # Returns
///
/// A Rosetta/toolchain hint when applicable.
pub(crate) fn no_gpu_hint() -> Option<&'static str> {
    if rosetta_translated() == Some(true) {
        Some("this process appears to be running under Rosetta; rebuild/reinstall for aarch64-apple-darwin")
    } else if !cfg!(target_arch = "aarch64") {
        Some("this macOS build is not aarch64-apple-darwin; Apple Silicon is the supported Metal target")
    } else {
        None
    }
}

/// Run an insertion action behind a temporary suppressing event tap.
///
/// # Arguments
///
/// * `action` - Synthetic input action to execute while the tap is enabled.
///
/// # Returns
///
/// `Ok(())` when the action succeeds and key-down/key-up events are observed.
///
/// # Errors
///
/// Returns an error if the event tap cannot be created, the action fails, or no
/// key events are observed.
pub(crate) fn suppressed_key_event_smoke(action: impl FnOnce() -> Result<()>) -> Result<()> {
    accessibility_preflight()?;

    let state = SmokeTapState::default();
    let mask = event_mask(K_CG_EVENT_KEY_DOWN)
        | event_mask(K_CG_EVENT_KEY_UP)
        | event_mask(K_CG_EVENT_FLAGS_CHANGED);
    let tap = unsafe {
        CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            smoke_tap_callback,
            (&state as *const SmokeTapState).cast_mut().cast(),
        )
    };
    if tap.is_null() {
        bail!(
            "could not create macOS event tap for insertion smoke test; grant Accessibility to your terminal and rerun parakit doctor --deep"
        );
    }

    let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if source.is_null() {
        unsafe {
            CFRelease(tap.cast());
        }
        bail!("could not create macOS event-tap run-loop source");
    }

    unsafe {
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, 1);
    }

    let action_result = action();
    wait_for_smoke_events(&state);

    unsafe {
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRelease(source.cast());
        CFRelease(tap.cast());
    }

    action_result?;
    if state.saw_key_down.load(Ordering::Acquire) && state.saw_key_up.load(Ordering::Acquire) {
        Ok(())
    } else {
        bail!("macOS insertion smoke test did not observe synthetic key down/up events")
    }
}

fn frontmost_application() -> Result<MacOsFocusSnapshot> {
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
        Ok(MacOsFocusSnapshot {
            pid,
            bundle_identifier,
        })
    })
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

#[derive(Default)]
struct SmokeTapState {
    saw_key_down: AtomicBool,
    saw_key_up: AtomicBool,
}

extern "C" fn smoke_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if !user_info.is_null() {
        let state = unsafe { &*(user_info.cast::<SmokeTapState>()) };
        match event_type {
            K_CG_EVENT_KEY_DOWN => state.saw_key_down.store(true, Ordering::Release),
            K_CG_EVENT_KEY_UP => state.saw_key_up.store(true, Ordering::Release),
            K_CG_EVENT_FLAGS_CHANGED => {}
            _ => return event,
        }
        return ptr::null_mut();
    }
    event
}

fn wait_for_smoke_events(state: &SmokeTapState) {
    let deadline = Instant::now() + SMOKE_TIMEOUT;
    while Instant::now() < deadline {
        unsafe {
            let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, SMOKE_POLL.as_secs_f64(), 1);
        }
        if state.saw_key_down.load(Ordering::Acquire) && state.saw_key_up.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
}
