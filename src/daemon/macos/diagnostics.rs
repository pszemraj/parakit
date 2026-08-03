//! Real end-to-end paste-transaction smoke test for `parakit doctor --deep`.
//!
//! [`crate::daemon::macos::suppressed_paste_shortcut_smoke`] (stage 1 of the
//! macOS `doctor --deep` insertion check) only proves that parakit can post a
//! synthetic Cmd+V chord and that the CoreGraphics event tap observes it —
//! the tap *suppresses* the chord, so it never reaches a real application.
//! That leaves the entire production paste transaction unverified: the
//! clipboard swap, the focus-verification `before_chord` recheck (commit 3),
//! the full Cmd-down/V-down/V-up/Cmd-up chord delivery (commit 4), and the
//! `AXValue` post-paste acknowledgement polling in
//! [`crate::daemon::macos::pasteboard`] (commit 5).
//!
//! This module (stage 2, wired in
//! [`crate::daemon::desktop::inject::platform_paste_smoke_test`]) closes that
//! gap: it opens a small throwaway `NSWindow`/`NSTextView`, captures a focus
//! snapshot of that window exactly the way the daemon captures one for a
//! real target, then runs [`Injector::paste_text_guarded`] — the same
//! production call the worker thread makes — to paste a unique sentinel into
//! it. It then asserts the acknowledgement tier, reads the text view back to
//! confirm the sentinel actually landed, and confirms the clipboard was
//! restored.
//!
//! Only [`PasteMode::Standard`] and [`PasteMode::Terminal`] reach this stage
//! (see `platform_paste_smoke_test`): direct-mode typing never touches the
//! clipboard, paste chord, or `AXValue` acknowledgement machinery this test
//! exists to exercise, and stage 1's suppressed key-event tap already proves
//! synthetic keystroke delivery for that mode.
//!
//! ## Why this is safe to run on the main thread
//!
//! `NSApplication`/`NSWindow`/`NSTextView` are hard main-thread-only AppKit
//! APIs. `doctor --deep` is the one place in parakit allowed to call them,
//! because `parakit doctor` runs the entire diagnostic path synchronously on
//! the process main thread before any daemon thread is spawned (see
//! `app::run`'s `Commands::Doctor` arm). [`real_paste_transaction_smoke_test`]
//! double-checks that assumption at runtime via [`MainThreadMarker::new`]
//! rather than trusting a comment, so a future refactor that starts calling
//! this from a background thread fails with a clear error instead of
//! crashing or corrupting AppKit state.
//!
//! ## Threading design: worker thread pastes, main thread pumps events
//!
//! [`crate::daemon::macos::pasteboard::await_paste_confirmation`] polls
//! `AXValue` with a plain blocking sleep loop on the calling thread. In
//! production that thread is always the daemon's dedicated worker thread
//! (see `daemon::worker::worker_loop`), never the main thread — the main
//! thread there is busy running the hotkey event tap's run loop.
//! `doctor --deep`, by contrast, runs everything on the main thread. If this
//! function called `Injector::paste_text_guarded` directly from the main
//! thread, the blocking `AXValue` poll loop would starve AppKit's own event
//! dispatch: the synthetic Cmd+V chord this same call just posted to the HID
//! tap would never be delivered to the probe `NSTextView`, so the paste
//! could never be observed landing and the poll would always end in
//! [`crate::daemon::desktop::clipboard_restore::PasteConfirmation::NoEvidence`].
//!
//! The fix mirrors production exactly rather than inventing new threading:
//! the paste (clipboard stage, chord send, `AXValue` poll) runs on a
//! short-lived worker thread — exactly where it runs in the real daemon —
//! while the main thread pumps AppKit events in bounded slices, checking a
//! channel for the worker's result each tick.
//!
//! The pump must be a real AppKit event pump — `nextEventMatchingMask:` +
//! `sendEvent:` (see [`pump_app_events`]) — not a bare `CFRunLoopRunInMode`
//! loop. An `NSApplication` that never enters `[NSApp run]` dequeues nothing
//! from the window-server event queue on its own: merely running the
//! CFRunLoop leaves the app-activation handshake unprocessed (so the probe
//! window never becomes key, even in a perfectly healthy GUI session) and
//! would likewise never deliver the synthetic paste chord to the text view.
//!
//! Moving [`FocusSnapshot`] into the worker thread's closure requires it to
//! be `Send`. It already is: `WorkerEvent::Stopped` (see
//! `daemon::worker::WorkerEvent`) carries a `Box<FocusSnapshot>` across the
//! hotkey-to-worker-thread channel in production today, which only compiles
//! because every field of `FocusSnapshot` (transitively including
//! `AxElementHandle`, see its `unsafe impl Send`/`Sync` in
//! `daemon::macos::focus`) is already `Send`. This module relies on that
//! same, already-relied-upon property rather than adding anything new.

use anyhow::{bail, Context, Result};
use arboard::{Clipboard, ImageData};
use objc2::rc::{autoreleasepool, Retained};
use objc2::{sel, MainThreadMarker, MainThreadOnly as _};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEventMask, NSMenu,
    NSMenuItem, NSTextView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::daemon::desktop::inject::{
    owned_image, restore_html_clipboard, ClipboardPolicy, FocusSnapshot, Injector, PasteMode,
    PasteOutcome, PasteReport,
};
use crate::daemon::macos::pasteboard;

