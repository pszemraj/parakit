//! Clipboard restore timing and history-observation policy.

use std::thread;
use std::time::{Duration, Instant};

use super::inject::FocusSnapshot;

#[cfg(target_os = "windows")]
const CLIPBOARD_CONFIRM_TIMEOUT: Duration = Duration::from_millis(1000);
#[cfg(target_os = "windows")]
const CLIPBOARD_HISTORY_CONFIRM_GRACE: Duration = Duration::from_millis(50);
#[cfg(target_os = "windows")]
const CLIPBOARD_PASTE_CONSUME_DELAY: Duration = Duration::from_millis(200);

/// Clipboard observation state captured before staging transcript text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ClipboardWriteSnapshot {
    pub(super) sequence: Option<u32>,
}

/// Clipboard observation token for one staged transcript write.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ClipboardWriteToken {
    pub(super) before_sequence: Option<u32>,
    pub(super) after_sequence: Option<u32>,
}

/// Focus and transcript context available to a paste-acknowledgement
/// strategy. Kept as a struct so a future confirmation strategy can read
/// more context without changing the [`ClipboardRestoreGate`] trait's
/// method signature again.
///
/// `pub(crate)` rather than `pub(super)`: the macOS override built on top
/// of this trait lives in `daemon::macos`, a sibling of `daemon::desktop`,
/// so both this type and [`PasteConfirmation`] must be visible crate-wide
/// to cross that module boundary.
pub(crate) struct PasteConfirmationContext<'a> {
    /// Focus captured before insertion became eligible, when available.
    pub(crate) focus: Option<&'a FocusSnapshot>,
    /// Transcript text that was just pasted.
    pub(crate) transcript: &'a str,
    /// Target's observable value as read *before* the paste chord was sent,
    /// via [`ClipboardRestoreGate::capture_paste_baseline`].
    ///
    /// This is what makes "the value grew" trustworthy evidence. Reading the
    /// baseline after the chord races the target: an app that refreshes its
    /// accessibility tree coarsely (terminals especially) can already have
    /// the pasted text in the first post-chord read, after which the value
    /// never grows again and a successful paste looks exactly like a failed
    /// one. `None` when no pre-chord read was possible, in which case the
    /// confirmation strategy falls back to a post-chord baseline.
    pub(crate) baseline: Option<&'a str>,
}

/// Result of waiting for evidence that a just-sent paste chord was consumed
/// by the insertion target.
///
/// Every variant carries the stable telemetry label
/// (`InsertionLogFields::acknowledgement_kind`) that explains how the
/// variant was decided, alongside how long the wait took.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasteConfirmation {
    /// Insertion was positively observed (or, on platforms without a
    /// stronger signal, this is simply how long the fallback/history wait
    /// took — see `kind`).
    Confirmed {
        /// Time spent waiting for confirmation.
        elapsed: Duration,
        /// Stable telemetry label, e.g. `"ax_confirmed"` or
        /// `"not_applicable"` on platforms without acknowledgement
        /// machinery.
        kind: &'static str,
    },
    /// No positive evidence was available at all (no pollable signal), so a
    /// fixed grace period was used instead. The paste chord was sent; it is
    /// simply unknown whether it landed.
    Unverified {
        /// Time spent in the grace period.
        elapsed: Duration,
        /// Stable telemetry label, e.g. `"unverified_timeout"`.
        kind: &'static str,
    },
    /// A pollable signal was available but never showed insertion evidence
    /// before the deadline.
    NoEvidence {
        /// Time spent polling for evidence.
        elapsed: Duration,
        /// Stable telemetry label, e.g. `"no_evidence"`.
        kind: &'static str,
    },
}

/// Clipboard restore gate used to wait until listeners observe a staged write.
pub(super) trait ClipboardRestoreGate {
    /// Capture state immediately before the transcript is written.
    ///
    /// # Returns
    ///
    /// Clipboard observation state that can be paired with the post-write
    /// state.
    fn before_transcript_write(&self) -> ClipboardWriteSnapshot;

    /// Capture state immediately after the transcript is written.
    ///
    /// # Returns
    ///
    /// A token identifying the transcript clipboard write to wait on.
    fn after_transcript_write(&self, before: ClipboardWriteSnapshot) -> ClipboardWriteToken;

