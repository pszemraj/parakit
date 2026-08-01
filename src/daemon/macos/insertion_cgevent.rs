//! macOS CGEvent-based paste shortcut and insertion smoke-test helpers.

use super::cgevent_ffi::{
    event_mask, guarded_tap_callback, install_event_tap, kCFRunLoopDefaultMode, physical_key_down,
    Boolean, CFMachPortInvalidate, CFMachPortRef, CFRelease, CFRunLoopRef, CFRunLoopRemoveSource,
    CFRunLoopRunInMode, CFRunLoopSourceRef, CGEventCreateKeyboardEvent, CGEventGetFlags,
    CGEventGetIntegerValueField, CGEventPost, CGEventRef, CGEventSetFlags, CGEventSourceCreate,
    CGEventTapEnable, CGEventTapProxy, EventTapInstallError, K_CG_EVENT_FLAGS_CHANGED,
    K_CG_EVENT_FLAG_MASK_COMMAND, K_CG_EVENT_KEY_DOWN, K_CG_EVENT_KEY_UP,
    K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE, K_CG_HID_EVENT_TAP, K_CG_KEYBOARD_EVENT_KEYCODE,
};
use super::permissions::event_tap_preflight;
use crate::daemon::desktop::hotkey::{
    MACOS_LEFT_COMMAND_KEYCODE, MACOS_LEFT_OPTION_KEYCODE, MACOS_LEFT_SHIFT_KEYCODE,
    MACOS_PTT_LEFT_CONTROL_KEYCODE, MACOS_PTT_SPACE_KEYCODE, MACOS_RIGHT_COMMAND_KEYCODE,
    MACOS_RIGHT_CONTROL_KEYCODE, MACOS_RIGHT_OPTION_KEYCODE, MACOS_RIGHT_SHIFT_KEYCODE,
};
use anyhow::{bail, Result};
use std::ffi::c_void;
#[cfg(test)]
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MACOS_V_KEYCODE: u16 = 9;
const MACOS_FUNCTION_KEYCODE: u16 = 63;

/// Physical keys that must be released before a synthetic Cmd+V chord.
///
/// Modifier keys can change the chord's meaning. The configured PTT Space
/// key is also included so insertion waits for the triggering key release;
/// it is not a `CGEventFlags` modifier. Caps Lock is deliberately absent:
/// it does not alter paste, and its latched state must not block insertion.
const MACOS_PASTE_CONFLICT_KEYCODES: &[u16] = &[
    MACOS_PTT_SPACE_KEYCODE,
    MACOS_RIGHT_COMMAND_KEYCODE,
    MACOS_LEFT_COMMAND_KEYCODE,
    MACOS_LEFT_SHIFT_KEYCODE,
    MACOS_LEFT_OPTION_KEYCODE,
    MACOS_PTT_LEFT_CONTROL_KEYCODE,
    MACOS_RIGHT_SHIFT_KEYCODE,
    MACOS_RIGHT_OPTION_KEYCODE,
    MACOS_RIGHT_CONTROL_KEYCODE,
    MACOS_FUNCTION_KEYCODE,
];

const SMOKE_TIMEOUT: Duration = Duration::from_millis(750);
const SMOKE_POLL: Duration = Duration::from_millis(20);
/// Yield between run-loop slices so the smoke wait never becomes a spin loop
/// when `CFRunLoopRunInMode` returns early with nothing to dispatch.
const SMOKE_YIELD: Duration = Duration::from_millis(5);
/// Maximum time a paste transaction waits for physical PTT/modifier keys to
/// be released before withholding the synthetic chord.
pub(crate) const PASTE_MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const PASTE_MODIFIER_RELEASE_POLL: Duration = Duration::from_millis(15);

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
        let installed = unsafe {
            install_event_tap(
                mask,
                smoke_tap_callback,
                (state as *const SmokeTapState).cast_mut().cast(),
            )
        };
        let (tap, source, run_loop) = match installed {
            Ok(installed) => installed,
            Err(EventTapInstallError::TapCreateFailed) => bail!(
                "could not create macOS event tap for insertion smoke test; grant Accessibility and Input Monitoring to your terminal and rerun parakit doctor --deep"
            ),
            Err(EventTapInstallError::SourceCreateFailed(tap)) => {
                unsafe {
                    CFMachPortInvalidate(tap);
                    CFRelease(tap.cast());
                }
                bail!("could not create macOS event-tap run-loop source");
            }
        };
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
    /// The chord was withheld because a conflicting physical key was down.
    UnsafeModifiers,
}