type OSStatus = i32;
type ProcessApplicationTransformState = u32;

/// `kProcessTransformToForegroundApplication` from the (still-functional,
/// if long-deprecated) Carbon Process Manager.
const K_PROCESS_TRANSFORM_TO_FOREGROUND_APPLICATION: ProcessApplicationTransformState = 1;

#[repr(C)]
struct ProcessSerialNumber {
    high_long_of_psn: u32,
    low_long_of_psn: u32,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn GetCurrentProcess(psn: *mut ProcessSerialNumber) -> OSStatus;
    fn TransformProcessType(
        psn: *const ProcessSerialNumber,
        transform_state: ProcessApplicationTransformState,
    ) -> OSStatus;
}

/// Register this process as a foreground-capable application.
///
/// parakit runs as a plain, unbundled command-line binary rather than an
/// `.app` bundle. Unbundled processes are not registered with the Dock/
/// window-server activation machinery by default, which is a common reason
/// `NSApplication` `-activateIgnoringOtherApps:` silently no-ops for them:
/// the probe window can be shown but never becomes the key window, because
/// the *process* never becomes the active application.
/// `TransformProcessType` is the long-standing (Carbon Process Manager,
/// still functional) fix for exactly this: it tells the window server this
/// process may become a normal foreground application. This has no effect
/// on windowed GUI apps; it only matters for bare CLI binaries like
/// `doctor --deep`, run from an interactive terminal session.
///
/// Note this is necessary but not always sufficient: some launch contexts
/// (for example, a process spawned by an automation harness rather than a
/// user directly typing into an interactive terminal) can still have window
/// activation denied by the window server even after this call succeeds.
/// That is a genuine environment constraint outside parakit's control, not
/// a bug in this harness — [`ProbeWindow::make_key_and_focus`] surfaces it
/// as a plain, actionable timeout rather than hanging or crashing.
///
/// Best-effort: failure of the registration call itself is not fatal here
/// either; if it fails, the probe window may simply never become key, which
/// again resolves to that same actionable timeout.
fn register_as_foreground_application() {
    let mut psn = ProcessSerialNumber {
        high_long_of_psn: 0,
        low_long_of_psn: 0,
    };
    unsafe {
        if GetCurrentProcess(&mut psn) == 0 {
            let _ = TransformProcessType(&psn, K_PROCESS_TRANSFORM_TO_FOREGROUND_APPLICATION);
        }
    }
}

/// How long to wait for the probe window to become key before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(3);
/// Event pump slice used while waiting for the probe window to become key.
const READY_POLL: Duration = Duration::from_millis(20);
/// How often to re-request app activation while waiting for the probe
/// window to become key. Modern macOS treats activation as a cooperative,
/// asynchronous request the window server may not honor on the first ask
/// for a freshly registered CLI process; periodic re-requests while pumping
/// events make becoming key reliable instead of a first-try coin flip.
const ACTIVATION_NUDGE_INTERVAL: Duration = Duration::from_millis(250);
/// Best-effort settle pump after first-responder assignment, giving the
/// Accessibility subsystem a moment to register the new focused element
/// before it is snapshotted. Capture failure after this still degrades
/// gracefully (see [`check_paste_report`]); this just improves the odds of
/// exercising the `ax_confirmed` path instead of skipping it.
const AX_SETTLE_PUMP: Duration = Duration::from_millis(150);
/// Event pump slice used while waiting for the paste worker thread.
const EVENT_PUMP_SLICE: Duration = Duration::from_millis(20);
/// Upper bound on the whole paste transaction (clipboard settle + chord +
/// `AXValue` confirmation deadline/unverified grace, see
/// `daemon::macos::pasteboard`) plus a generous margin. Exceeding this means
/// something is stuck, not merely slow.
const PASTE_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(8);

const PROBE_WINDOW_TITLE: &str = "parakit paste smoke";
const PROBE_ORIGIN: (f64, f64) = (80.0, 80.0);
const PROBE_SIZE: (f64, f64) = (360.0, 90.0);

/// Clipboard payload kinds the production swap path can restore.
///
/// The doctor harness captures the same supported formats independently so
/// it can verify restoration rather than trusting `PasteReport` telemetry.
enum DoctorClipboardSnapshot {
    Text(String),
    Html {
        html: String,
        alt_text: Option<String>,
    },
    FileList(Vec<PathBuf>),
    Image(ImageData<'static>),
    EmptyOrUnsupported,
}

impl DoctorClipboardSnapshot {
    fn capture(clipboard: &mut Clipboard) -> Self {
        if let Ok(files) = clipboard.get().file_list() {
            return Self::FileList(files);
        }

        if let Ok(html) = clipboard.get().html() {
            let alt_text = Clipboard::get_text(clipboard).ok();
            if alt_text.as_deref().is_none_or(str::is_empty) {
                if let Ok(image) = Clipboard::get_image(clipboard) {
                    return Self::Image(owned_image(image));
                }
            }
            return Self::Html { html, alt_text };
        }

        if let Ok(image) = Clipboard::get_image(clipboard) {
            return Self::Image(owned_image(image));
        }

        match Clipboard::get_text(clipboard).ok() {
            Some(text) => Self::Text(text),
            None => Self::EmptyOrUnsupported,
        }
    }

