//! macOS focused-Accessibility-element identity snapshots.
//!
//! A snapshot captured at push-to-talk-down is compared against a fresh
//! read immediately before insertion. Matching is by the Accessibility
//! focused UI element's identity (via `CFEqual`), which survives the
//! transient window churn (palettes, popovers, sheets, z-order changes)
//! that made the previous on-screen-window-id comparison unreliable.
//! Frontmost-application identity is still checked first, but it is not a
//! substitute for focused-element identity: when Accessibility cannot expose
//! a focused element on either side, automatic insertion is not authorized.

use super::cgevent_ffi::{Boolean, CFAllocatorRef, CFIndex, CFRelease, CFStringRef, CFTypeRef};
use crate::daemon::desktop::{
    clipboard_restore::{PasteTargetSelection, PasteTargetValue},
    FocusVerification,
};
use anyhow::{bail, Context, Result};
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use std::ffi::c_void;
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

type AXError = i32;
type AXUIElementRef = *mut c_void;
type AXValueType = u32;
type CFTypeID = usize;

#[repr(C)]
#[derive(Clone, Copy)]
struct CFRange {
    location: CFIndex,
    length: CFIndex,
}

const K_AX_ERROR_SUCCESS: AXError = 0;
const K_AX_VALUE_CF_RANGE_TYPE: AXValueType = 4;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Per-call timeout applied to every application and focused Accessibility
/// element used by the paste transaction. The outer confirmation deadline
/// is checked between polls; this bound prevents one unresponsive target
/// from trapping the worker inside a single AX request for the system
/// default timeout.
pub(crate) const AX_MESSAGE_TIMEOUT: Duration = Duration::from_millis(250);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_in_seconds: f32) -> AXError;
    fn AXValueGetType(value: CFTypeRef) -> AXValueType;
    fn AXValueGetValue(
        value: CFTypeRef,
        value_type: AXValueType,
        value_ptr: *mut c_void,
    ) -> Boolean;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFEqual(cf1: CFTypeRef, cf2: CFTypeRef) -> Boolean;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFStringCreateWithBytes(
        alloc: CFAllocatorRef,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: Boolean,
    ) -> CFStringRef;
    fn CFStringGetCharacters(the_string: CFStringRef, range: CFRange, buffer: *mut u16);
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

fn ax_value_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXValue")
}

