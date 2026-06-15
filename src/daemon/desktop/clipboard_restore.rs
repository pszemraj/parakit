//! Clipboard restore timing and history-observation policy.

use std::thread;
use std::time::Duration;

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

    /// Wait after a successful paste before restoring the previous clipboard.
    ///
    /// # Arguments
    ///
    /// * `token` - Clipboard write token for the staged transcript.
    pub(super) fn wait_after_paste_before_restore(&self, token: ClipboardWriteToken) {
        sleep_if_nonzero(self.paste_consume_delay);
        self.wait_before_restore(token);
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
