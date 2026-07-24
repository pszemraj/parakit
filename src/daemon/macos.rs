//! macOS desktop permission, focus, and diagnostic helpers.

#![cfg(target_os = "macos")]

mod focus;
mod insertion_cgevent;
/// Post-paste Accessibility acknowledgement (`AXValue` confirmation polling).
pub(crate) mod pasteboard;
mod permissions;

pub(crate) use focus::{AxElementSnapshot, FocusVerification, MacOsFocusSnapshot};
pub(crate) use insertion_cgevent::{
    send_paste_shortcut, suppressed_key_event_smoke, suppressed_paste_shortcut_smoke,
};
#[allow(unused_imports)]
pub(crate) use permissions::{
    accessibility_preflight, architecture_warning_lines, event_tap_preflight, microphone_preflight,
    no_gpu_hint, permission_report, PermissionReport, PermissionStatus,
};