    /// Wait until the transcript write has been observed, or until fallback.
    ///
    /// # Arguments
    ///
    /// * `token` - Clipboard write token returned after staging the transcript.
    /// * `fallback_delay` - Time-based restore delay used when observation is
    ///   unavailable.
    fn wait_before_restore(&self, token: ClipboardWriteToken, fallback_delay: Duration);

    /// Read the insertion target's observable value immediately *before* the
    /// paste chord is sent, to be handed back as
    /// [`PasteConfirmationContext::baseline`].
    ///
    /// Called on the hot path between the final focus recheck and the chord,
    /// so an implementation must be non-blocking; returning `None` is always
    /// acceptable and merely degrades confirmation to a post-chord baseline.
    ///
    /// # Arguments
    ///
    /// * `_focus` - Focus snapshot the chord is about to target, when
    ///   available.
    ///
    /// # Returns
    ///
    /// `None` in the default implementation: only macOS has a pollable
    /// per-element value to baseline against.
    fn capture_paste_baseline(&self, _focus: Option<&FocusSnapshot>) -> Option<String> {
        None
    }

    /// Await evidence that a just-sent paste chord was consumed by the
    /// insertion target, before the caller decides whether to restore the
    /// previous clipboard contents.
    ///
    /// The default implementation reproduces the historical behavior of
    /// every gate that does not have a stronger acknowledgement signal:
    /// sleep `paste_consume_delay` so the target has a chance to read the
    /// clipboard, then apply [`Self::wait_before_restore`]'s existing
    /// policy. It always resolves to [`PasteConfirmation::Confirmed`] with
    /// `kind: "not_applicable"`, since no platform-specific positive signal
    /// was consulted.
    ///
    /// # Arguments
    ///
    /// * `token` - Clipboard write token for the staged transcript.
    /// * `fallback_delay` - Time-based restore delay used when observation
    ///   is unavailable.
    /// * `paste_consume_delay` - Extra delay after a successful paste chord
    ///   so the target can consume the clipboard before restore.
    /// * `_ctx` - Focus/transcript context. Unused by the default
    ///   implementation; available to overrides with a real acknowledgement
    ///   signal (see the macOS override on [`PlatformClipboardRestoreGate`]).
    ///
    /// # Returns
    ///
    /// The confirmation tier the gate observed. The default implementation
    /// always returns [`PasteConfirmation::Confirmed`] with
    /// `kind: "not_applicable"`.
    fn await_paste_confirmation(
        &self,
        token: ClipboardWriteToken,
        fallback_delay: Duration,
        paste_consume_delay: Duration,
        _ctx: &PasteConfirmationContext<'_>,
    ) -> PasteConfirmation {
        default_paste_confirmation(self, token, fallback_delay, paste_consume_delay)
    }
}

/// Shared body for [`ClipboardRestoreGate::await_paste_confirmation`]'s
/// default implementation, factored out so test gates can reuse the exact
/// same fallback behavior instead of duplicating it (see
/// `MockRestoreGate::await_paste_confirmation` in `inject_tests.rs`).
///
/// # Arguments
///
/// * `gate` - Gate to wait on.
/// * `token` - Clipboard write token for the staged transcript.
/// * `fallback_delay` - Time-based restore delay used when observation is
///   unavailable.
/// * `paste_consume_delay` - Extra delay after a successful paste chord so
///   the target can consume the clipboard before restore.
///
/// # Returns
///
/// Always [`PasteConfirmation::Confirmed`] with `kind: "not_applicable"`,
/// since no positive platform signal is consulted on this path.
pub(super) fn default_paste_confirmation<G: ClipboardRestoreGate + ?Sized>(
    gate: &G,
    token: ClipboardWriteToken,
    fallback_delay: Duration,
    paste_consume_delay: Duration,
) -> PasteConfirmation {
    let start = Instant::now();
    sleep_if_nonzero(paste_consume_delay);
    gate.wait_before_restore(token, fallback_delay);
    PasteConfirmation::Confirmed {
        elapsed: start.elapsed(),
        kind: "not_applicable",
    }
}

/// Restore timing policy for one staged clipboard write.
#[derive(Clone, Copy)]
pub(super) struct ClipboardRestorePlan<'a, G: ClipboardRestoreGate + ?Sized> {
    delay: Duration,
    paste_consume_delay: Duration,
    gate: &'a G,
}