    fn restore(&self, clipboard: &mut Clipboard) -> Result<()> {
        match self {
            Self::Text(text) => Clipboard::set_text(clipboard, text.clone())
                .context("could not restore the doctor smoke test's previous clipboard text"),
            Self::Html { html, alt_text } => {
                restore_html_clipboard(clipboard, html.clone(), alt_text.clone())
                    .context("could not restore the doctor smoke test's previous HTML clipboard")
            }
            Self::FileList(files) => clipboard
                .set()
                .file_list(files)
                .context("could not restore the doctor smoke test's previous file-list clipboard"),
            Self::Image(image) => Clipboard::set_image(
                clipboard,
                ImageData {
                    width: image.width,
                    height: image.height,
                    bytes: Cow::Owned(image.bytes.to_vec()),
                },
            )
            .context("could not restore the doctor smoke test's previous image clipboard"),
            Self::EmptyOrUnsupported => Clipboard::clear(clipboard)
                .context("could not clear the clipboard after the doctor smoke test"),
        }
    }

    fn matches_current(&self, clipboard: &mut Clipboard) -> bool {
        self.same_payload(&Self::capture(clipboard))
    }

    fn same_payload(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Text(expected), Self::Text(actual)) => expected == actual,
            (
                Self::Html {
                    html: expected_html,
                    alt_text: expected_alt,
                },
                Self::Html {
                    html: actual_html,
                    alt_text: actual_alt,
                },
            ) => expected_html == actual_html && expected_alt == actual_alt,
            (Self::FileList(expected), Self::FileList(actual)) => expected == actual,
            (Self::Image(expected), Self::Image(actual)) => {
                expected.width == actual.width
                    && expected.height == actual.height
                    && expected.bytes == actual.bytes
            }
            (Self::EmptyOrUnsupported, Self::EmptyOrUnsupported) => true,
            _ => false,
        }
    }

    fn is_text(&self, expected: &str) -> bool {
        matches!(self, Self::Text(actual) if actual == expected)
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Html { .. } => "HTML",
            Self::FileList(_) => "file list",
            Self::Image(_) => "image",
            Self::EmptyOrUnsupported => "empty or unsupported",
        }
    }
}

/// Main-thread failsafe for the clipboard mutation performed by the worker.
///
/// Normal completion verifies every supported payload kind. On an earlier
/// return or panic in the worker, `Drop` restores the snapshot when the
/// staged sentinel is still active, preventing a diagnostic failure from
/// stranding it on the user's clipboard.
struct DoctorClipboardGuard {
    clipboard: Clipboard,
    previous: DoctorClipboardSnapshot,
    sentinel: Option<String>,
    finished: bool,
}

impl DoctorClipboardGuard {
    fn capture() -> Result<Self> {
        let mut clipboard = Clipboard::new()
            .context("could not open the system clipboard for the doctor paste smoke test")?;
        let previous = DoctorClipboardSnapshot::capture(&mut clipboard);
        Ok(Self {
            clipboard,
            previous,
            sentinel: None,
            finished: false,
        })
    }

    fn arm(&mut self, sentinel: &str) {
        self.sentinel = Some(sentinel.to_owned());
    }

    /// Verify the production restore and repair it before returning an error.
    fn verify_and_repair(&mut self) -> Result<()> {
        let current = DoctorClipboardSnapshot::capture(&mut self.clipboard);
        if self.previous.same_payload(&current) {
            self.finished = true;
            return Ok(());
        }

        let expected_kind = self.previous.kind();
        let sentinel_is_staged = self
            .sentinel
            .as_deref()
            .is_some_and(|sentinel| current.is_text(sentinel));
        if !sentinel_is_staged {
            // A clipboard payload other than our sentinel belongs to the
            // user or another application. Report that restoration could
            // not be verified, but never overwrite the newer value.
            self.finished = true;
            bail!(
                "macOS doctor paste-transaction did not restore the previous {expected_kind} \
                 clipboard payload; the clipboard now contains a different {} payload, which \
                 the diagnostic left untouched",
                current.kind()
            );
        }

        self.previous
            .restore(&mut self.clipboard)
            .with_context(|| {
                format!(
                    "macOS doctor paste-transaction did not restore the previous \
                     {expected_kind} clipboard payload, and diagnostic cleanup also failed"
                )
            })?;
        let repaired = self.previous.matches_current(&mut self.clipboard);
        self.finished = repaired;
        if repaired {
            bail!(
                "macOS doctor paste-transaction did not restore the previous {expected_kind} \
                 clipboard payload; the diagnostic repaired it before returning"
            );
        }
        bail!(
            "macOS doctor paste-transaction did not restore the previous {expected_kind} \
             clipboard payload, and the cleanup write could not be verified"
        )
    }

    fn restore_staged_sentinel(&mut self) -> Result<()> {
        let Some(sentinel) = self.sentinel.as_deref() else {
            return Ok(());
        };
        let current = DoctorClipboardSnapshot::capture(&mut self.clipboard);
        if current.is_text(sentinel) {
            self.previous.restore(&mut self.clipboard)?;
            self.finished = self.previous.matches_current(&mut self.clipboard);
            if !self.finished {
                bail!(
                    "restored the pre-diagnostic clipboard after a failed paste smoke test, but \
                     the {} payload could not be verified",
                    self.previous.kind()
                );
            }
        }
        Ok(())
    }
}

