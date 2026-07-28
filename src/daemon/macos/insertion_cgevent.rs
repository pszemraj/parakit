//! macOS CGEvent-based paste shortcut and insertion smoke-test helpers.

use super::permissions::event_tap_preflight;
use crate::daemon::desktop::hotkey::{MACOS_PTT_LEFT_CONTROL_KEYCODE, MACOS_PTT_SPACE_KEYCODE};
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
const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: i32 = 1;
const MACOS_V_KEYCODE: u16 = 9;
const MACOS_COMMAND_KEYCODE: u16 = 55;

const SMOKE_TIMEOUT: Duration = Duration::from_millis(750);
const SMOKE_POLL: Duration = Duration::from_millis(20);
/// Yield between run-loop slices so the smoke wait never becomes a spin loop
/// when `CFRunLoopRunInMode` returns early with nothing to dispatch.
const SMOKE_YIELD: Duration = Duration::from_millis(5);
const PTT_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const PTT_RELEASE_POLL: Duration = Duration::from_millis(15);

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
    fn CFMachPortInvalidate(port: CFMachPortRef);
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
    fn CGEventSourceCreate(state_id: i32) -> *mut c_void;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
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
    let tap = SmokeTap::install(&state, mask)?;
    run_smoke_action(tap, action, || wait_for_smoke_events(&state))?;
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

/// Installed smoke tap whose lifetime keeps `SmokeTapState::user_info` valid.
///
/// Declared after the stack-owned state and moved into
/// [`run_smoke_action`], so teardown runs before that state can be dropped on
/// both normal return and unwinding.
struct SmokeTap {
    tap: CFMachPortRef,
    source: CFRunLoopSourceRef,
    run_loop: CFRunLoopRef,
}

impl SmokeTap {
    fn install(state: &SmokeTapState, mask: u64) -> Result<Self> {
        let tap = unsafe {
            CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_DEFAULT,
                mask,
                smoke_tap_callback,
                (state as *const SmokeTapState).cast_mut().cast(),
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
                CFMachPortInvalidate(tap);
                CFRelease(tap.cast());
            }
            bail!("could not create macOS event-tap run-loop source");
        }

        let run_loop = unsafe { CFRunLoopGetCurrent() };
        unsafe {
            CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
            CGEventTapEnable(tap, 1);
        }
        Ok(Self {
            tap,
            source,
            run_loop,
        })
    }
}

impl Drop for SmokeTap {
    fn drop(&mut self) {
        // Full teardown, in dependency order. Releasing the port objects
        // alone is not enough: without an explicit disable + invalidate, the
        // window server can keep routing events into the callback after its
        // stack-owned `user_info` state has gone away.
        unsafe {
            CGEventTapEnable(self.tap, 0);
            CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(self.tap);
            CFRelease(self.source.cast());
            CFRelease(self.tap.cast());
        }
    }
}

/// Run the smoke action and bounded observation while a teardown guard lives.
///
/// Keeping this tiny wrapper generic makes the unwind contract independently
/// testable: if `action` panics, Rust drops `guard` before unwinding back to
/// the caller.
fn run_smoke_action<G>(
    guard: G,
    action: impl FnOnce() -> Result<()>,
    wait: impl FnOnce(),
) -> Result<()> {
    let action_result = action();
    wait();
    drop(guard);
    action_result
}

/// Result of preparing and posting a macOS paste shortcut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasteShortcutOutcome {
    /// A full Cmd+V chord was posted.
    Sent,
    /// The chord was withheld because a physical push-to-talk key stayed down.
    PttKeysHeld,
}

/// Send a macOS paste shortcut as a full Cmd+V hardware-style chord.
///
/// # Returns
///
/// [`PasteShortcutOutcome::Sent`] when CoreGraphics accepted the synthetic
/// Cmd+V key events, or [`PasteShortcutOutcome::PttKeysHeld`] when the
/// physical push-to-talk keys did not become safe before the bounded
/// deadline and no event was posted.
///
/// # Errors
///
/// Returns an error if CoreGraphics cannot allocate the keyboard events.
///
/// Callers must run `accessibility_preflight()` before choosing this backend.
/// The daemon and `doctor --deep` both do that once before entering this hot path.
pub(crate) fn send_paste_shortcut() -> Result<PasteShortcutOutcome> {
    // CoreGraphics combines live hardware modifier state into synthetic
    // events. Never post while the physical PTT chord is still down: Safari
    // can otherwise receive Control+Command+V and reject an otherwise valid
    // paste. Short dictations can finish before a natural key release, so the
    // old 200ms allowance was too narrow.
    if !wait_for_ptt_keys_released(PTT_RELEASE_TIMEOUT) {
        return Ok(PasteShortcutOutcome::PttKeysHeld);
    }

    // A HID-system event source makes the synthetic chord carry the same
    // source CoreGraphics attaches to real hardware input, which is what
    // CGEventSourceKeyState-based modifier trackers key off of. If allocation
    // fails, fall back to posting with a null source rather than failing the
    // paste outright.
    let source = unsafe { CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE) };

    let events = match create_paste_chord_events(source) {
        Ok(events) => events,
        Err(err) => {
            release_event_source(source);
            return Err(err);
        }
    };

    unsafe {
        for event in &events {
            CGEventPost(K_CG_HID_EVENT_TAP, *event);
        }
        for event in &events {
            CFRelease(event.cast());
        }
    }
    release_event_source(source);

    Ok(PasteShortcutOutcome::Sent)
}

