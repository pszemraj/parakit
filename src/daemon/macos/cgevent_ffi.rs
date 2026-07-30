//! Shared CoreFoundation and CoreGraphics event-tap declarations.

use std::ffi::c_void;

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
