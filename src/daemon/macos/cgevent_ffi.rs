//! Shared CoreFoundation and CoreGraphics event-tap declarations.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

/// CoreFoundation Boolean ABI type.
pub(in crate::daemon) type Boolean = u8;
/// Opaque CoreFoundation allocator reference.
pub(in crate::daemon) type CFAllocatorRef = *const c_void;
/// CoreFoundation signed index ABI type.
pub(in crate::daemon) type CFIndex = isize;
/// Opaque CoreFoundation Mach-port reference.
pub(in crate::daemon) type CFMachPortRef = *mut c_void;
/// Opaque CoreFoundation run-loop reference.
pub(in crate::daemon) type CFRunLoopRef = *mut c_void;
/// Opaque CoreFoundation run-loop-source reference.
pub(in crate::daemon) type CFRunLoopSourceRef = *mut c_void;
/// Opaque CoreFoundation string reference.
pub(in crate::daemon) type CFStringRef = *const c_void;
/// Opaque CoreFoundation object reference.
pub(in crate::daemon) type CFTypeRef = *const c_void;
/// Opaque CoreGraphics event reference.
pub(in crate::daemon) type CGEventRef = *mut c_void;
/// Opaque CoreGraphics event-tap proxy.
pub(in crate::daemon) type CGEventTapProxy = *mut c_void;
/// CoreGraphics event-tap callback ABI.
pub(in crate::daemon) type CGEventTapCallBack =
    extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

/// CoreGraphics HID event-tap location.
pub(in crate::daemon) const K_CG_HID_EVENT_TAP: u32 = 0;
/// CoreGraphics session event-tap location.
pub(in crate::daemon) const K_CG_SESSION_EVENT_TAP: u32 = 1;
/// CoreGraphics head-insert event-tap placement.
pub(in crate::daemon) const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
/// CoreGraphics default event-tap behavior.
pub(in crate::daemon) const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
/// CoreGraphics key-down event type.
pub(in crate::daemon) const K_CG_EVENT_KEY_DOWN: u32 = 10;
/// CoreGraphics key-up event type.
pub(in crate::daemon) const K_CG_EVENT_KEY_UP: u32 = 11;
/// CoreGraphics modifier-flags-changed event type.
pub(in crate::daemon) const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
/// CoreGraphics integer field containing a keyboard event's keycode.
pub(in crate::daemon) const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
/// CoreGraphics event flag for the Command modifier.
pub(in crate::daemon) const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
/// CoreGraphics event-source state for hardware-level key state.
pub(in crate::daemon) const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: i32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub(in crate::daemon) static kCFRunLoopDefaultMode: CFStringRef;

    pub(in crate::daemon) fn CFRelease(cf: CFTypeRef);
    pub(in crate::daemon) fn CFRunLoopAddSource(
        rl: CFRunLoopRef,
        source: CFRunLoopSourceRef,
        mode: CFStringRef,
    );
    pub(in crate::daemon) fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub(in crate::daemon) fn CFRunLoopRemoveSource(
        rl: CFRunLoopRef,
        source: CFRunLoopSourceRef,
        mode: CFStringRef,
    );
    pub(in crate::daemon) fn CFRunLoopRun();
    pub(in crate::daemon) fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: f64,
        return_after_source_handled: Boolean,
    ) -> i32;
    pub(in crate::daemon) fn CFMachPortInvalidate(port: CFMachPortRef);
    pub(in crate::daemon) fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub(in crate::daemon) fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub(in crate::daemon) fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtual_key: u16,
        key_down: Boolean,
    ) -> CGEventRef;
    pub(in crate::daemon) fn CGEventPost(tap: u32, event: CGEventRef);
    pub(in crate::daemon) fn CGEventGetFlags(event: CGEventRef) -> u64;
    pub(in crate::daemon) fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub(in crate::daemon) fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub(in crate::daemon) fn CGEventTapEnable(tap: CFMachPortRef, enable: Boolean);
    pub(in crate::daemon) fn CGEventSourceCreate(state_id: i32) -> *mut c_void;
    pub(in crate::daemon) fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

