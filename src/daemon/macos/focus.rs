//! macOS focused-Accessibility-element identity snapshots.
//!
//! A snapshot captured at push-to-talk-down is compared against a fresh
//! read immediately before insertion. Matching is by the Accessibility
//! focused UI element's identity (via `CFEqual`), which survives the
//! transient window churn (palettes, popovers, sheets, z-order changes)
//! that made the previous on-screen-window-id comparison unreliable. When
//! Accessibility cannot expose a focused element on either side, matching
//! falls back to frontmost-application pid + bundle identifier.

use anyhow::{bail, Context, Result};
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use std::ffi::c_void;
use std::ptr;
use std::sync::OnceLock;

type AXError = i32;
type AXUIElementRef = *mut c_void;
type Boolean = u8;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type CFStringRef = *const c_void;
type CFTypeID = usize;
type CFTypeRef = *const c_void;

const K_AX_ERROR_SUCCESS: AXError = 0;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFEqual(cf1: CFTypeRef, cf2: CFTypeRef) -> Boolean;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFRelease(cf: CFTypeRef);
    fn CFStringCreateWithBytes(
        alloc: CFAllocatorRef,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: Boolean,
    ) -> CFStringRef;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut u8,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
    fn CFStringGetTypeID() -> CFTypeID;
}

/// Attribute name constants (`kAXFocusedUIElementAttribute` and friends) are
/// preprocessor macros in Apple's headers rather than linkable symbols, so
/// they are built once here from their literal string values and cached for
/// the life of the process (intentionally never released — a handful of
/// interned CFStrings live for as long as the daemon runs).
fn cached_ax_attribute(cache: &'static OnceLock<usize>, name: &'static str) -> CFStringRef {
    let addr = *cache.get_or_init(|| cfstring_from_static_str(name) as usize);
    addr as CFStringRef
}

fn cfstring_from_static_str(s: &'static str) -> CFStringRef {
    let value = unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            s.as_ptr(),
            s.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
            0,
        )
    };
    assert!(
        !value.is_null(),
        "CFStringCreateWithBytes returned null for {s:?}"
    );
    value
}

fn ax_focused_ui_element_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXFocusedUIElement")
}

fn ax_role_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXRole")
}

fn ax_subrole_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXSubrole")
}

fn ax_value_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXValue")
}

/// Owning handle to a single retained Core Foundation object copied from an
/// Accessibility attribute (an `AXUIElementRef` or a `CFStringRef`, both of
/// which are CF-retainable opaque types).
struct AxElementHandle(*mut c_void);

impl AxElementHandle {
    fn as_cftype(&self) -> CFTypeRef {
        self.0.cast_const()
    }
}

impl Drop for AxElementHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0.cast_const()) };
        }
    }
}

impl std::fmt::Debug for AxElementHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AxElementHandle({:p})", self.0)
    }
}

// SAFETY: `AxElementHandle` owns a CF-retained object. Core Foundation's
// retain/release/CFEqual are thread-safe and may be called from any thread;
// the wrapped pointer here is only ever used for `CFEqual` identity
// comparison and read-only AX attribute copies (never for AppKit calls that
// require the main thread), which Accessibility client tooling routinely
// performs off the main thread. `MacOsFocusSnapshot` is moved across the
// recording-coordinator/worker thread boundary through `WorkerEvent`, which
// requires `Send`; `Sync` is additionally required transitively (unrelated
// crates place `Send + Sync` bounds on error types carried through this
// channel). Neither trait allows mutation through a shared reference here:
// `AxElementHandle` exposes no interior mutability, and CF's atomic
// retain-count bookkeeping makes concurrent reads/releases safe.
unsafe impl Send for AxElementHandle {}
unsafe impl Sync for AxElementHandle {}

/// Focused Accessibility element captured for the frontmost application.
///
/// `role`, `subrole`, and `supports_value_polling` are not consulted by the
/// current [`decide_focus_verification`] logic (identity is decided purely
/// by `CFEqual` on `element`); they are captured now so a later
/// acknowledgement/read-back commit does not need to re-plumb Accessibility
/// captures.
#[derive(Debug)]
pub(crate) struct AxElementSnapshot {
    element: AxElementHandle,
    /// `AXRole` of the focused element, when it could be read.
    #[allow(dead_code, reason = "captured for a future acknowledgement commit")]
    role: Option<String>,
    /// `AXSubrole` of the focused element, when it could be read.
    #[allow(dead_code, reason = "captured for a future acknowledgement commit")]
    subrole: Option<String>,
    /// Whether `AXValue` could be read as a string-typed value on the
    /// focused element at capture time.
    supports_value_polling: bool,
}

impl AxElementSnapshot {
    /// Return whether `AXValue` could be read as a string-typed value on
    /// this element at capture time.
    ///
    /// `false` is the expected, deliberate result for secure/password input
    /// fields: macOS withholds `AXValue` from Accessibility clients for
    /// those fields as a privacy boundary (the same protection "Secure
    /// Input" relies on), not a bug in this snapshot. Callers should treat
    /// `false` as "no pollable acknowledgement signal here", not as an
    /// error.
    ///
    /// # Returns
    ///
    /// `true` when post-paste `AXValue` polling is worth attempting.
    pub(crate) fn supports_value_polling(&self) -> bool {
        self.supports_value_polling
    }