fn ax_selected_text_range_attribute() -> CFStringRef {
    static CACHE: OnceLock<usize> = OnceLock::new();
    cached_ax_attribute(&CACHE, "AXSelectedTextRange")
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
/// Target identity is decided purely by `CFEqual` on `element` (see
/// [`decide_focus_verification`]); `supports_value_polling` only records
/// whether post-paste acknowledgement is worth attempting.
#[derive(Debug)]
pub(crate) struct AxElementSnapshot {
    element: AxElementHandle,
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
    /// `Ok(Some(value))` when `AXValue` is currently a string; the returned
    /// leading/trailing portions contain at most `max_utf16_units` total
    /// UTF-16 code units. `Ok(None)` when the read succeeded but the value
    /// is not (or is no longer) a string.
    ///
    /// # Arguments
    ///
    /// * `max_utf16_units` - Maximum total UTF-16 code units copied from the
    ///   Accessibility value. Must be at least 2.
    ///
    /// # Errors
    ///
    /// `Err(())` when the Accessibility read itself failed, which in
    /// practice means the element died or otherwise stopped answering AX
    /// requests (e.g. focus moved to a different element or application);
    /// callers should treat that as the end of evidence-gathering rather
    /// than a transient error to retry.
    pub(crate) fn poll_value(
        &self,
        max_utf16_units: usize,
    ) -> Result<Option<PasteTargetValue>, ()> {
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
        let mut observation = cfstring_to_bounded_value(handle.as_cftype().cast(), max_utf16_units);
        if let Some(observation) = observation.as_mut() {
            observation.selection = copy_ax_text_selection(element);
        }
        Ok(observation)
    }
}

/// Sendable representation of the focused macOS insertion target.
#[derive(Debug)]
pub(crate) struct MacOsFocusSnapshot {
    pid: libc::pid_t,
    bundle_identifier: Option<String>,
    ax: Option<AxElementSnapshot>,
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
    /// they leave the Accessibility portion of the snapshot empty, which
    /// makes [`Self::verify_current`] return
    /// [`FocusVerification::AxUnsupported`] and blocks automatic insertion.
    pub(crate) fn capture() -> Result<Self> {
        frontmost_application_window(true).context("could not capture macOS frontmost window")
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
        let current = match frontmost_application_window(false) {
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

    /// Return the process id of the captured frontmost application.
    ///
    /// # Returns
    ///
    /// The pid macOS reported as frontmost at capture time.
    pub(crate) fn pid(&self) -> libc::pid_t {
        self.pid
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

    /// Reacquire the live focused Accessibility element and read its value.
    ///
    /// Safari/WebKit may replace a focused Accessibility object while
    /// updating an editable field. Reacquiring lets paste acknowledgement
    /// continue across that replacement without treating a dead captured
    /// object as proof that insertion failed.
    ///
    /// # Arguments
    ///
    /// * `max_utf16_units` - Maximum total UTF-16 code units copied from the
    ///   Accessibility value.
    ///
    /// # Returns
    ///
    /// The current bounded value and whether its Accessibility object is
    /// identical to the one captured in this snapshot. `None` means the same
    /// application is still frontmost but exposes no readable focused value.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the frontmost application changed or cannot be
    /// read.
    pub(crate) fn poll_current_value(
        &self,
        max_utf16_units: usize,
    ) -> Result<Option<(PasteTargetValue, bool)>, ()> {
        // This path immediately reads AXValue below, so reacquisition only
        // needs element identity. Skipping the capture-time capability probe
        // avoids copying the same potentially large value twice per poll.
        let current = frontmost_application_window(false).map_err(|_| ())?;
        if self.pid != current.pid || self.bundle_identifier != current.bundle_identifier {
            return Err(());
        }
        let Some(current_element) = current.ax.as_ref() else {
            return Ok(None);
        };
        let same_element =
            ax_identity_equal(self.ax.as_ref(), Some(current_element)).unwrap_or(false);
        current_element
            .poll_value(max_utf16_units)
            .map(|value| value.map(|value| (value, same_element)))
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

/// Read the live frontmost application and its focused Accessibility
/// element from `NSWorkspace` and the AX API.
///
/// # Off-main-thread `NSWorkspace` usage
///
/// This is called from several non-main threads: the recording coordinator
/// (`daemon::recording`, at push-to-talk-down, via
/// [`MacOsFocusSnapshot::capture`]), the daemon worker thread (the
/// pre-chord [`MacOsFocusSnapshot::verify_current`] recheck, and — at much
/// higher frequency — [`MacOsFocusSnapshot::poll_current_value`], the
/// `daemon::macos::pasteboard` `AXValue`-reacquire fallback, which can fire
/// on every remaining poll tick, roughly every 40ms, for the rest of one
/// paste's confirmation window once the originally captured Accessibility
/// object stops answering), the IPC handling thread (`daemon::ipc`'s
/// `paste_text`), and the process main thread (`doctor --deep`'s
/// synchronous paste-transaction smoke test, `daemon::macos::diagnostics`).
///
/// The AX/CF calls just below (`AXUIElementCopyAttributeValue`, `CFEqual`,
/// retain/release) have an explicit thread-safety justification for this —
/// see the `SAFETY` comment on `AxElementHandle`'s `Send`/`Sync` impls
/// above. Apple does not extend that same documented guarantee to
/// `NSWorkspace`'s query methods (`sharedWorkspace`,
/// `frontmostApplication`); as far as published Apple documentation goes,
/// calling them off the main thread is an undocumented gray area, not a
/// confirmed-safe pattern.
///
/// This usage is accepted anyway: every call here is a read-only query (no
/// window server state is mutated), the pattern predates this PR (it is not
/// new off-main-thread exposure introduced here), and no crash or corruption
/// has ever been observed traced back to it in practice. If that ever
/// changes — a crash report or hang implicates `NSWorkspace` from one of
/// these threads — the escalation path is to stop calling it directly here
/// and instead dispatch the query to the main thread (e.g. proxy the read
/// through a call scheduled on the main run loop) and block the calling
/// thread on the result, rather than continuing to call it inline off the
/// main thread.
fn frontmost_application_window(probe_value_support: bool) -> Result<MacOsFocusSnapshot> {
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
        let ax = capture_ax_focused_element(pid, probe_value_support);
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
fn capture_ax_focused_element(
    pid: libc::pid_t,
    probe_value_support: bool,
) -> Option<AxElementSnapshot> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    // Held only to release the application AXUIElementRef when this
    // function returns; the focused-element copy below is independently
    // retained in `element`.
    let _app = AxElementHandle(app);
    set_ax_messaging_timeout(app)?;

    let element = copy_ax_element(app, ax_focused_ui_element_attribute())?;
    set_ax_messaging_timeout(element.0)?;
    let supports_value_polling = probe_value_support && ax_value_is_string(element.0);
    Some(AxElementSnapshot {
        element,
        supports_value_polling,
    })
}

fn set_ax_messaging_timeout(element: AXUIElementRef) -> Option<()> {
    let status =
        unsafe { AXUIElementSetMessagingTimeout(element, AX_MESSAGE_TIMEOUT.as_secs_f32()) };
    (status == K_AX_ERROR_SUCCESS).then_some(())
}

fn copy_ax_element(element: AXUIElementRef, attribute: CFStringRef) -> Option<AxElementHandle> {
    let mut value: CFTypeRef = ptr::null();
    let status = unsafe { AXUIElementCopyAttributeValue(element, attribute, &mut value) };
    if status != K_AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    Some(AxElementHandle(value.cast_mut()))
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

fn copy_ax_text_selection(element: AXUIElementRef) -> Option<PasteTargetSelection> {
    let handle = copy_ax_element(element, ax_selected_text_range_attribute())?;
    if unsafe { AXValueGetType(handle.as_cftype()) } != K_AX_VALUE_CF_RANGE_TYPE {
        return None;
    }

    let mut range = CFRange {
        location: 0,
        length: 0,
    };
    let copied = unsafe {
        AXValueGetValue(
            handle.as_cftype(),
            K_AX_VALUE_CF_RANGE_TYPE,
            (&mut range as *mut CFRange).cast(),
        )
    };
    if copied == 0 || range.location < 0 || range.length < 0 {
        return None;
    }
    Some(PasteTargetSelection {
        location: usize::try_from(range.location).ok()?,
        length: usize::try_from(range.length).ok()?,
    })
}

// Intentionally separate from `pasteboard::BoundedNormalizedValue`: this
// bounds raw UTF-16 units at the AX FFI boundary, while that bounds
// whitespace-stripped normalized chars for layout-independent insertion
// matching. Different unit systems for different jobs — do not merge.
fn cfstring_to_bounded_value(
    value: CFStringRef,
    max_utf16_units: usize,
) -> Option<PasteTargetValue> {
    if max_utf16_units < 2 {
        return None;
    }
    let length = unsafe { CFStringGetLength(value) };
    if length < 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    if length <= max_utf16_units {
        return Some(PasteTargetValue {
            head: cfstring_range_to_string(value, 0, length)?,
            tail: None,
            utf16_units: length,
            selection: None,
        });
    }

    let head_units = max_utf16_units / 2;
    let tail_units = max_utf16_units - head_units;
    Some(PasteTargetValue {
        head: cfstring_range_to_string(value, 0, head_units)?,
        tail: Some(cfstring_range_to_string(
            value,
            length - tail_units,
            tail_units,
        )?),
        utf16_units: length,
        selection: None,
    })
}

fn cfstring_range_to_string(value: CFStringRef, location: usize, length: usize) -> Option<String> {
    let mut units = vec![0_u16; length];
    if length != 0 {
        unsafe {
            CFStringGetCharacters(
                value,
                CFRange {
                    location: CFIndex::try_from(location).ok()?,
                    length: CFIndex::try_from(length).ok()?,
                },
                units.as_mut_ptr(),
            );
        }
    }
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messaging_timeout_bounds_each_confirmation_poll() {
        const MAX_AX_CALLS_PER_POLL: u32 = 3;

        assert!(!AX_MESSAGE_TIMEOUT.is_zero());
        assert!(
            AX_MESSAGE_TIMEOUT * MAX_AX_CALLS_PER_POLL
                < crate::daemon::macos::pasteboard::AX_CONFIRM_DEADLINE
        );
    }

    #[test]
    fn cfstring_conversion_copies_only_bounded_ends() {
        let input = format!("{}{}", "a".repeat(80), "z".repeat(80));
        let value = unsafe {
            CFStringCreateWithBytes(
                ptr::null(),
                input.as_ptr(),
                input.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                0,
            )
        };
        assert!(!value.is_null());
        let value = AxElementHandle(value.cast_mut());

        let bounded =
            cfstring_to_bounded_value(value.as_cftype().cast(), 64).expect("valid CFString");

        assert_eq!(bounded.head, "a".repeat(32));
        assert_eq!(bounded.tail.as_deref(), Some("z".repeat(32).as_str()));
        assert_eq!(bounded.utf16_units, 160);
        assert_eq!(bounded.selection, None);
    }

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
    fn decision_does_not_authorize_insertion_without_ax_identity() {
        let verification = decide_focus_verification(true, true, None);

        assert_eq!(verification, FocusVerification::AxUnsupported);
        assert!(!verification.allows_insertion());
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