impl Drop for DoctorClipboardGuard {
    fn drop(&mut self) {
        if !self.finished {
            // A failed restore can be transient (for example, clipboard
            // ownership changing mid-write), so make one best-effort retry.
            // `restore_staged_sentinel` writes only while our sentinel is
            // still current and therefore never overwrites newer user data.
            let _ = self.restore_staged_sentinel();
        }
    }
}

/// Run the real end-to-end paste-transaction smoke test: paste a sentinel
/// through the production guarded-paste transaction into a throwaway
/// `NSTextView` and verify it actually landed, was acknowledged, and the
/// clipboard was restored.
///
/// # Arguments
///
/// * `mode` - Paste shortcut mode to exercise (`Standard` or `Terminal`;
///   callers must not invoke this for `Direct`, see the module docs).
///
/// # Returns
///
/// `Ok(())` when the sentinel was pasted, `ax_confirmed` (or the documented
/// AX-self-inspection-failure carve-out), read back from the probe text
/// view, and the previous clipboard contents were restored.
///
/// # Errors
///
/// Returns an actionable error identifying what failed: the probe window
/// could not be created or focused, the production paste transaction
/// errored, the acknowledgement tier did not match what focus capture
/// implied was possible, the sentinel did not land in the probe text view,
/// or the previous clipboard contents were not restored.
pub(crate) fn real_paste_transaction_smoke_test(mode: PasteMode) -> Result<()> {
    // `doctor --deep` runs this synchronously on the process main thread
    // before any daemon thread exists (see `app::run`'s `Commands::Doctor`
    // arm); that is the only reason touching AppKit here is sound. Verify it
    // at runtime rather than trusting that ordering never changes.
    let mtm = MainThreadMarker::new().context(
        "the macOS real paste-transaction smoke test must run on the main thread, but the \
         current thread is not the main thread",
    )?;

    // parakit is a plain unbundled binary, not an `.app`; register it as
    // foreground-capable before touching NSApplication at all, otherwise
    // activation below silently no-ops (see `register_as_foreground_application`).
    register_as_foreground_application();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    install_paste_menu(&app, mtm);
    app.finishLaunching();
    request_activation(&app);

    let probe = ProbeWindow::create(mtm)
        .context("could not create the macOS doctor paste-transaction probe window")?;
    probe.make_key_and_focus(&app)?;
    probe.run_paste_transaction(&app, mode)
}

/// Install a minimal main menu holding Edit > Paste (Cmd+V).
///
/// AppKit routes command-key chords through menu key equivalents:
/// `-[NSApplication sendEvent:]` offers a command-modified key-down to the
/// key window and then to the main menu's `performKeyEquivalent:`.
/// `NSTextView` has no Cmd+V handling of its own — in a real application
/// the Edit menu's Paste item (action `paste:`, key equivalent "v") is what
/// turns the chord into a `paste:` message down the responder chain. A bare
/// unbundled CLI process has no main menu, so without this the production
/// paste chord is delivered to the activated probe app and then dropped,
/// and the probe text view never consumes the clipboard. The menu is never
/// drawn (accessory activation policy); only its key-equivalent table
/// matters.
fn install_paste_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let empty = NSString::from_str("");
    let menubar = NSMenu::initWithTitle(NSMenu::alloc(mtm), &empty);
    // SAFETY: a nil action with an empty key equivalent is the inert
    // container form of NSMenuItem; nothing is ever dispatched through it.
    let edit_holder = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Edit"),
            None,
            &empty,
        )
    };
    let edit_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
    // SAFETY: `paste:` is the standard NSResponder editing action; with a
    // nil target, menu dispatch walks the key window's responder chain and
    // reaches the probe NSTextView, which implements it. The "v" key
    // equivalent carries the default Command modifier mask.
    let paste_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Paste"),
            Some(sel!(paste:)),
            &NSString::from_str("v"),
        )
    };
    edit_menu.addItem(&paste_item);
    edit_holder.setSubmenu(Some(&edit_menu));
    menubar.addItem(&edit_holder);
    app.setMainMenu(Some(&menubar));
}

/// Request app activation with the forceful, deprecated
/// `-activateIgnoringOtherApps:`.
///
/// The modern `-activate` is cooperative: on a busy desktop with several
/// other visible apps it can be silently ignored unless a peer app yields
/// activation to us first, which never happens for a synchronous CLI
/// diagnostic with no cooperating peer. `doctor --deep` needs its probe
/// window to reliably become key regardless of whatever else has focus, so
/// it deliberately uses the forceful variant, re-requested periodically
/// while the ready wait pumps events (see [`ProbeWindow::make_key_and_focus`]).
fn request_activation(app: &NSApplication) {
    #[allow(
        deprecated,
        reason = "`-activate` is cooperative and can be silently ignored on a busy desktop; \
                   this diagnostic has no cooperating peer app to yield activation, so it needs \
                   the forceful `-activateIgnoringOtherApps:` to reliably become the key window"
    )]
    app.activateIgnoringOtherApps(true);
}

