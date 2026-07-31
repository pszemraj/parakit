//! macOS CoreGraphics push-to-talk hotkey tap.

use super::{
    send_hotkey_transition, HotkeyAction, HotkeyBackend, HotkeyState, MacOsModifierState,
    MACOS_LEFT_COMMAND_KEYCODE, MACOS_LEFT_OPTION_KEYCODE, MACOS_LEFT_SHIFT_KEYCODE,
    MACOS_PTT_LEFT_CONTROL_KEYCODE, MACOS_PTT_SPACE_KEYCODE, MACOS_RIGHT_COMMAND_KEYCODE,
    MACOS_RIGHT_CONTROL_KEYCODE, MACOS_RIGHT_OPTION_KEYCODE, MACOS_RIGHT_SHIFT_KEYCODE,
};
use crate::daemon::logging::Logger;
use crate::daemon::macos::cgevent_ffi::{
    event_mask, guarded_tap_callback, install_event_tap, physical_key_down, CFMachPortRef,
    CFRelease, CFRunLoopRun, CGEventGetIntegerValueField, CGEventRef, CGEventTapEnable,
    CGEventTapProxy, EventTapInstallError, K_CG_KEYBOARD_EVENT_KEYCODE,
};
use crate::daemon::recording::HotkeyTransition;
use crossbeam_channel::Sender;
use rdev::Key;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
/// CoreGraphics modifier-state event type used by macOS hotkey tests.
pub(super) use crate::daemon::macos::cgevent_ffi::K_CG_EVENT_FLAGS_CHANGED;
/// CoreGraphics key-down event type used by macOS hotkey tests.
pub(super) use crate::daemon::macos::cgevent_ffi::K_CG_EVENT_KEY_DOWN;
/// CoreGraphics key-up event type used by macOS hotkey tests.
pub(super) use crate::daemon::macos::cgevent_ffi::K_CG_EVENT_KEY_UP;
/// Virtual keycode for Space in the macOS hardware-independent key map.
pub(super) const MACOS_KEY_SPACE: i64 = MACOS_PTT_SPACE_KEYCODE as i64;
const MACOS_KEY_RIGHT_COMMAND: i64 = MACOS_RIGHT_COMMAND_KEYCODE as i64;
const MACOS_KEY_LEFT_COMMAND: i64 = MACOS_LEFT_COMMAND_KEYCODE as i64;
const MACOS_KEY_LEFT_SHIFT: i64 = MACOS_LEFT_SHIFT_KEYCODE as i64;
const MACOS_KEY_LEFT_OPTION: i64 = MACOS_LEFT_OPTION_KEYCODE as i64;
const MACOS_KEY_LEFT_CONTROL: i64 = MACOS_PTT_LEFT_CONTROL_KEYCODE as i64;
const MACOS_KEY_RIGHT_SHIFT: i64 = MACOS_RIGHT_SHIFT_KEYCODE as i64;
const MACOS_KEY_RIGHT_OPTION: i64 = MACOS_RIGHT_OPTION_KEYCODE as i64;
const MACOS_KEY_RIGHT_CONTROL: i64 = MACOS_RIGHT_CONTROL_KEYCODE as i64;

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

    let installed = unsafe { install_event_tap(mask, hotkey_tap_callback, state_ptr.cast()) };
    let (tap, source, _run_loop) = match installed {
        Ok(installed) => installed,
        Err(EventTapInstallError::TapCreateFailed) => {
            unsafe {
                drop(Box::from_raw(state_ptr));
            }
            anyhow::bail!("could not create CoreGraphics session event tap");
        }
        Err(EventTapInstallError::SourceCreateFailed(tap)) => {
            unsafe {
                CFRelease(tap.cast());
                drop(Box::from_raw(state_ptr));
            }
            anyhow::bail!("could not create CoreGraphics event-tap run-loop source");
        }
    };

    unsafe {
        (*state_ptr).tap.store(tap.cast(), Ordering::Release);
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
    guarded_tap_callback(event, || {
        hotkey_tap_callback_inner(event_type, event, user_info)
    })
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

// The MACOS_KEY_* constants above are i64 (matching CGEventGetIntegerValueField's
// return type, which they are compared against elsewhere in this file), while
// the shared physical_key_down takes the u16 keycode ABI type; casting back
// here is lossless since every constant is itself derived from a u16 keycode
// via `as i64`.
fn physical_modifier_state() -> MacOsModifierState {
    MacOsModifierState {
        ctrl_left: physical_key_down(MACOS_KEY_LEFT_CONTROL as u16),
        ctrl_right: physical_key_down(MACOS_KEY_RIGHT_CONTROL as u16),
        shift_left: physical_key_down(MACOS_KEY_LEFT_SHIFT as u16),
        shift_right: physical_key_down(MACOS_KEY_RIGHT_SHIFT as u16),
        alt: physical_key_down(MACOS_KEY_LEFT_OPTION as u16),
        alt_gr: physical_key_down(MACOS_KEY_RIGHT_OPTION as u16),
        meta_left: physical_key_down(MACOS_KEY_LEFT_COMMAND as u16),
        meta_right: physical_key_down(MACOS_KEY_RIGHT_COMMAND as u16),
    }
}