impl<'a, G: ClipboardRestoreGate + ?Sized> ClipboardRestorePlan<'a, G> {
    /// Build a restore plan from a fallback delay, paste delay, and gate.
    ///
    /// # Arguments
    ///
    /// * `delay` - Fallback delay used when observation is unavailable.
    /// * `paste_consume_delay` - Extra delay after a successful paste chord so
    ///   the target can consume the clipboard before restore.
    /// * `gate` - Clipboard observation gate.
    ///
    /// # Returns
    ///
    /// A restore plan for the current clipboard write.
    pub(super) fn new(delay: Duration, paste_consume_delay: Duration, gate: &'a G) -> Self {
        Self {
            delay,
            paste_consume_delay,
            gate,
        }
    }

    /// Capture state before writing transcript text.
    ///
    /// # Returns
    ///
    /// Clipboard observation state.
    pub(super) fn before_transcript_write(&self) -> ClipboardWriteSnapshot {
        self.gate.before_transcript_write()
    }

    /// Capture state after writing transcript text.
    ///
    /// # Arguments
    ///
    /// * `before` - State captured before the write.
    ///
    /// # Returns
    ///
    /// Clipboard write token for restore gating.
    pub(super) fn after_transcript_write(
        &self,
        before: ClipboardWriteSnapshot,
    ) -> ClipboardWriteToken {
        self.gate.after_transcript_write(before)
    }

    /// Read the target's observable value before the paste chord is sent.
    ///
    /// # Arguments
    ///
    /// * `focus` - Focus snapshot the chord is about to target.
    ///
    /// # Returns
    ///
    /// The pre-chord baseline, or `None` when the platform has no pollable
    /// value.
    pub(super) fn capture_paste_baseline(&self, focus: Option<&FocusSnapshot>) -> Option<String> {
        self.gate.capture_paste_baseline(focus)
    }

    /// Await evidence that a just-sent paste chord was consumed by the
    /// insertion target.
    ///
    /// # Arguments
    ///
    /// * `token` - Clipboard write token for the staged transcript.
    /// * `ctx` - Focus/transcript context for platform confirmation
    ///   strategies that have a real acknowledgement signal.
    ///
    /// # Returns
    ///
    /// The confirmation tier reported by the underlying gate.
    pub(super) fn await_paste_confirmation(
        &self,
        token: ClipboardWriteToken,
        ctx: &PasteConfirmationContext<'_>,
    ) -> PasteConfirmation {
        self.gate
            .await_paste_confirmation(token, self.delay, self.paste_consume_delay, ctx)
    }

    /// Wait before restoring the previous clipboard.
    ///
    /// # Arguments
    ///
    /// * `token` - Clipboard write token for the staged transcript.
    pub(super) fn wait_before_restore(&self, token: ClipboardWriteToken) {
        self.gate.wait_before_restore(token, self.delay);
    }
}

/// Platform restore gate used by the production injector.
#[derive(Clone)]
pub(super) struct PlatformClipboardRestoreGate {
    #[cfg(target_os = "windows")]
    history: Option<super::windows_clipboard_history::ClipboardHistoryHandle>,
}

impl PlatformClipboardRestoreGate {
    /// Build a time-based fallback gate.
    ///
    /// # Returns
    ///
    /// A gate that waits the fallback restore delay.
    pub(super) fn fallback() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            history: None,
        }
    }

    #[cfg(target_os = "windows")]
    /// Build a Windows restore gate from an optional clipboard listener.
    ///
    /// # Arguments
    ///
    /// * `listener` - Persistent Windows clipboard update listener.
    ///
    /// # Returns
    ///
    /// A listener-backed gate when available, otherwise a fallback gate.
    pub(super) fn from_listener(
        listener: Option<&super::windows_clipboard_history::ClipboardHistoryListener>,
    ) -> Self {
        match listener {
            Some(listener) => Self {
                history: Some(listener.handle()),
            },
            None => Self::fallback(),
        }
    }

    #[cfg(target_os = "windows")]
    /// Return the post-paste consume delay for this gate.
    ///
    /// # Returns
    ///
    /// A nonzero delay only when listener-backed restore is active.
    pub(super) fn paste_consume_delay(&self) -> Duration {
        if self.history.is_some() {
            CLIPBOARD_PASTE_CONSUME_DELAY
        } else {
            Duration::ZERO
        }
    }

    #[cfg(not(target_os = "windows"))]
    /// Return the post-paste consume delay for this gate.
    ///
    /// # Returns
    ///
    /// Always zero on non-Windows targets.
    pub(super) fn paste_consume_delay(&self) -> Duration {
        Duration::ZERO
    }
}

