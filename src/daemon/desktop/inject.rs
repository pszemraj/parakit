//! Insert text at the cursor position.
//!
//! Batch mode uses the clipboard plus the platform paste shortcut so the final
//! transcript appears as a single insertion. Direct mode uses
//! `enigo::Keyboard::text()`, which:
//!   - Windows: synthesizes Unicode keystrokes via `SendInput` with
//!     `KEYEVENTF_UNICODE`. Works for any character; no keyboard layout
//!     translation required.
//!   - Linux X11: uses `XTestFakeKeyEvent` plus a temporary keymap remap
//!     for non-keyboard characters. Works for ASCII/Latin reliably; some
//!     emoji or rare scripts may not pass through cleanly.
//!   - Linux Wayland: unsupported. Startup preflight rejects Wayland sessions
//!     because XTest cannot insert into focused native Wayland applications.
//!   - macOS: synthesizes via the CGEvent API. Requires Accessibility for the
//!     terminal that launched parakit.

use anyhow::{Context, Result};
use arboard::Clipboard;
use clap::ValueEnum;
use enigo::{Enigo, Keyboard, Settings};
use std::{cell::RefCell, time::Duration};

#[cfg(target_os = "windows")]
use super::clipboard_restore::clipboard_history_debug;
use super::clipboard_restore::{ClipboardRestorePlan, PlatformClipboardRestoreGate};
#[cfg(target_os = "linux")]
use super::FocusVerification;

#[path = "clipboard_guard.rs"]
mod clipboard_guard;

#[path = "clipboard_store.rs"]
mod clipboard_store;
#[cfg(target_os = "linux")]
use clipboard_store::clipboard_content_unavailable;
#[cfg(target_os = "macos")]
pub(crate) use clipboard_store::{owned_image, restore_html_clipboard};
pub(super) use clipboard_store::{restore_or_clear_clipboard, ClipboardSnapshot, ClipboardStore};

#[path = "focus_snapshot.rs"]
mod focus_snapshot;
#[cfg(target_os = "linux")]
use focus_snapshot::linux_current_input_focus;
pub(crate) use focus_snapshot::FocusSnapshot;

#[path = "paste_transaction.rs"]
mod paste_transaction;
use paste_transaction::{paste_with_clipboard_swap_guarded, stage_text_without_paste};

#[cfg(target_os = "linux")]
#[path = "inject_smoke.rs"]
mod inject_smoke;

#[cfg(target_os = "linux")]
#[path = "direct.rs"]
mod direct;
#[cfg(target_os = "linux")]
pub(crate) use direct::DirectTypingFailure;

#[cfg(target_os = "linux")]
#[path = "x11_paste.rs"]
mod x11_paste;
#[cfg(target_os = "linux")]
use super::x11::wait_for_modifier_release;
#[cfg(target_os = "linux")]
use x11_paste::{linux_x11_xtest_preflight, LinuxX11Paste};

/// Error label used when paste succeeded but previous clipboard restore failed.
pub(crate) const CLIPBOARD_RESTORE_ERROR: &str = "could not restore previous clipboard contents";

/// Paste shortcut style for batch transcript insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PasteMode {
    /// Terminal-friendly paste: `Ctrl+Shift+V` on Linux/Windows, `Cmd+V` on macOS.
    Terminal,
    /// GUI-app paste: `Ctrl+V` on Linux/Windows, `Cmd+V` on macOS.
    Standard,
    /// Type text directly without using the clipboard.
    Direct,
}

impl PasteMode {
    /// Return the short label used in verbose startup output.
    ///
    /// # Returns
    ///
    /// A stable lowercase mode label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Standard => "standard",
            Self::Direct => "direct",
        }
    }
}