/// Throwaway `NSWindow` + `NSTextView` used only for the duration of
/// [`real_paste_transaction_smoke_test`].
struct ProbeWindow {
    window: Retained<NSWindow>,
    text_view: Retained<NSTextView>,
}

impl ProbeWindow {
    fn create(mtm: MainThreadMarker) -> Result<Self> {
        let frame = NSRect::new(
            NSPoint::new(PROBE_ORIGIN.0, PROBE_ORIGIN.1),
            NSSize::new(PROBE_SIZE.0, PROBE_SIZE.1),
        );

        // SAFETY: `objc2-app-kit` requires `setReleasedWhenClosed(false)`
        // immediately after creating an `NSWindow` outside a window
        // controller (see the crate's `## NSWindow` module docs), so that
        // `close()` below never performs an extra implicit release beyond
        // the one retain this `Retained<NSWindow>` already accounts for.
        // That call happens unconditionally right after `init`, before this
        // window is ever shown or closed.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(PROBE_WINDOW_TITLE));

        let content_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(PROBE_SIZE.0, PROBE_SIZE.1),
        );
        let text_view = NSTextView::initWithFrame(NSTextView::alloc(mtm), content_frame);
        text_view.setEditable(true);
        text_view.setSelectable(true);

        window.setContentView(Some(&text_view));

        Ok(Self { window, text_view })
    }

    /// Order the probe window front, wait for it to become key (pumping
    /// AppKit events and periodically re-requesting activation, since the
    /// window server treats activation as an asynchronous request it may
    /// not honor on the first ask), and make its text view the first
    /// responder.
    fn make_key_and_focus(&self, app: &NSApplication) -> Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut next_nudge = Instant::now();
        while !self.window.isKeyWindow() {
            if Instant::now() >= deadline {
                bail!(
                    "the macOS doctor paste-transaction probe window did not become the key \
                     window within {READY_TIMEOUT:?}; the window server did not activate the \
                     probe process"
                );
            }
            if Instant::now() >= next_nudge {
                request_activation(app);
                self.window.makeKeyAndOrderFront(None);
                next_nudge = Instant::now() + ACTIVATION_NUDGE_INTERVAL;
            }
            pump_app_events(app, READY_POLL);
        }
        if !self.window.makeFirstResponder(Some(&self.text_view)) {
            bail!(
                "the macOS doctor paste-transaction probe window refused to make its text view \
                 the first responder"
            );
        }
        pump_app_events(app, AX_SETTLE_PUMP);
        Ok(())
    }

    /// Capture focus on the probe window, run the production guarded-paste
    /// transaction with a unique sentinel, and assert it landed, was
    /// acknowledged, and the clipboard was restored.
    fn run_paste_transaction(&self, app: &NSApplication, mode: PasteMode) -> Result<()> {
        let focus = FocusSnapshot::capture().context(
            "could not capture a macOS focus snapshot of the doctor paste-transaction probe \
             window itself",
        )?;

        // The snapshot captures whatever application the window server says
        // is frontmost. If that is not this process, the probe window
        // becoming key was a lie at the system level, and posting the paste
        // chord would deliver a Cmd+V (and the sentinel on the clipboard) to
        // some unrelated application the user is actually using. Refuse
        // loudly instead.
        let probe_pid = std::process::id() as libc::pid_t;
        let captured_pid = focus.macos_pid();
        if captured_pid != probe_pid {
            bail!(
                "the frontmost application at snapshot time was pid {captured_pid} \
                 (bundle {:?}), not the probe process (pid {probe_pid}); refusing to post the \
                 paste chord because it would be delivered to that application instead of the \
                 probe window",
                focus.target_bundle_id(),
            );
        }
        let ax_focused_element_available = focus.macos_ax_element().is_some();

        let mut clipboard_guard = DoctorClipboardGuard::capture()?;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let sentinel = build_sentinel(std::process::id(), nonce);
        clipboard_guard.arm(&sentinel);

        let (tx, rx) = mpsc::channel::<Result<PasteReport>>();
        let worker_focus = focus;
        let worker_sentinel = sentinel.clone();
        // Shared with the worker thread below so the timeout path can tell
        // it that this harness has taken over clipboard cleanup; see
        // `pasteboard::install_abandonment_signal` for the full mechanism
        // and why it exists.
        let abandoned = Arc::new(AtomicBool::new(false));
        let worker_abandoned = Arc::clone(&abandoned);
        let worker = thread::Builder::new()
            .name("parakit-doctor-paste-smoke".to_string())
            .spawn(move || {
                pasteboard::install_abandonment_signal(worker_abandoned);
                let outcome =
                    paste_sentinel_on_worker_thread(mode, &worker_sentinel, &worker_focus);
                let _ = tx.send(outcome);
            })
            .context("could not spawn the macOS doctor paste-transaction worker thread")?;

        let report = match pump_until_paste_result(app, &rx, PASTE_TRANSACTION_TIMEOUT) {
            Ok(report) => {
                // The worker already sent its result, so this returns
                // essentially immediately; join it for cleanliness.
                let _ = worker.join();
                report
            }
            Err(err) => {
                // The worker's own bounded waits (AX confirmation deadline,
                // unverified grace, clipboard settle) are all well inside
                // `PASTE_TRANSACTION_TIMEOUT`. Blowing through that timeout
                // means something is genuinely stuck, so let the thread
                // finish on its own rather than risking `doctor` hanging in
                // `join()` too.
                //
                // That leaves the worker thread free to keep running after
                // this function returns, potentially still inside the
                // `AXValue` confirmation poll, and it must not be left free
                // to also mutate the real clipboard once it eventually
                // unblocks: `clipboard_guard.restore_staged_sentinel()` below
                // and the process exit that (per `app::run`) follows a
                // `doctor --deep` failure both assume unilateral ownership
                // of it. Flip the abandonment signal before that cleanup
                // runs, not after, so the worker sees it as soon as possible
                // once it resumes from whatever it is blocked on.
                abandoned.store(true, Ordering::Release);
                return match clipboard_guard.restore_staged_sentinel() {
                    Ok(()) => Err(err),
                    Err(cleanup_err) => Err(err.context(format!(
                        "failed to restore the pre-diagnostic clipboard after the paste worker \
                         failed: {cleanup_err:#}"
                    ))),
                };
            }
        };

        // Read the probe text view back before judging the report: whether
        // the sentinel physically landed is the ground truth that tells a
        // delivery failure apart from an acknowledgement failure.
        let landed_text = self.text_view.string().to_string();
        let sentinel_landed = landed_text.contains(&sentinel);
        let clipboard_restore_result = clipboard_guard.verify_and_repair();

        if let Err(message) = check_paste_report(&report, ax_focused_element_available) {
            let clipboard_status = match &clipboard_restore_result {
                Ok(()) => "previous clipboard payload verified".to_owned(),
                Err(err) => format!("clipboard restore check: {err:#}"),
            };
            bail!(
                "{message} (sentinel landed in probe text view: {sentinel_landed}, \
                 paste_event_posted={}, acknowledgement_ms={:?}, clipboard_restored={:?}, \
                 {clipboard_status})",
                report.telemetry.paste_event_posted,
                report.telemetry.acknowledgement_ms,
                report.telemetry.clipboard_restored,
            );
        }

        if !sentinel_landed {
            bail!(
                "macOS doctor paste-transaction reported {:?}/{} but the probe text view does \
                 not contain the sentinel; the acknowledgement signal and the actual inserted \
                 text disagree (text view holds {landed_text:?})",
                report.outcome,
                report.telemetry.acknowledgement_kind,
            );
        }

        clipboard_restore_result
    }
}

