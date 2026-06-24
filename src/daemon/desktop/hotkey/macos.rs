//! macOS CoreGraphics push-to-talk hotkey tap.

use super::{send_hotkey_transition, HotkeyAction, HotkeyBackend, HotkeyState, MacOsModifierState};
use crate::daemon::logging::Logger;
use crate::daemon::recording::HotkeyTransition;
use crossbeam_channel::Sender;
use rdev::Key;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
/// CoreGraphics key-down event type used by macOS hotkey tests.
pub(super) const K_CG_EVENT_KEY_DOWN: u32 = 10;
/// CoreGraphics key-up event type used by macOS hotkey tests.
pub(super) const K_CG_EVENT_KEY_UP: u32 = 11;
/// CoreGraphics modifier-state event type used by macOS hotkey tests.
pub(super) const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: i32 = 1;
/// Virtual keycode for Space in the macOS hardware-independent key map.
pub(super) const MACOS_KEY_SPACE: i64 = 49;
const MACOS_KEY_RIGHT_COMMAND: i64 = 54;
const MACOS_KEY_LEFT_COMMAND: i64 = 55;
const MACOS_KEY_LEFT_SHIFT: i64 = 56;
const MACOS_KEY_LEFT_OPTION: i64 = 58;
const MACOS_KEY_LEFT_CONTROL: i64 = 59;
const MACOS_KEY_RIGHT_SHIFT: i64 = 60;
const MACOS_KEY_RIGHT_OPTION: i64 = 61;
const MACOS_KEY_RIGHT_CONTROL: i64 = 62;

type Boolean = u8;
type CFAllocatorRef = *const c_void;
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

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;

    fn CFRelease(cf: CFTypeRef);
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
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
    fn CGEventTapEnable(tap: CFMachPortRef, enable: Boolean);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

/// Run the macOS hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
/// * `_backend` - Ignored backend preference on macOS.
/// * `log` - Logger used for backend diagnostics.
pub(crate) fn run_grab_loop(
    tx: Sender<HotkeyTransition>,
    _backend: HotkeyBackend,
    log: Arc<Logger>,
) {
    log.verbose("parakit: macOS hotkey backend: CoreGraphics session event tap Left Control+Space");
    run_event_tap_loop_or_exit(tx);
}

fn run_event_tap_loop_or_exit(tx: Sender<HotkeyTransition>) {
    if let Err(err) = run_event_tap_loop(tx) {
        eprintln!(
            "parakit: macOS hotkey event tap failed: {err:#}\n{}",
            crate::daemon::hotkey_help::macos_failure_help()
        );
        std::process::exit(2);
    }
}

fn run_event_tap_loop(tx: Sender<HotkeyTransition>) -> anyhow::Result<()> {
    crate::daemon::macos::event_tap_preflight()?;

    let state = Box::new(MacOsHotkeyTapState {
        hotkey: Arc::new(Mutex::new(HotkeyState::default())),
        tx,
        tap: AtomicPtr::new(ptr::null_mut()),
    });
    // CFRunLoopRun owns the normal daemon lifetime. Keep the tap state alive
    // until process exit; setup error paths below reclaim it before the loop.
    let state_ptr = Box::into_raw(state);
    let mask = event_mask(K_CG_EVENT_KEY_DOWN)
        | event_mask(K_CG_EVENT_KEY_UP)
        | event_mask(K_CG_EVENT_FLAGS_CHANGED);
    let tap = unsafe {
        CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            hotkey_tap_callback,
            state_ptr.cast(),
        )
    };
    if tap.is_null() {
        unsafe {
            drop(Box::from_raw(state_ptr));
        }
        anyhow::bail!("could not create CoreGraphics session event tap");
    }

    unsafe {
        (*state_ptr).tap.store(tap.cast(), Ordering::Release);
    }

    let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if source.is_null() {
        unsafe {
            CFRelease(tap.cast());
            drop(Box::from_raw(state_ptr));
        }
        anyhow::bail!("could not create CoreGraphics event-tap run-loop source");
    }

    unsafe {
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, 1);
        CFRelease(source.cast());
        CFRunLoopRun();
    }
    Ok(())
}

struct MacOsHotkeyTapState {
    hotkey: Arc<Mutex<HotkeyState>>,
    tx: Sender<HotkeyTransition>,
    tap: AtomicPtr<c_void>,
}