/// Result of a guarded paste attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasteOutcome {
    /// The paste chord or direct typing path was sent, and insertion was
    /// positively confirmed (or no stronger confirmation signal exists on
    /// this platform, in which case this is the historical unconditional
    /// meaning of "pasted").
    Pasted,
    /// The paste chord was sent, but insertion could not be positively
    /// confirmed within the acknowledgement grace period (e.g. on macOS,
    /// `AXValue` could not be polled at all — see the `daemon::macos::pasteboard`
    /// module docs for why that happens legitimately). Treated as a success
    /// for retry and clipboard-restore purposes, distinct telemetry from
    /// [`Self::Pasted`].
    PastedUnverified,
    /// The transcript was left on the clipboard and no synthetic input was sent.
    CopiedOnly,
    /// The transcript was left on the clipboard because active physical
    /// modifiers made posting the paste chord unsafe.
    UnsafeModifiers,
    /// No paste chord was sent and clipboard policy was applied.
    Blocked,
    /// Clipboard contents changed or could not be verified; no replacement
    /// paste or clipboard fallback is allowed.
    ClipboardChanged,
}

/// Paths that never send a paste chord (staging, guard-blocked, direct
/// typing) use `acknowledgement_kind: "not_applicable"` and
/// `acknowledgement_ms: None` because they have nothing to acknowledge.
/// After a paste chord, [`paste_with_clipboard_swap_guarded`] records the
/// actual confirmation kind and elapsed time from
/// [`crate::daemon::desktop::clipboard_restore::PasteConfirmation`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InsertionTelemetry {
    /// Whether a synthetic paste chord or type event was actually sent.
    pub(crate) paste_event_posted: bool,
    /// How insertion was acknowledged, or `"not_applicable"` when no paste
    /// chord was sent.
    pub(crate) acknowledgement_kind: &'static str,
    /// Milliseconds spent waiting for acknowledgement, or `None` when no
    /// acknowledgement was attempted.
    pub(crate) acknowledgement_ms: Option<u128>,
    /// Whether the previous clipboard contents were restored, when the
    /// restore policy was applied. `None` also records a deliberately skipped
    /// restore when the staged clipboard no longer matched or was unreadable.
    pub(crate) clipboard_restored: Option<bool>,
}

impl InsertionTelemetry {
    /// Build telemetry for a path that does not attempt acknowledgement.
    ///
    /// # Arguments
    ///
    /// * `paste_event_posted` - Whether synthetic input was sent.
    /// * `clipboard_restored` - Known clipboard restore result, when touched.
    ///
    /// # Returns
    ///
    /// Telemetry with acknowledgement fields set to not applicable.
    pub(crate) const fn not_applicable(
        paste_event_posted: bool,
        clipboard_restored: Option<bool>,
    ) -> Self {
        Self {
            paste_event_posted,
            acknowledgement_kind: "not_applicable",
            acknowledgement_ms: None,
            clipboard_restored,
        }
    }

    fn acknowledged(
        acknowledgement_kind: &'static str,
        elapsed: Duration,
        clipboard_restored: Option<bool>,
    ) -> Self {
        Self {
            paste_event_posted: true,
            acknowledgement_kind,
            acknowledgement_ms: Some(elapsed.as_millis()),
            clipboard_restored,
        }
    }
}

/// Outcome of a guarded paste attempt plus its insertion telemetry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PasteReport {
    /// Coarse guarded-paste result.
    pub(crate) outcome: PasteOutcome,
    /// Acknowledgement and clipboard details shared with the worker report.
    pub(crate) telemetry: InsertionTelemetry,
    /// Clipboard observation error retained for diagnostics on a safe,
    /// non-destructive outcome.
    pub(crate) diagnostic: Option<String>,
}

impl PasteReport {
    /// Build a report for a path that does not attempt acknowledgement.
    fn new(
        outcome: PasteOutcome,
        paste_event_posted: bool,
        clipboard_restored: Option<bool>,
    ) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(paste_event_posted, clipboard_restored),
            diagnostic: None,
        }
    }

    fn clipboard_changed(diagnostic: Option<String>) -> Self {
        Self {
            outcome: PasteOutcome::ClipboardChanged,
            telemetry: InsertionTelemetry::not_applicable(false, None),
            diagnostic,
        }
    }

    /// Record why the previous clipboard was not restored, unless a later
    /// observation already explains this outcome.
    fn with_retention_diagnostic(mut self, diagnostic: Option<String>) -> Self {
        if self.diagnostic.is_none() {
            self.diagnostic = diagnostic;
        }
        self
    }
}