impl Drop for ProbeWindow {
    /// Take the probe window off screen on every exit path.
    ///
    /// `make_key_and_focus` orders the window front before it can fail, so an
    /// early return from there would otherwise strand a visible window until
    /// the process exits. `Retained<NSWindow>` is main-thread-only, so this
    /// can only run on the thread that built it.
    fn drop(&mut self) {
        self.window.close();
    }
}

/// Build a unique sentinel string for one paste-transaction smoke run.
///
/// Factored out as a pure function (no FFI) so it stays unit-testable
/// without a GUI session or Accessibility permission.
fn build_sentinel(pid: u32, nonce: u128) -> String {
    format!("parakit-doctor-macos-smoke-{pid}-{nonce}")
}

/// Build and run the production guarded-paste transaction on the calling
/// (worker) thread, exactly as `daemon::worker::worker_loop` does.
fn paste_sentinel_on_worker_thread(
    mode: PasteMode,
    sentinel: &str,
    focus: &FocusSnapshot,
) -> Result<PasteReport> {
    let mut injector = Injector::new().context(
        "could not open the macOS insertion backend for the doctor paste-transaction smoke test",
    )?;
    injector.prepare_for_mode(mode).context(
        "could not prepare the macOS insertion backend for the doctor paste-transaction smoke \
         test",
    )?;
    injector.paste_text_guarded(
        sentinel,
        mode,
        ClipboardPolicy::RestorePrevious,
        Some(focus),
        || Ok(true),
    )
}

/// Validate the acknowledgement tier a real paste transaction reached.
///
/// Factored out as a pure function (no FFI) so it stays unit-testable
/// without a GUI session or Accessibility permission; see the `tests` module
/// below.
///
/// # Arguments
///
/// * `report` - Outcome of the production guarded-paste call.
/// * `ax_focused_element_available` - Whether Accessibility exposed a
///   focused element on the probe text view at capture time. When `false`,
///   [`PasteOutcome::PastedUnverified`] is accepted as the documented
///   AX-self-inspection-failure carve-out (see `daemon::macos::pasteboard`'s
///   module docs); otherwise `ax_confirmed` is required.
///
/// # Errors
///
/// Returns a diagnostic message describing what failed and what it implies
/// when the report does not meet the expected acknowledgement tier.
fn check_paste_report(
    report: &PasteReport,
    ax_focused_element_available: bool,
) -> Result<(), String> {
    match (report.outcome, report.telemetry.acknowledgement_kind) {
        (PasteOutcome::Pasted, "ax_confirmed") => Ok(()),
        (PasteOutcome::PastedUnverified, _) if !ax_focused_element_available => Ok(()),
        (PasteOutcome::Pasted, kind) => Err(format!(
            "paste landed (outcome=Pasted) but the acknowledgement kind was {kind:?}, not \
             \"ax_confirmed\", even though Accessibility exposed a focused element on the probe \
             text view; the commit-5 AXValue confirmation path may not be firing"
        )),
        (PasteOutcome::PastedUnverified, kind) => Err(format!(
            "paste was only Unverified (kind={kind:?}) even though Accessibility exposed a \
             focused element on the probe text view; expected ax_confirmed — the AXValue \
             polling loop may not be observing the change"
        )),
        (PasteOutcome::CopiedOnly, kind) => Err(format!(
            "the paste chord never landed in the probe text view: the transcript was left on \
             the clipboard instead (outcome=CopiedOnly, kind={kind:?}); the synthetic Cmd+V may \
             not have reached the probe window, or the text view never consumed the clipboard"
        )),
        (PasteOutcome::UnsafeModifiers, kind) => Err(format!(
            "the paste chord was withheld because physical modifiers remained active \
             (outcome=UnsafeModifiers, kind={kind:?}); release the push-to-talk keys before \
             rerunning the deep diagnostic"
        )),
        (PasteOutcome::Blocked, kind) => Err(format!(
            "the doctor smoke harness's own safety-recheck closure blocked insertion \
             (outcome=Blocked, kind={kind:?}); this indicates a bug in the harness itself, not \
             the production paste path"
        )),
    }
}

