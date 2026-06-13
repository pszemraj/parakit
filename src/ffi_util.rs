//! Small helpers for local FFI boundaries.

use std::ffi::CStr;
use std::os::raw::c_char;

/// Convert a nullable C string pointer to an owned Rust string.
///
/// # Returns
///
/// An empty string for null pointers, otherwise a lossy UTF-8 conversion of
/// the NUL-terminated string.
pub(crate) fn c_string_lossy(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        // SAFETY: CrispASR and ggml return NUL-terminated strings for these
        // APIs, and null pointers are handled above.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}