/// Result of staging clipboard text without sending paste or type input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StageOutcome {
    /// The transcript was left on the clipboard.
    CopiedOnly,
    /// The previous clipboard policy was applied after staging.
    Blocked,
    /// Another clipboard value was preserved instead of restoring or copying.
    ClipboardChanged(Option<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PasteDispatch {
    Posted,
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "only the macOS and Linux chord backends can withhold a dispatch for live modifiers"
        )
    )]
    SkippedUnsafeModifiers,
}

/// Maximum time Linux batch paste waits for physical modifiers to be released.
///
/// Matches macOS: releasing Space before Ctrl stops a recording, and a short
/// dictation can finish before the rest of the chord is naturally released.
#[cfg(target_os = "linux")]
const LINUX_PASTE_MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// Clipboard retention policy after staging text for paste.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardPolicy {
    /// Restore previous supported clipboard contents after paste or guarded cancellation.
    RestorePrevious,
    /// Leave the transcript on the clipboard after paste or guarded cancellation.
    KeepTranscript,
}

/// Check whether the configured insertion path can be initialized.
///
/// # Arguments
///
/// * `mode` - Insertion mode to probe.
///
/// # Returns
///
/// `Ok(())` when the required insertion resources are available.
///
/// # Errors
///
/// Returns an error if the keyboard, clipboard, or platform paste support is
/// unavailable.
pub(crate) fn preflight(mode: PasteMode) -> Result<()> {
    let mut injector = Injector::new()?;
    injector.prepare_for_mode(mode)?;
    if mode != PasteMode::Direct || cfg!(target_os = "macos") {
        platform_paste_preflight()?;
    }
    Ok(())
}

/// Exercise the configured insertion backend without inserting into the user's
/// focused application.
///
/// # Arguments
///
/// * `mode` - Insertion mode to validate.
///
/// # Returns
///
/// `Ok(())` when the backend can initialize and the platform smoke test passes.
///
/// # Errors
///
/// Returns an error when the keyboard, clipboard, or platform event backend
/// fails the validation.
pub(crate) fn smoke_test(mode: PasteMode) -> Result<()> {
    preflight(mode)?;
    if mode == PasteMode::Direct && !cfg!(target_os = "macos") {
        return Ok(());
    }
    platform_paste_smoke_test(mode)
}

/// Open a text insertion handle.
pub struct Injector {
    enigo: Option<Enigo>,
    clipboard: Option<Clipboard>,
    #[cfg(target_os = "windows")]
    clipboard_history: Option<super::windows_clipboard_history::ClipboardHistoryListener>,
    #[cfg(target_os = "linux")]
    x11_paste: Option<LinuxX11Paste>,
    #[cfg(target_os = "linux")]
    x11_direct: Option<LinuxX11Paste>,
}