extern "C" fn hotkey_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    catch_unwind(AssertUnwindSafe(|| {
        hotkey_tap_callback_inner(event_type, event, user_info)
    }))
    .unwrap_or(event)
}

fn hotkey_tap_callback_inner(
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    let state = unsafe { &*(user_info.cast::<MacOsHotkeyTapState>()) };
    match event_type {
        K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT | K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT => {
            let action = match state.hotkey.lock() {
                Ok(mut hotkey) => hotkey.reset_after_tap_disabled(Instant::now()),
                Err(_) => {
                    reenable_tap(state);
                    return event;
                }
            };
            if let Some(action) = action {
                send_hotkey_transition(action, &state.tx);
            }
            reenable_tap(state);
            return event;
        }
        K_CG_EVENT_FLAGS_CHANGED | K_CG_EVENT_KEY_DOWN | K_CG_EVENT_KEY_UP => {}
        _ => return event,
    }

    if event.is_null() {
        return event;
    }

    let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
    let now = Instant::now();
    let modifiers = physical_modifier_state();
    let (action, suppress) = {
        let Ok(mut hotkey) = state.hotkey.lock() else {
            return event;
        };
        handle_tap_event(&mut hotkey, event_type, keycode, modifiers, now)
    };

    if let Some(action) = action {
        send_hotkey_transition(action, &state.tx);
    }
    if suppress {
        ptr::null_mut()
    } else {
        event
    }
}

fn reenable_tap(state: &MacOsHotkeyTapState) {
    let tap: CFMachPortRef = state.tap.load(Ordering::Acquire).cast();
    if !tap.is_null() {
        unsafe {
            CGEventTapEnable(tap, 1);
        }
    }
}

/// Apply one macOS tap event to the logical hotkey state.
///
/// # Arguments
///
/// * `hotkey` - Mutable hotkey state for the active event tap.
/// * `event_type` - CoreGraphics event type.
/// * `keycode` - CoreGraphics keyboard event keycode.
/// * `modifiers` - Physical modifier state sampled from the HID system state.
/// * `now` - Timestamp for any emitted hotkey action.
///
/// # Returns
///
/// The logical hotkey action, if any, and whether the event should be
/// suppressed before it reaches the foreground application.
pub(super) fn handle_tap_event(
    hotkey: &mut HotkeyState,
    event_type: u32,
    keycode: i64,
    modifiers: MacOsModifierState,
    now: Instant,
) -> (Option<HotkeyAction>, bool) {
    match (event_type, keycode) {
        (K_CG_EVENT_FLAGS_CHANGED, _) => (hotkey.macos_sync_modifiers(modifiers, now), false),
        (K_CG_EVENT_KEY_DOWN, MACOS_KEY_SPACE) => {
            let sync_action = hotkey.macos_sync_modifiers(modifiers, now);
            let (space_action, suppress) = hotkey.press(Key::Space, now);
            (sync_action.or(space_action), suppress)
        }
        (K_CG_EVENT_KEY_UP, MACOS_KEY_SPACE) => {
            let sync_action = hotkey.macos_sync_modifiers(modifiers, now);
            let (space_action, suppress) = hotkey.release(Key::Space, now);
            (sync_action.or(space_action), suppress)
        }
        _ => (None, false),
    }
}

fn physical_modifier_state() -> MacOsModifierState {
    MacOsModifierState {
        ctrl_left: physical_key_down(MACOS_KEY_LEFT_CONTROL),
        ctrl_right: physical_key_down(MACOS_KEY_RIGHT_CONTROL),
        shift_left: physical_key_down(MACOS_KEY_LEFT_SHIFT),
        shift_right: physical_key_down(MACOS_KEY_RIGHT_SHIFT),
        alt: physical_key_down(MACOS_KEY_LEFT_OPTION),
        alt_gr: physical_key_down(MACOS_KEY_RIGHT_OPTION),
        meta_left: physical_key_down(MACOS_KEY_LEFT_COMMAND),
        meta_right: physical_key_down(MACOS_KEY_RIGHT_COMMAND),
    }
}

fn physical_key_down(keycode: i64) -> bool {
    unsafe { CGEventSourceKeyState(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE, keycode as u16) }
}

fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
}
