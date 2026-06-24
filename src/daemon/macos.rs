//! macOS desktop permission, focus, and diagnostic helpers.

#![cfg(target_os = "macos")]

use anyhow::{bail, Context, Result};
use objc2::rc::autoreleasepool;
use objc2::{class, msg_send};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

type Boolean = u8;
type CFArrayRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFIndex = isize;
type CFNumberRef = *const c_void;
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
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const K_CG_NULL_WINDOW_ID: u32 = 0;
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const K_CF_NUMBER_SINT64_TYPE: i32 = 4;
const MACOS_V_KEYCODE: u16 = 9;

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

    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, the_type: i32, value_ptr: *mut c_void) -> Boolean;
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
    static kCGWindowLayer: CFStringRef;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;

    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtual_key: u16,
        key_down: Boolean,
    ) -> CGEventRef;
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventTapEnable(tap: CFMachPortRef, enable: Boolean);
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
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

    fn same_target(&self, current: &Self) -> bool {
        current.pid == self.pid
            && current.bundle_identifier == self.bundle_identifier
            && current.window_id == self.window_id
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
    if permissions.accessibility.granted() && permissions.input_monitoring.granted() {
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
    suppressed_key_event_smoke_with_expectation(action, None, 0)
}

/// Run a paste shortcut action behind a temporary suppressing event tap.
///
/// # Arguments
///
/// * `action` - Synthetic paste action to execute while the tap is enabled.
///
/// # Returns
///
/// `Ok(())` when Cmd+V down/up events are observed.
///
/// # Errors
///
/// Returns an error if the event tap cannot be created, the action fails, or the
/// observed events do not include a Command-flagged V key down/up pair.
pub(crate) fn suppressed_paste_shortcut_smoke(action: impl FnOnce() -> Result<()>) -> Result<()> {
    suppressed_key_event_smoke_with_expectation(
        action,
        Some(MACOS_V_KEYCODE.into()),
        K_CG_EVENT_FLAG_MASK_COMMAND,
    )
}

fn suppressed_key_event_smoke_with_expectation(
    action: impl FnOnce() -> Result<()>,
    expected_keycode: Option<i64>,
    required_flags: u64,
) -> Result<()> {
    event_tap_preflight()?;

    let state = SmokeTapState {
        expected_keycode,
        required_flags,
        ..SmokeTapState::default()
    };
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
            "could not create macOS event tap for insertion smoke test; grant Accessibility and Input Monitoring to your terminal and rerun parakit doctor --deep"
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
        if expected_keycode.is_some()
            && (!state.saw_expected_key_down.load(Ordering::Acquire)
                || !state.saw_expected_key_up.load(Ordering::Acquire))
        {
            bail!("macOS insertion smoke test did not observe expected Cmd+V key down/up events")
        }
        Ok(())
    } else {
        bail!("macOS insertion smoke test did not observe synthetic key down/up events")
    }
}

/// Send a macOS paste shortcut using a single flagged key event pair.
///
/// # Returns
///
/// `Ok(())` when CoreGraphics accepted the synthetic Cmd+V key events.
///
/// # Errors
///
/// Returns an error if CoreGraphics cannot allocate the keyboard events.
///
/// Callers must run `accessibility_preflight()` before choosing this backend.
/// The daemon and `doctor --deep` both do that once before entering this hot path.
pub(crate) fn send_paste_shortcut() -> Result<()> {
    let key_down = unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), MACOS_V_KEYCODE, 1) };
    if key_down.is_null() {
        bail!("could not create macOS paste key-down event");
    }
    let key_up = unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), MACOS_V_KEYCODE, 0) };
    if key_up.is_null() {
        unsafe {
            CFRelease(key_down.cast());
        }
        bail!("could not create macOS paste key-up event");
    }

    unsafe {
        CGEventSetFlags(key_down, K_CG_EVENT_FLAG_MASK_COMMAND);
        CGEventSetFlags(key_up, K_CG_EVENT_FLAG_MASK_COMMAND);
        CGEventPost(K_CG_HID_EVENT_TAP, key_down);
        CGEventPost(K_CG_HID_EVENT_TAP, key_up);
        CFRelease(key_down.cast());
        CFRelease(key_up.cast());
    }
    Ok(())
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
    saw_expected_key_down: AtomicBool,
    saw_expected_key_up: AtomicBool,
    expected_keycode: Option<i64>,
    required_flags: u64,
}

extern "C" fn smoke_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    catch_unwind(AssertUnwindSafe(|| {
        smoke_tap_callback_inner(event_type, event, user_info)
    }))
    .unwrap_or(event)
}

fn smoke_tap_callback_inner(
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if !user_info.is_null() {
        let state = unsafe { &*(user_info.cast::<SmokeTapState>()) };
        match event_type {
            K_CG_EVENT_KEY_DOWN => {
                state.saw_key_down.store(true, Ordering::Release);
                if state.matches_expected(event) {
                    state.saw_expected_key_down.store(true, Ordering::Release);
                }
            }
            K_CG_EVENT_KEY_UP => {
                state.saw_key_up.store(true, Ordering::Release);
                if state.matches_expected(event) {
                    state.saw_expected_key_up.store(true, Ordering::Release);
                }
            }
            K_CG_EVENT_FLAGS_CHANGED => {}
            _ => return event,
        }
        return ptr::null_mut();
    }
    event
}

impl SmokeTapState {
    fn complete(&self) -> bool {
        if self.expected_keycode.is_some() {
            self.saw_expected_key_down.load(Ordering::Acquire)
                && self.saw_expected_key_up.load(Ordering::Acquire)
        } else {
            self.saw_key_down.load(Ordering::Acquire) && self.saw_key_up.load(Ordering::Acquire)
        }
    }

    fn matches_expected(&self, event: CGEventRef) -> bool {
        let Some(expected_keycode) = self.expected_keycode else {
            return true;
        };
        let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
        let flags = unsafe { CGEventGetFlags(event) };
        keycode == expected_keycode && (flags & self.required_flags) == self.required_flags
    }
}

fn wait_for_smoke_events(state: &SmokeTapState) {
    let deadline = Instant::now() + SMOKE_TIMEOUT;
    while Instant::now() < deadline {
        unsafe {
            let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, SMOKE_POLL.as_secs_f64(), 1);
        }
        if state.complete() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
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