impl Injector {
    /// Create an injector backed by the platform's keyboard API.
    ///
    /// # Returns
    ///
    /// A ready-to-use text inserter.
    ///
    /// # Errors
    ///
    /// Returns an error if the current desktop session is unsupported or if
    /// `enigo` cannot initialize the platform keyboard backend.
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "linux")]
        super::session::ensure_x11_session_supported()?;

        #[cfg(target_os = "windows")]
        let clipboard_history =
            match super::windows_clipboard_history::ClipboardHistoryListener::start() {
                Ok(listener) => Some(listener),
                Err(err) => {
                    clipboard_history_debug(format_args!(
                        "Windows clipboard-history listener unavailable; using timed restore fallback: {err:#}"
                    ));
                    None
                }
            };

        Ok(Self {
            enigo: None,
            clipboard: None,
            #[cfg(target_os = "windows")]
            clipboard_history,
            #[cfg(target_os = "linux")]
            x11_paste: None,
            #[cfg(target_os = "linux")]
            x11_direct: None,
        })
    }

    /// Initialize the platform resources needed for `mode`.
    ///
    /// Linux keeps the X11 paste connection and resolved keycodes warm so a
    /// long-running daemon does not have to rediscover the display during the
    /// narrow paste window after transcription.
    ///
    /// # Arguments
    ///
    /// * `mode` - Insertion mode that will be used later.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the required keyboard, clipboard, and paste handles are
    /// ready.
    ///
    /// # Errors
    ///
    /// Returns an error if a required platform handle cannot be opened.
    pub fn prepare_for_mode(&mut self, mode: PasteMode) -> Result<()> {
        if mode == PasteMode::Direct {
            let _keyboard = self.keyboard()?;
        }

        if mode != PasteMode::Direct && self.clipboard.is_none() {
            self.clipboard = Some(Clipboard::new().context("could not open system clipboard")?);
        }

        #[cfg(target_os = "linux")]
        if mode != PasteMode::Direct && self.x11_paste.is_none() {
            self.x11_paste =
                Some(LinuxX11Paste::open().context("could not initialize X11 paste connection")?);
        }

        Ok(())
    }

    /// Paste text, but re-run a caller-supplied safety check immediately before
    /// synthetic input is sent.
    ///
    /// By default the previous supported clipboard payload is restored after
    /// the paste consume delay. Callers may opt into leaving the transcript on
    /// the clipboard for workflows that prefer that behavior.
    ///
    /// # Arguments
    ///
    /// * `text` - Transcript text to insert.
    /// * `mode` - Paste shortcut style to send after updating the clipboard.
    /// * `clipboard_policy` - Clipboard retention policy after paste or guarded cancellation.
    /// * `focus` - Focus snapshot captured before insertion became eligible,
    ///   when available. Passed through to the post-paste acknowledgement
    ///   strategy (e.g. macOS `AXValue` confirmation polling reads the
    ///   focused Accessibility element from this snapshot).
    ///
    /// # Returns
    ///
    /// A [`PasteReport`] whose `outcome` is [`PasteOutcome::Pasted`] or
    /// [`PasteOutcome::PastedUnverified`] when synthetic input was sent,
    /// [`PasteOutcome::CopiedOnly`] when the guard blocked insertion (or
    /// post-paste acknowledgement never found evidence of insertion) and the
    /// transcript was intentionally left on the clipboard,
    /// [`PasteOutcome::UnsafeModifiers`] when physical modifiers prevented a
    /// safe chord, or
    /// [`PasteOutcome::Blocked`] when no input was sent and the previous
    /// clipboard was restored, or [`PasteOutcome::ClipboardChanged`] when
    /// competing or unreadable clipboard contents were preserved.
    ///
    /// The `UnsafeModifiers` case always keeps the transcript on the
    /// clipboard and ignores `clipboard_policy`, even when the caller asked
    /// for [`ClipboardPolicy::RestorePrevious`]: uncertainty about whether
    /// the chord could be posted safely must not destroy the only copy of
    /// the transcript.
    ///
    /// # Errors
    ///
    /// Returns an error if clipboard staging, the guard, direct typing, or the
    /// paste shortcut fails.
    pub fn paste_text_guarded(
        &mut self,
        text: &str,
        mode: PasteMode,
        clipboard_policy: ClipboardPolicy,
        focus: Option<&FocusSnapshot>,
        before_chord: impl FnMut() -> Result<bool>,
    ) -> Result<PasteReport> {
        if mode == PasteMode::Direct {
            #[cfg(target_os = "linux")]
            {
                self.type_linux_text_guarded(text, before_chord)
                    .map_err(anyhow::Error::new)?;
                return Ok(PasteReport::new(PasteOutcome::Pasted, true, None));
            }
            #[cfg(not(target_os = "linux"))]
            {
                let mut before_chord = before_chord;
                if before_chord()? {
                    self.type_text(text)?;
                    return Ok(PasteReport::new(PasteOutcome::Pasted, true, None));
                }
                anyhow::bail!("direct insertion blocked by safety guard");
            }
        }

        let mut clipboard = self.take_clipboard()?;
        let restore_gate = self.clipboard_restore_gate();
        let restore_plan = ClipboardRestorePlan::new(
            clipboard_restore_delay(),
            restore_gate.paste_consume_delay(),
            &restore_gate,
        );
        // Readiness and dispatch share the platform input backend; the
        // transaction calls them sequentially, never re-entrantly.
        let input = RefCell::new(&mut *self);
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            text,
            mode,
            || input.borrow_mut().wait_for_paste_shortcut_safety(),
            || input.borrow_mut().paste_clipboard(mode),
            clipboard_settle_delay(),
            restore_plan,
            clipboard_policy,
            focus,
            before_chord,
        );
        self.clipboard = Some(clipboard);
        result
    }

    /// Copy text to the clipboard without sending any paste or type event.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard contains `text`.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened or written.
    pub fn copy_text(&mut self, text: &str) -> Result<()> {
        let mut clipboard = self.take_clipboard()?;
        let result = clipboard
            .set_text(text.to_owned())
            .context("could not copy transcript to clipboard");
        self.clipboard = Some(clipboard);
        result
    }

    /// Stage text without sending any paste or type event, then apply the
    /// configured clipboard retention policy.
    ///
    /// This is used for blocked insertion paths so clipboard history managers
    /// can still observe the transcript while the active clipboard is restored
    /// by default.
    ///
    /// # Arguments
    ///
    /// * `text` - Transcript text to stage.
    /// * `clipboard_policy` - Policy deciding whether the transcript remains
    ///   on the active clipboard.
    ///
    /// # Returns
    ///
    /// [`StageOutcome::CopiedOnly`] when the transcript remains on the active
    /// clipboard, or [`StageOutcome::Blocked`] when the previous clipboard was
    /// restored.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened, written, or restored.
    pub fn stage_text_for_history(
        &mut self,
        text: &str,
        clipboard_policy: ClipboardPolicy,
    ) -> Result<StageOutcome> {
        let mut clipboard = self.take_clipboard()?;
        let restore_gate = self.clipboard_restore_gate();
        let restore_plan = ClipboardRestorePlan::new(
            clipboard_restore_delay(),
            restore_gate.paste_consume_delay(),
            &restore_gate,
        );
        let result = stage_text_without_paste(&mut clipboard, text, restore_plan, clipboard_policy);
        self.clipboard = Some(clipboard);
        result
    }

    fn take_clipboard(&mut self) -> Result<Clipboard> {
        match self.clipboard.take() {
            Some(clipboard) => Ok(clipboard),
            None => Clipboard::new().context("could not open system clipboard"),
        }
    }

    /// Type the given text as synthetic keystrokes at the focused cursor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the text was accepted by the platform backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform backend rejects the synthetic typing
    /// request.
    fn type_text(&mut self, text: &str) -> Result<()> {
        self.keyboard()?
            .text(text)
            .map_err(|e| anyhow::anyhow!("enigo type failed: {e:?}"))
            .context("could not type text at cursor")
    }

    /// Query Linux modifiers through the direct-typing connection.
    ///
    /// A failed query discards the cached connection so the next dictation
    /// reopens it instead of remaining broken until restart.
    #[cfg(target_os = "linux")]
    fn linux_direct_modifiers_held(&mut self) -> Result<bool> {
        if self.x11_direct.is_none() {
            self.x11_direct = Some(LinuxX11Paste::open_for_mode(PasteMode::Direct)?);
        }
        let result = self
            .x11_direct
            .as_mut()
            .expect("X11 modifier probe was just initialized")
            .modifiers_held();
        if result.is_err() {
            self.x11_direct = None;
        }
        result
    }

    /// Type Linux text while checking physical modifiers and focus per character.
    ///
    /// # Errors
    ///
    /// Rejects control characters, held or unreadable modifiers, changed focus,
    /// and failed key events. A mid-text failure stops further insertion.
    #[cfg(target_os = "linux")]
    fn type_linux_text_guarded(
        &mut self,
        text: &str,
        before_character: impl FnMut() -> Result<bool>,
    ) -> std::result::Result<usize, DirectTypingFailure> {
        let input = RefCell::new(self);
        direct::type_text_guarded(
            text,
            LINUX_PASTE_MODIFIER_RELEASE_TIMEOUT,
            || input.borrow_mut().linux_direct_modifiers_held(),
            before_character,
            |character| {
                let mut encoded = [0; 4];
                input
                    .borrow_mut()
                    .type_text(character.encode_utf8(&mut encoded))
            },
        )
    }

    /// Wait until no held modifier can alter or interrupt the paste chord.
    ///
    /// Runs before the transaction's final focus recheck, so waiting cannot
    /// leave a stale focus decision authorizing the chord.
    ///
    /// # Returns
    ///
    /// `false` when a modifier stayed down for the whole bounded wait.
    #[cfg(target_os = "linux")]
    fn wait_for_paste_shortcut_safety(&mut self) -> bool {
        if self.x11_paste.is_none() {
            match LinuxX11Paste::open() {
                Ok(paste) => self.x11_paste = Some(paste),
                // Dispatch reopens X11 and reports the connection error.
                Err(_) => return true,
            }
        }
        let paste = self
            .x11_paste
            .as_mut()
            .expect("X11 paste backend was just initialized");
        let ready = wait_for_modifier_release(LINUX_PASTE_MODIFIER_RELEASE_TIMEOUT, || {
            paste.modifiers_held()
        });
        ready.unwrap_or_else(|_| {
            // Dispatch reopens X11 and rechecks modifiers before sending input.
            self.x11_paste = None;
            true
        })
    }

    /// Wait until no held modifier can alter the synthetic Cmd+V chord.
    ///
    /// # Returns
    ///
    /// `false` when a conflicting key stayed down for the whole bounded wait.
    #[cfg(target_os = "macos")]
    fn wait_for_paste_shortcut_safety(&mut self) -> bool {
        crate::daemon::macos::wait_for_safe_paste_modifiers()
    }

    /// Windows sends its chord without a modifier readiness wait.
    ///
    /// # Returns
    ///
    /// Always `true`.
    #[cfg(target_os = "windows")]
    fn wait_for_paste_shortcut_safety(&mut self) -> bool {
        true
    }

    #[cfg(target_os = "linux")]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<PasteDispatch> {
        if self.x11_paste.is_none() {
            self.x11_paste = Some(LinuxX11Paste::open()?);
        }

        let result = self
            .x11_paste
            .as_mut()
            .expect("X11 paste backend was just initialized")
            .send_paste_chord(mode);
        if result.is_err() {
            self.x11_paste = None;
        }
        result
    }

    #[cfg(target_os = "windows")]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<PasteDispatch> {
        let use_shift = mode == PasteMode::Terminal;
        super::windows_input::send_paste_chord(use_shift)
            .context("could not send Windows paste shortcut")?;
        Ok(PasteDispatch::Posted)
    }

    #[cfg(target_os = "macos")]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<PasteDispatch> {
        match mode {
            PasteMode::Standard | PasteMode::Terminal => {
                match crate::daemon::macos::send_paste_shortcut()
                    .context("could not send macOS paste shortcut")?
                {
                    crate::daemon::macos::PasteShortcutOutcome::Sent => Ok(PasteDispatch::Posted),
                    crate::daemon::macos::PasteShortcutOutcome::UnsafeModifiers => {
                        Ok(PasteDispatch::SkippedUnsafeModifiers)
                    }
                }
            }
            PasteMode::Direct => anyhow::bail!("direct mode does not use the paste shortcut"),
        }
    }

    fn keyboard(&mut self) -> Result<&mut Enigo> {
        if self.enigo.is_none() {
            self.enigo = Some(
                Enigo::new(&Settings::default())
                    .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?,
            );
        }
        Ok(self.enigo.as_mut().expect("enigo was just initialized"))
    }

    fn clipboard_restore_gate(&self) -> PlatformClipboardRestoreGate {
        #[cfg(target_os = "windows")]
        {
            PlatformClipboardRestoreGate::from_listener(self.clipboard_history.as_ref())
        }
        #[cfg(not(target_os = "windows"))]
        {
            PlatformClipboardRestoreGate::fallback()
        }
    }
}