    /// Re-read `AXValue` from this element, for post-paste acknowledgement
    /// polling.
    ///
    /// # Returns
    ///
    /// `Ok(Some(value))` when `AXValue` is currently a string; `Ok(None)`
    /// when the read succeeded but the value is not (or is no longer) a
    /// string. `Err(())` when the Accessibility read itself failed, which
    /// in practice means the element died or otherwise stopped answering AX
    /// requests (e.g. focus moved to a different element or application);
    /// callers should treat that as the end of evidence-gathering rather
    /// than a transient error to retry.
    pub(crate) fn poll_value(&self) -> Result<Option<String>, ()> {
        let element: AXUIElementRef = self.element.0;
        let mut value: CFTypeRef = ptr::null();
        let status =
            unsafe { AXUIElementCopyAttributeValue(element, ax_value_attribute(), &mut value) };
        if status != K_AX_ERROR_SUCCESS {
            return Err(());
        }
        if value.is_null() {
            return Ok(None);
        }
        let handle = AxElementHandle(value.cast_mut());
        if unsafe { CFGetTypeID(handle.as_cftype()) } != unsafe { CFStringGetTypeID() } {
            return Ok(None);
        }
        Ok(cfstring_to_string(handle.as_cftype().cast()))
    }
}

/// Sendable representation of the focused macOS insertion target.
#[derive(Debug)]
pub(crate) struct MacOsFocusSnapshot {
    pid: libc::pid_t,
    bundle_identifier: Option<String>,
    ax: Option<AxElementSnapshot>,
}

/// Result of comparing a captured [`MacOsFocusSnapshot`] against a fresh
/// read of the live macOS focus state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FocusVerification {
    /// Same frontmost pid and bundle identifier, and (when Accessibility
    /// exposed a focused element on both sides) the same focused element
    /// identity.
    Matched,
    /// The frontmost application changed, or the focused element identity
    /// no longer matches.
    Changed,
    /// The frontmost pid and bundle identifier still match, but the
    /// Accessibility focused element could not be compared because it was
    /// unavailable on the captured snapshot, the live read, or both.
    AxUnsupported,
}

impl MacOsFocusSnapshot {
    /// Capture the current frontmost application and its focused
    /// Accessibility element.
    ///
    /// # Returns
    ///
    /// A snapshot suitable for comparison immediately before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when macOS reports no frontmost application.
    /// Accessibility read failures at capture time never produce an error;
    /// they simply leave the Accessibility portion of the snapshot empty
    /// (see [`Self::verify_current`] for the pid+bundle fallback this
    /// enables).
    pub(crate) fn capture() -> Result<Self> {
        frontmost_application_window().context("could not capture macOS frontmost window")
    }

    /// Return whether the current frontmost focus still matches this
    /// snapshot.
    ///
    /// # Returns
    ///
    /// `true` unless [`Self::verify_current`] reports [`FocusVerification::Changed`].
    pub(crate) fn matches_current(&self) -> bool {
        !matches!(self.verify_current(), FocusVerification::Changed)
    }

    /// Compare this snapshot against a fresh read of the live macOS focus
    /// state.
    ///
    /// # Returns
    ///
    /// The verification outcome: [`FocusVerification::Matched`] when pid,
    /// bundle identifier, and focused Accessibility element identity all
    /// agree; [`FocusVerification::AxUnsupported`] when pid and bundle
    /// identifier agree but Accessibility could not expose a focused
    /// element on one or both sides; [`FocusVerification::Changed`]
    /// otherwise (including when the live frontmost application cannot be
    /// read at all).
    pub(crate) fn verify_current(&self) -> FocusVerification {
        let current = match frontmost_application_window() {
            Ok(current) => current,
            Err(_) => return FocusVerification::Changed,
        };
        decide_focus_verification(
            self.pid == current.pid,
            self.bundle_identifier == current.bundle_identifier,
            ax_identity_equal(self.ax.as_ref(), current.ax.as_ref()),
        )
    }

    /// Return the bundle identifier of the captured frontmost application,
    /// when macOS reported one.
    ///
    /// # Returns
    ///
    /// The bundle identifier string, or `None` when the frontmost
    /// application had no bundle identifier.
    pub(crate) fn bundle_id(&self) -> Option<&str> {
        self.bundle_identifier.as_deref()
    }

    /// Return the focused Accessibility element captured for this snapshot,
    /// when Accessibility exposed one.
    ///
    /// # Returns
    ///
    /// `Some` when a focused element was captured; `None` when
    /// Accessibility could not expose one (permission not granted, no
    /// focused element, etc. — see [`Self::capture`]).
    pub(crate) fn ax_element(&self) -> Option<&AxElementSnapshot> {
        self.ax.as_ref()
    }
}