impl ClipboardRestoreGate for PlatformClipboardRestoreGate {
    fn before_transcript_write(&self) -> ClipboardWriteSnapshot {
        #[cfg(target_os = "windows")]
        if let Some(history) = &self.history {
            return ClipboardWriteSnapshot {
                sequence: Some(history.current_sequence()),
            };
        }

        ClipboardWriteSnapshot::default()
    }

    fn after_transcript_write(&self, before: ClipboardWriteSnapshot) -> ClipboardWriteToken {
        #[cfg(target_os = "windows")]
        if let Some(history) = &self.history {
            return ClipboardWriteToken {
                before_sequence: before.sequence,
                after_sequence: Some(history.current_sequence()),
            };
        }
        #[cfg(not(target_os = "windows"))]
        let _ = before;

        ClipboardWriteToken::default()
    }

    fn wait_before_restore(&self, token: ClipboardWriteToken, fallback_delay: Duration) {
        #[cfg(target_os = "windows")]
        if let Some(history) = &self.history {
            wait_for_windows_clipboard_history(history, token, fallback_delay);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = token;

        sleep_if_nonzero(fallback_delay);
    }

    #[cfg(target_os = "macos")]
    fn capture_paste_baseline(&self, focus: Option<&FocusSnapshot>) -> Option<String> {
        crate::daemon::macos::pasteboard::capture_baseline(focus)
    }

    #[cfg(target_os = "macos")]
    fn await_paste_confirmation(
        &self,
        _token: ClipboardWriteToken,
        _fallback_delay: Duration,
        _paste_consume_delay: Duration,
        ctx: &PasteConfirmationContext<'_>,
    ) -> PasteConfirmation {
        // macOS has a real acknowledgement signal (Accessibility `AXValue`
        // polling), so it does not use the clipboard-sequence/history-based
        // wait the trait default applies on other platforms. Linux and
        // Windows have no override here, so they keep using the trait's
        // default `await_paste_confirmation` unchanged.
        crate::daemon::macos::pasteboard::await_paste_confirmation(ctx)
    }
}

fn sleep_if_nonzero(delay: Duration) {
    if !delay.is_zero() {
        thread::sleep(delay);
    }
}

#[cfg(target_os = "windows")]
fn wait_for_windows_clipboard_history(
    history: &super::windows_clipboard_history::ClipboardHistoryHandle,
    token: ClipboardWriteToken,
    fallback_delay: Duration,
) {
    let (Some(before), Some(after)) = (token.before_sequence, token.after_sequence) else {
        sleep_if_nonzero(fallback_delay);
        return;
    };

    if after <= before {
        clipboard_history_debug(format_args!(
            "Windows clipboard sequence did not advance after transcript write; restoring after timeout"
        ));
        sleep_if_nonzero(CLIPBOARD_CONFIRM_TIMEOUT);
        return;
    }

    if history.wait_for_sequence(after, CLIPBOARD_CONFIRM_TIMEOUT) {
        // Clipboard History receives the same WM_CLIPBOARDUPDATE notification
        // path as this listener. The small grace gives cbdhsvc time to finish
        // handling the dispatched update before Parakit restores old content.
        sleep_if_nonzero(CLIPBOARD_HISTORY_CONFIRM_GRACE);
    } else {
        clipboard_history_debug(format_args!(
            "timed out waiting for Windows clipboard-history listener confirmation"
        ));
    }
}

#[cfg(target_os = "windows")]
/// Print a Windows clipboard-history debug message in debug builds.
///
/// # Arguments
///
/// * `message` - Lazily formatted diagnostic.
pub(super) fn clipboard_history_debug(message: impl std::fmt::Display) {
    #[cfg(debug_assertions)]
    eprintln!("parakit: debug: {message}");
    #[cfg(not(debug_assertions))]
    let _ = message;
}