/// Wait until no physical key can alter the synthetic Cmd+V chord.
///
/// This wait intentionally runs before the paste transaction's final focus
/// recheck. Short dictations can finish before the push-to-talk keys are
/// naturally released; waiting afterward would leave a stale focus snapshot
/// able to authorize a chord in whichever application became frontmost during
/// the wait.
///
/// # Returns
///
/// `true` when every conflicting key is up before the bounded deadline;
/// `false` when any remains down.
pub(crate) fn wait_for_safe_paste_modifiers() -> bool {
    wait_for_safe_paste_modifiers_with(PASTE_MODIFIER_RELEASE_TIMEOUT, physical_key_down)
}

/// Send a macOS paste shortcut as a full Cmd+V hardware-style chord.
///
/// # Returns
///
/// [`PasteShortcutOutcome::Sent`] when CoreGraphics accepted the synthetic
/// Cmd+V key events, or [`PasteShortcutOutcome::UnsafeModifiers`] when a
/// physical key became unsafe after the caller's bounded wait and no event
/// was posted.
///
/// # Errors
///
/// Returns an error if CoreGraphics cannot allocate the keyboard events.
///
/// Callers must run `accessibility_preflight()` before choosing this backend.
/// The daemon and `doctor --deep` both do that once before entering this hot path.
pub(crate) fn send_paste_shortcut() -> Result<PasteShortcutOutcome> {
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

    // CoreGraphics merges live hardware flags into synthetic events. Check
    // every conflicting key immediately before posting, after event
    // allocation, so a modifier pressed during clipboard staging or the
    // final focus check cannot turn Cmd+V into another shortcut.
    if !safe_paste_modifiers_with(physical_key_down) {
        release_events(&events);
        release_event_source(source);
        return Ok(PasteShortcutOutcome::UnsafeModifiers);
    }

    unsafe {
        for event in &events {
            CGEventPost(K_CG_HID_EVENT_TAP, *event);
        }
    }
    release_events(&events);
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
            release_events(&events);
            Err(err)
        }
    }
}

fn build_paste_chord_events(source: *mut c_void, events: &mut Vec<CGEventRef>) -> Result<()> {
    events.push(create_keyboard_event(
        source,
        MACOS_LEFT_COMMAND_KEYCODE,
        1,
    )?);

    let v_down = create_keyboard_event(source, MACOS_V_KEYCODE, 1)?;
    unsafe { CGEventSetFlags(v_down, K_CG_EVENT_FLAG_MASK_COMMAND) };
    events.push(v_down);

    let v_up = create_keyboard_event(source, MACOS_V_KEYCODE, 0)?;
    unsafe { CGEventSetFlags(v_up, K_CG_EVENT_FLAG_MASK_COMMAND) };
    events.push(v_up);

    events.push(create_keyboard_event(
        source,
        MACOS_LEFT_COMMAND_KEYCODE,
        0,
    )?);
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

fn release_events(events: &[CGEventRef]) {
    for event in events {
        unsafe { CFRelease(event.cast()) };
    }
}

/// Poll the HID system key state until paste-conflicting keys are released.
///
/// # Arguments
///
/// * `timeout` - Maximum time to wait before withholding the paste chord.
///
/// # Returns
///
/// `true` when every conflicting key is up; `false` when any remains down at
/// the deadline.
fn wait_for_safe_paste_modifiers_with(timeout: Duration, key_down: impl Fn(u16) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if safe_paste_modifiers_with(&key_down) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(PASTE_MODIFIER_RELEASE_POLL);
    }
}

fn safe_paste_modifiers_with(key_down: impl Fn(u16) -> bool) -> bool {
    !MACOS_PASTE_CONFLICT_KEYCODES.iter().copied().any(key_down)
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
    guarded_tap_callback(event, || {
        smoke_tap_callback_inner(event_type, event, user_info)
    })
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
    fn paste_modifier_wait_withholds_only_for_conflict_keycodes() {
        // (label, held physical key, expect ready at deadline)
        let mut cases: Vec<(&str, Option<u16>, bool)> =
            vec![("no_key_held_is_ready_without_waiting", None, true)];
        for &keycode in MACOS_PASTE_CONFLICT_KEYCODES {
            cases.push((
                "conflicting_key_withholds_paste_at_deadline",
                Some(keycode),
                false,
            ));
        }
        cases.push((
            "unrelated_physical_key_does_not_withhold_paste",
            Some(MACOS_V_KEYCODE),
            true,
        ));

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|&(label, held, expect)| {
                let ready = wait_for_safe_paste_modifiers_with(Duration::ZERO, move |key| {
                    Some(key) == held
                });
                (ready != expect).then(|| {
                    format!("{label} (held={held:?}): expected ready={expect}, got {ready}")
                })
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
