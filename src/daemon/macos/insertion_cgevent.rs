//! macOS CGEvent-based paste shortcut and insertion smoke-test helpers.

use super::permissions::event_tap_preflight;
use anyhow::{bail, Result};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

type Boolean = u8;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CFTypeRef = *const c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CGEventTapCallBack =
    extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const MACOS_V_KEYCODE: u16 = 9;

const SMOKE_TIMEOUT: Duration = Duration::from_millis(750);
const SMOKE_POLL: Duration = Duration::from_millis(20);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;

    fn CFRelease(cf: CFTypeRef);
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: f64,
        return_after_source_handled: Boolean,
    ) -> i32;
    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
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