/// Poll `rx` for the paste worker thread's result while pumping the main
/// thread's AppKit events, so the synthetic paste chord can be delivered to
/// the probe text view while the worker's `AXValue` poll loop waits for
/// evidence.
fn pump_until_paste_result(
    app: &NSApplication,
    rx: &mpsc::Receiver<Result<PasteReport>>,
    timeout: Duration,
) -> Result<PasteReport> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(result) => return result,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                bail!(
                    "the macOS doctor paste-transaction worker thread ended without sending a \
                     result; it may have panicked while pasting the sentinel"
                );
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out after {timeout:?} waiting for the macOS doctor paste-transaction \
                 worker thread; the paste chord was posted but no confirmation result arrived \
                 (the probe window or Accessibility polling may be stuck)"
            );
        }
        pump_app_events(app, EVENT_PUMP_SLICE);
    }
}

/// Dequeue and dispatch pending AppKit events for up to `slice`.
///
/// This is the manual equivalent of one bounded turn of `[NSApp run]`: an
/// `NSApplication` that never enters its own run loop must explicitly pull
/// events from the window-server queue with `nextEventMatchingMask:` and
/// hand them to `sendEvent:`, or nothing is ever delivered — no activation
/// handshake (the probe window can never become key) and no synthetic
/// keystrokes to the probe text view. Merely running the CFRunLoop does not
/// do this. `expiration` is an absolute date, so the wait for further
/// events naturally ends at the deadline; events already queued are
/// dispatched immediately regardless.
fn pump_app_events(app: &NSApplication, slice: Duration) {
    autoreleasepool(|_| {
        let expiration = NSDate::dateWithTimeIntervalSinceNow(slice.as_secs_f64());
        // SAFETY: `NSDefaultRunLoopMode` is an extern static; reading it has no
        // side effects and it is always a valid, immortal NSString constant.
        let mode = unsafe { NSDefaultRunLoopMode };
        while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&expiration),
            mode,
            true,
        ) {
            app.sendEvent(&event);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinel_is_unique_per_pid_and_nonce() {
        let a = build_sentinel(123, 1);
        let b = build_sentinel(123, 2);
        let c = build_sentinel(124, 1);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("parakit-doctor-macos-smoke-123-"));
    }

    #[test]
    fn failure_cleanup_only_replaces_the_staged_sentinel() {
        let sentinel = "parakit-doctor-macos-smoke-123-1";
        assert!(DoctorClipboardSnapshot::Text(sentinel.to_owned()).is_text(sentinel));
        assert!(
            !DoctorClipboardSnapshot::Text("a newer user clipboard value".to_owned())
                .is_text(sentinel)
        );
        assert!(!DoctorClipboardSnapshot::Html {
            html: "<b>newer payload</b>".to_owned(),
            alt_text: Some(sentinel.to_owned()),
        }
        .is_text(sentinel));
    }

    #[test]
    fn non_text_clipboard_snapshots_compare_payload_contents() {
        let html = DoctorClipboardSnapshot::Html {
            html: "<b>hello</b>".to_owned(),
            alt_text: Some("hello".to_owned()),
        };
        let changed_html = DoctorClipboardSnapshot::Html {
            html: "<b>hello</b>".to_owned(),
            alt_text: Some("different".to_owned()),
        };
        assert!(html.same_payload(&DoctorClipboardSnapshot::Html {
            html: "<b>hello</b>".to_owned(),
            alt_text: Some("hello".to_owned()),
        }));
        assert!(!html.same_payload(&changed_html));

        let image = DoctorClipboardSnapshot::Image(ImageData {
            width: 1,
            height: 1,
            bytes: Cow::Owned(vec![0, 1, 2, 3]),
        });
        let changed_image = DoctorClipboardSnapshot::Image(ImageData {
            width: 1,
            height: 1,
            bytes: Cow::Owned(vec![3, 2, 1, 0]),
        });
        assert!(!image.same_payload(&changed_image));

        let files = DoctorClipboardSnapshot::FileList(vec![PathBuf::from("one.txt")]);
        assert!(
            files.same_payload(&DoctorClipboardSnapshot::FileList(vec![PathBuf::from(
                "one.txt"
            )]))
        );
    }

    fn report(outcome: PasteOutcome, acknowledgement_kind: &'static str) -> PasteReport {
        PasteReport {
            outcome,
            telemetry: crate::daemon::desktop::inject::InsertionTelemetry {
                paste_event_posted: true,
                acknowledgement_kind,
                acknowledgement_ms: Some(42),
                clipboard_restored: Some(true),
            },
        }
    }

    /// One (kind, ax_focused) input paired with the expected
    /// `check_paste_report` result, grouped per [`PasteOutcome`] variant by
    /// [`report_rows`].
    struct ReportRow {
        name: &'static str,
        kind: &'static str,
        ax_focused: bool,
        expect_ok: bool,
    }

    /// Rows exercised for `outcome`, keyed through an exhaustive match (no
    /// wildcard arm) so adding a new [`PasteOutcome`] variant fails
    /// compilation here until its `check_paste_report` coverage is stated as
    /// data. [`PasteOutcome`]'s variants are all fieldless (see
    /// `daemon::desktop::inject::PasteOutcome`), so matching on `outcome`
    /// alone is exhaustive without binding any payload.
    fn report_rows(outcome: PasteOutcome) -> &'static [ReportRow] {
        match outcome {
            PasteOutcome::Pasted => &[
                ReportRow {
                    name: "ax_confirmed_paste_passes_ax_available",
                    kind: "ax_confirmed",
                    ax_focused: true,
                    expect_ok: true,
                },
                ReportRow {
                    name: "ax_confirmed_paste_passes_ax_unavailable",
                    kind: "ax_confirmed",
                    ax_focused: false,
                    expect_ok: true,
                },
                ReportRow {
                    name: "pasted_without_ax_confirmed_kind_fails_ax_available",
                    kind: "not_applicable",
                    ax_focused: true,
                    expect_ok: false,
                },
                ReportRow {
                    name: "pasted_without_ax_confirmed_kind_fails_ax_unavailable",
                    kind: "not_applicable",
                    ax_focused: false,
                    expect_ok: false,
                },
            ],
            PasteOutcome::PastedUnverified => &[
                ReportRow {
                    name: "unverified_paste_passes_when_ax_capture_failed",
                    kind: "unverified_timeout",
                    ax_focused: false,
                    expect_ok: true,
                },
                ReportRow {
                    name: "unverified_paste_fails_when_ax_capture_available",
                    kind: "unverified_timeout",
                    ax_focused: true,
                    expect_ok: false,
                },
            ],
            PasteOutcome::CopiedOnly => &[
                ReportRow {
                    name: "copied_only_fails_ax_available",
                    kind: "no_evidence",
                    ax_focused: true,
                    expect_ok: false,
                },
                ReportRow {
                    name: "copied_only_fails_ax_unavailable",
                    kind: "no_evidence",
                    ax_focused: false,
                    expect_ok: false,
                },
            ],
            // New coverage: `PasteOutcome::UnsafeModifiers` previously had
            // zero test coverage despite a dedicated production arm in
            // `check_paste_report` (~diagnostics.rs:815-819). That arm
            // returns `Err` unconditionally — unlike the `PastedUnverified`
            // arm, it carries no `if !ax_focused_element_available` guard —
            // so both rows below pin `expect_ok: false` regardless of
            // `ax_focused`.
            PasteOutcome::UnsafeModifiers => &[
                ReportRow {
                    name: "unsafe_modifiers_fails_ax_available",
                    kind: "not_applicable",
                    ax_focused: true,
                    expect_ok: false,
                },
                ReportRow {
                    name: "unsafe_modifiers_fails_ax_unavailable",
                    kind: "not_applicable",
                    ax_focused: false,
                    expect_ok: false,
                },
            ],
            PasteOutcome::Blocked => &[
                ReportRow {
                    name: "blocked_fails_ax_available",
                    kind: "not_applicable",
                    ax_focused: true,
                    expect_ok: false,
                },
                ReportRow {
                    name: "blocked_fails_ax_unavailable",
                    kind: "not_applicable",
                    ax_focused: false,
                    expect_ok: false,
                },
            ],
        }
    }

    #[test]
    fn check_paste_report_matches_expected_ok_per_outcome_and_kind() {
        let outcomes = [
            PasteOutcome::Pasted,
            PasteOutcome::PastedUnverified,
            PasteOutcome::CopiedOnly,
            PasteOutcome::UnsafeModifiers,
            PasteOutcome::Blocked,
        ];

        let failures: Vec<String> = outcomes
            .iter()
            .flat_map(|&outcome| report_rows(outcome).iter().map(move |row| (outcome, row)))
            .filter_map(|(outcome, row)| {
                let paste_report = report(outcome, row.kind);
                let actual = check_paste_report(&paste_report, row.ax_focused).is_ok();
                (actual != row.expect_ok).then(|| {
                    format!(
                        "{}: outcome={outcome:?} kind={:?} ax_focused={} expected \
                         is_ok()={}, got {actual}",
                        row.name, row.kind, row.ax_focused, row.expect_ok
                    )
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