/// Pure decision logic for [`FocusVerification`], factored out of the live
/// Accessibility/AppKit reads so it can be unit-tested without an
/// Accessibility permission grant.
///
/// # Arguments
///
/// * `same_pid` - Whether the live frontmost pid matches the captured pid.
/// * `same_bundle` - Whether the live frontmost bundle identifier matches
///   the captured bundle identifier.
/// * `ax_equal` - `Some(true)`/`Some(false)` when both sides exposed a
///   focused Accessibility element and could be compared via `CFEqual`;
///   `None` when Accessibility was unavailable on either side.
fn decide_focus_verification(
    same_pid: bool,
    same_bundle: bool,
    ax_equal: Option<bool>,
) -> FocusVerification {
    if !same_pid || !same_bundle {
        return FocusVerification::Changed;
    }
    match ax_equal {
        Some(true) => FocusVerification::Matched,
        Some(false) => FocusVerification::Changed,
        None => FocusVerification::AxUnsupported,
    }
}

fn ax_identity_equal(
    expected: Option<&AxElementSnapshot>,
    current: Option<&AxElementSnapshot>,
) -> Option<bool> {
    let (expected, current) = (expected?, current?);
    Some(unsafe { CFEqual(expected.element.as_cftype(), current.element.as_cftype()) != 0 })
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
        let ax = capture_ax_focused_element(pid);
        Ok(MacOsFocusSnapshot {
            pid,
            bundle_identifier,
            ax,
        })
    })
}

/// Best-effort capture of the focused Accessibility element for `pid`'s
/// application. Returns `None` on any Accessibility failure (permission not
/// granted, application has no AX focused element, etc.) rather than an
/// error, per the capture-failure tolerance policy: an Accessibility read
/// failure at capture time must never block dictation.
fn capture_ax_focused_element(pid: libc::pid_t) -> Option<AxElementSnapshot> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    // Held only to release the application AXUIElementRef when this
    // function returns; the focused-element copy below is independently
    // retained in `element`.
    let _app = AxElementHandle(app);

    let element = copy_ax_element(app, ax_focused_ui_element_attribute())?;
    let role = copy_ax_string(element.0, ax_role_attribute());
    let subrole = copy_ax_string(element.0, ax_subrole_attribute());
    let supports_value_polling = ax_value_is_string(element.0);
    Some(AxElementSnapshot {
        element,
        role,
        subrole,
        supports_value_polling,
    })
}

fn copy_ax_element(element: AXUIElementRef, attribute: CFStringRef) -> Option<AxElementHandle> {
    let mut value: CFTypeRef = ptr::null();
    let status = unsafe { AXUIElementCopyAttributeValue(element, attribute, &mut value) };
    if status != K_AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    Some(AxElementHandle(value.cast_mut()))
}

fn copy_ax_string(element: AXUIElementRef, attribute: CFStringRef) -> Option<String> {
    let handle = copy_ax_element(element, attribute)?;
    if unsafe { CFGetTypeID(handle.as_cftype()) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    cfstring_to_string(handle.as_cftype().cast())
}

/// Return whether copying `AXValue` on `element` succeeds right now and
/// yields a string-typed value. The value itself is discarded; only the
/// capability is recorded (see [`AxElementSnapshot::supports_value_polling`]).
fn ax_value_is_string(element: AXUIElementRef) -> bool {
    let Some(handle) = copy_ax_element(element, ax_value_attribute()) else {
        return false;
    };
    unsafe { CFGetTypeID(handle.as_cftype()) == CFStringGetTypeID() }
}

fn cfstring_to_string(value: CFStringRef) -> Option<String> {
    let length = unsafe { CFStringGetLength(value) };
    if length < 0 {
        return None;
    }
    // Each UTF-16 code unit needs at most 3 UTF-8 bytes; surrogate pairs (2
    // units) need at most 4, comfortably under this bound.
    let capacity = (length as usize).saturating_mul(3).saturating_add(1);
    let mut buffer = vec![0_u8; capacity];
    let ok = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            capacity as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        ) != 0
    };
    if !ok {
        return None;
    }
    let nul = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    buffer.truncate(nul);
    String::from_utf8(buffer).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_matches_when_pid_bundle_and_ax_element_agree() {
        assert_eq!(
            decide_focus_verification(true, true, Some(true)),
            FocusVerification::Matched
        );
    }

    #[test]
    fn decision_changes_when_ax_element_identity_differs() {
        assert_eq!(
            decide_focus_verification(true, true, Some(false)),
            FocusVerification::Changed
        );
    }

    #[test]
    fn decision_falls_back_when_ax_is_unavailable_on_either_side() {
        assert_eq!(
            decide_focus_verification(true, true, None),
            FocusVerification::AxUnsupported
        );
    }

    #[test]
    fn decision_changes_when_bundle_identifier_differs() {
        assert_eq!(
            decide_focus_verification(true, false, Some(true)),
            FocusVerification::Changed
        );
    }

    #[test]
    fn decision_changes_when_pid_differs() {
        assert_eq!(
            decide_focus_verification(false, true, Some(true)),
            FocusVerification::Changed
        );
    }
}