/// Build the Cmd-down, V-down, V-up, Cmd-up event chord for [`send_paste_shortcut`].
///
/// Command key-down/up events are typed by CoreGraphics as `flagsChanged`
/// automatically once posted (modifier keycodes have no key down/up
/// semantics for apps), so bracketing V with a real Cmd down/up pair makes
/// the synthetic input look like an actual hardware chord to anything
/// tracking global modifier state via `CGEventSourceKeyState` (this matters
/// for Chromium/Electron apps in particular). The explicit Command flag stays
/// set on the V events themselves because most apps read the per-event flags
/// field rather than following flagsChanged transitions.
///
/// On error, any events already created are released before returning.
fn create_paste_chord_events(source: *mut c_void) -> Result<Vec<CGEventRef>> {
    let mut events: Vec<CGEventRef> = Vec::with_capacity(4);
    match build_paste_chord_events(source, &mut events) {
        Ok(()) => Ok(events),
        Err(err) => {
            for event in &events {
                unsafe { CFRelease(event.cast()) };
            }
            Err(err)
        }
    }
}

fn build_paste_chord_events(source: *mut c_void, events: &mut Vec<CGEventRef>) -> Result<()> {
    events.push(create_keyboard_event(source, MACOS_COMMAND_KEYCODE, 1)?);

    let v_down = create_keyboard_event(source, MACOS_V_KEYCODE, 1)?;
    unsafe { CGEventSetFlags(v_down, K_CG_EVENT_FLAG_MASK_COMMAND) };
    events.push(v_down);

    let v_up = create_keyboard_event(source, MACOS_V_KEYCODE, 0)?;
    unsafe { CGEventSetFlags(v_up, K_CG_EVENT_FLAG_MASK_COMMAND) };
    events.push(v_up);

    events.push(create_keyboard_event(source, MACOS_COMMAND_KEYCODE, 0)?);
    Ok(())
}

fn create_keyboard_event(
    source: *mut c_void,
    keycode: u16,
    key_down: Boolean,
) -> Result<CGEventRef> {
    let event = unsafe { CGEventCreateKeyboardEvent(source, keycode, key_down) };
    if event.is_null() {
        bail!(
            "could not create macOS paste keyboard event (keycode {keycode}, key_down {key_down})"
        );
    }
    Ok(event)
}

fn release_event_source(source: *mut c_void) {
    if !source.is_null() {
        unsafe {
            CFRelease(source.cast());
        }
    }
}

/// Poll the HID system key state until the push-to-talk keys are released.
///
/// # Arguments
///
/// * `timeout` - Maximum time to wait before withholding the paste chord.
///
/// # Returns
///
/// `true` when both keys are up; `false` when either key remains down at the
/// deadline.
fn wait_for_ptt_keys_released(timeout: Duration) -> bool {
    wait_for_ptt_keys_released_with(timeout, ptt_key_down)
}

fn wait_for_ptt_keys_released_with(timeout: Duration, key_down: impl Fn(u16) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let ctrl_down = key_down(MACOS_PTT_LEFT_CONTROL_KEYCODE);
        let space_down = key_down(MACOS_PTT_SPACE_KEYCODE);
        if !ctrl_down && !space_down {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(PTT_RELEASE_POLL);
    }
}

fn ptt_key_down(keycode: u16) -> bool {
    unsafe { CGEventSourceKeyState(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE, keycode) }
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
        thread::sleep(SMOKE_YIELD);
    }
}

fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct DropSpy(Arc<AtomicBool>);

    impl Drop for DropSpy {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn smoke_guard_tears_down_when_action_panics() {
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropSpy(Arc::clone(&dropped));
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let _ = run_smoke_action(
                guard,
                || -> Result<()> { panic!("synthetic smoke action panic") },
                || {},
            );
        }));
        assert!(unwind.is_err());
        assert!(dropped.load(Ordering::Acquire));
    }

    #[test]
    fn released_ptt_keys_are_ready_without_waiting() {
        assert!(wait_for_ptt_keys_released_with(Duration::ZERO, |_| false));
    }

    #[test]
    fn held_ptt_key_at_deadline_withholds_paste() {
        assert!(!wait_for_ptt_keys_released_with(Duration::ZERO, |key| {
            key == MACOS_PTT_LEFT_CONTROL_KEYCODE
        }));
    }
}
