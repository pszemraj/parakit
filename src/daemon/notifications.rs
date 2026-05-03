//! Desktop notifications for actionable daemon fallbacks.

use std::sync::Arc;

use super::{audio::MicInfo, logging::Logger};

/// Thin notification wrapper with stderr logging fallback.
#[derive(Clone)]
pub(crate) struct Notifier {
    log: Arc<Logger>,
}

impl Notifier {
    /// Build a notifier.
    ///
    /// # Arguments
    ///
    /// * `log` - Logger used when the desktop notification backend fails.
    ///
    /// # Returns
    ///
    /// A notifier that falls back to verbose logging when notifications fail.
    pub(crate) fn new(log: Arc<Logger>) -> Self {
        Self { log }
    }

    /// Notify that a transcript was copied without sending a paste chord.
    ///
    /// # Arguments
    ///
    /// * `reason` - Short user-facing reason for leaving transcript text on the clipboard.
    pub(crate) fn transcript_copied(&self, reason: impl AsRef<str>) {
        self.show("Transcript copied", reason.as_ref());
    }

    /// Notify that a transcript was blocked before clipboard staging.
    ///
    /// # Arguments
    ///
    /// * `reason` - Short reason for the block.
    pub(crate) fn paste_blocked(&self, reason: impl AsRef<str>) {
        self.show("Paste blocked", reason.as_ref());
    }

    /// Notify that paste is disabled after repeated insertion failures.
    pub(crate) fn paste_disabled_for_session(&self) {
        self.show(
            "Paste disabled",
            "Repeated insertion failures disabled automatic paste for this session.",
        );
    }

    /// Notify that microphone capture failed and the daemon is trying to reopen it.
    ///
    /// # Arguments
    ///
    /// * `error` - Stream or device error summary.
    pub(crate) fn microphone_unavailable(&self, error: impl AsRef<str>) {
        self.show(
            "Microphone unavailable",
            format!("Audio capture failed: {}. Retrying.", error.as_ref()),
        );
    }

    /// Notify that microphone capture recovered after a failure.
    ///
    /// # Arguments
    ///
    /// * `mic` - Reopened microphone summary.
    pub(crate) fn microphone_recovered(&self, mic: &MicInfo) {
        self.show("Microphone recovered", mic.summary());
    }

    fn show(&self, summary: &str, body: impl AsRef<str>) {
        if let Err(err) = show_notification(summary, body.as_ref()) {
            self.log
                .verbose(format!("parakit: desktop notification failed: {err:#}"));
        }
    }
}

#[cfg(target_os = "linux")]
fn show_notification(summary: &str, body: &str) -> anyhow::Result<()> {
    notify_rust::Notification::new()
        .appname("parakit")
        .summary(summary)
        .body(body)
        .show()?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn show_notification(_summary: &str, _body: &str) -> anyhow::Result<()> {
    Ok(())
}