#[cfg(target_os = "linux")]
fn platform_paste_preflight() -> Result<()> {
    linux_x11_xtest_preflight()
}

#[cfg(target_os = "macos")]
fn platform_paste_preflight() -> Result<()> {
    crate::daemon::macos::accessibility_preflight()
}

#[cfg(target_os = "windows")]
fn platform_paste_preflight() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_paste_smoke_test(mode: PasteMode) -> Result<()> {
    inject_smoke::linux_x11_paste_smoke_test(mode)
}

#[cfg(target_os = "windows")]
fn platform_paste_smoke_test(mode: PasteMode) -> Result<()> {
    super::windows_paste_smoke::windows_paste_smoke_test(mode)
}

/// Run the macOS `doctor --deep` insertion smoke test.
///
/// Direct mode runs a single stage: a suppressed synthetic key-event tap
/// proving parakit can post keystrokes at all (there is no clipboard/paste
/// chord/`AXValue` acknowledgement pipeline in direct-typing mode for a
/// second stage to exercise).
///
/// Standard/Terminal mode runs two stages, and reports which one failed:
///   1. A suppressed Cmd+V event tap (fast, low-level, side-effect-free)
///      proving parakit can post a full paste chord.
///   2. [`crate::daemon::macos::real_paste_transaction_smoke_test`], which
///      pastes a sentinel through the production guarded-paste transaction
///      into a throwaway text view and verifies it actually landed, was
///      `AXValue`-acknowledged, and the clipboard was restored.
#[cfg(target_os = "macos")]
fn platform_paste_smoke_test(mode: PasteMode) -> Result<()> {
    let mut injector = Injector::new()?;
    match mode {
        PasteMode::Direct => {
            crate::daemon::macos::suppressed_key_event_smoke(|| injector.type_text("a"))
                .context("macOS insertion smoke stage 1 (suppressed key-event tap) failed")
        }
        PasteMode::Standard | PasteMode::Terminal => {
            crate::daemon::macos::suppressed_paste_shortcut_smoke(|| {
                match injector.paste_clipboard(mode)? {
                    PasteDispatch::Posted => Ok(()),
                    PasteDispatch::SkippedUnsafeModifiers => {
                        anyhow::bail!("physical push-to-talk keys remained held")
                    }
                }
            })
            .context("macOS insertion smoke stage 1 (suppressed paste-shortcut tap) failed")?;
            crate::daemon::macos::real_paste_transaction_smoke_test(mode)
                .context("macOS insertion smoke stage 2 (real paste-transaction) failed")
        }
    }
}

/// macOS delay between staging transcript text and the final focus check.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_CLIPBOARD_SETTLE_DELAY: Duration = Duration::from_millis(200);

fn clipboard_settle_delay() -> Duration {
    #[cfg(target_os = "linux")]
    {
        Duration::from_millis(150)
    }
    #[cfg(target_os = "macos")]
    {
        MACOS_CLIPBOARD_SETTLE_DELAY
    }
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(50)
    }
}

fn clipboard_restore_delay() -> Duration {
    #[cfg(target_os = "linux")]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "macos")]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(750)
    }
}

#[cfg(test)]
#[path = "inject_tests.rs"]
mod inject_tests;