/// Return the mask bit for a CoreGraphics event type.
///
/// # Returns
///
/// A mask with the requested event-type bit set.
pub(in crate::daemon) const fn event_mask(event_type: u32) -> u64 {
    1_u64 << event_type
}

/// Failure from [`install_event_tap`], carrying whatever CoreGraphics object
/// was already created so the caller can pick its own diagnostic wording and
/// perform its own teardown.
pub(in crate::daemon) enum EventTapInstallError {
    /// `CGEventTapCreate` returned a null Mach port; nothing to release.
    TapCreateFailed,
    /// `CFMachPortCreateRunLoopSource` returned a null source. The Mach port
    /// from `CGEventTapCreate` was created successfully and is returned here
    /// so the caller can release it.
    SourceCreateFailed(CFMachPortRef),
}

/// Install a session-scoped, head-insert CoreGraphics event tap: create it,
/// create and register its run-loop source on the current run loop, and
/// enable it.
///
/// Every caller in this crate installs the tap with the same location,
/// placement, and options (`K_CG_SESSION_EVENT_TAP`,
/// `K_CG_HEAD_INSERT_EVENT_TAP`, `K_CG_EVENT_TAP_OPTION_DEFAULT`), so only
/// the parts that vary per caller — the event mask, callback, and opaque
/// `user_info` pointer — are parameters here. What happens after a
/// successful install (teardown on drop vs. running forever) is left to the
/// caller, since that is exactly where the callers differ.
///
/// # Arguments
///
/// * `mask` - Event-type mask to intercept, built from [`event_mask`].
/// * `callback` - Extern "C" callback CoreGraphics invokes per event.
/// * `user_info` - Opaque pointer passed back to `callback` on every call.
///
/// # Returns
///
/// The installed tap's Mach port, its run-loop source, and the run loop it
/// was registered on.
///
/// # Errors
///
/// Returns [`EventTapInstallError`] when the tap or its run-loop source
/// could not be created.
///
/// # Safety
///
/// `callback` must be prepared to be invoked by CoreGraphics for as long as
/// the returned tap stays installed and enabled, and `user_info` must remain
/// valid for that same span.
pub(in crate::daemon) unsafe fn install_event_tap(
    mask: u64,
    callback: CGEventTapCallBack,
    user_info: *mut c_void,
) -> Result<(CFMachPortRef, CFRunLoopSourceRef, CFRunLoopRef), EventTapInstallError> {
    let tap = CGEventTapCreate(
        K_CG_SESSION_EVENT_TAP,
        K_CG_HEAD_INSERT_EVENT_TAP,
        K_CG_EVENT_TAP_OPTION_DEFAULT,
        mask,
        callback,
        user_info,
    );
    if tap.is_null() {
        return Err(EventTapInstallError::TapCreateFailed);
    }

    let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
    if source.is_null() {
        return Err(EventTapInstallError::SourceCreateFailed(tap));
    }

    let run_loop = CFRunLoopGetCurrent();
    CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
    CGEventTapEnable(tap, 1);
    Ok((tap, source, run_loop))
}

/// Run a CGEvent-tap callback body behind an unwind guard.
///
/// CoreGraphics event-tap callbacks are `extern "C"` functions invoked from
/// the tap's run loop; a panic unwinding across that boundary is undefined
/// behavior, so every tap callback in this crate routes its inner logic
/// through this guard instead of calling it directly.
///
/// # Arguments
///
/// * `event` - The original CGEvent, returned unmodified if `body` panics.
/// * `body` - Callback logic to run under `catch_unwind`.
///
/// # Returns
///
/// The event `body` returns on success, or `event` unchanged if `body` panicked.
pub(in crate::daemon) fn guarded_tap_callback(
    event: CGEventRef,
    body: impl FnOnce() -> CGEventRef,
) -> CGEventRef {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(event)
}

/// Return whether a physical key is currently held down, per the HID system
/// event source.
///
/// # Arguments
///
/// * `keycode` - macOS virtual keycode to query.
///
/// # Returns
///
/// `true` when CoreGraphics reports the key as physically down right now.
pub(in crate::daemon) fn physical_key_down(keycode: u16) -> bool {
    unsafe { CGEventSourceKeyState(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE, keycode) }
}
