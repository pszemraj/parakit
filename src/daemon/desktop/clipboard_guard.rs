//! Preserve competing clipboard writes during a staged insertion.
//!
//! Native stamps and text read-back bound the staging/restore races. They are
//! observations, not a lock on another application's future clipboard read.
//! In particular, X11 exposes an owner window rather than a content generation.

use super::{restore_or_clear_clipboard, ClipboardPolicy, ClipboardSnapshot, ClipboardStore};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::Result;

/// Clipboard state displaced by a transcript, paired with its staged value.
pub(super) struct StagedClipboard {
    previous: ClipboardSnapshot,
    transcript: String,
    stamp: Option<u64>,
}

/// Whether the previous clipboard was restored, retained, or superseded.
pub(super) enum ClipboardRestore {
    Restored,
    KeptTranscript,
    Changed,
}

impl StagedClipboard {
    /// Record the stamp immediately after staging a transcript.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard containing the staged text.
    /// * `previous` - Supported payload displaced by the write.
    /// * `transcript` - Text the insertion may consume.
    ///
    /// # Returns
    ///
    /// A guard; failed observations make later checks fail closed.
    pub(super) fn capture<C: ClipboardStore>(
        clipboard: &mut C,
        previous: ClipboardSnapshot,
        transcript: &str,
    ) -> Self {
        Self {
            previous,
            transcript: transcript.to_owned(),
            stamp: clipboard.change_stamp().ok(),
        }
    }

    /// Check text between two stamp reads, detecting writes during the read.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard to inspect without changing it.
    ///
    /// # Returns
    ///
    /// Whether the staged text and its observed owner/generation still match.
    pub(super) fn is_current<C: ClipboardStore>(&self, clipboard: &mut C) -> bool {
        self.stamp_is_current(clipboard)
            && clipboard
                .get_text()
                .is_ok_and(|text| text == self.transcript)
            && self.stamp_is_current(clipboard)
    }

    /// Recheck only the native stamp after the final focus guard.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard whose current stamp should be read.
    ///
    /// # Returns
    ///
    /// Whether the stamp remains readable and equal to the staged stamp.
    pub(super) fn stamp_is_current<C: ClipboardStore>(&self, clipboard: &mut C) -> bool {
        self.stamp.is_some() && clipboard.change_stamp().ok() == self.stamp
    }

    /// Restore only while the clipboard still matches the staged transcript.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard to conditionally restore.
    /// * `policy` - Whether to restore the snapshot or retain the transcript.
    ///
    /// # Returns
    ///
    /// The applied policy, or `Changed` when the current clipboard is preserved.
    ///
    /// # Errors
    ///
    /// Returns an error if writing the previous payload fails.
    pub(super) fn restore<C: ClipboardStore>(
        self,
        clipboard: &mut C,
        policy: ClipboardPolicy,
    ) -> Result<ClipboardRestore> {
        if !self.is_current(clipboard) {
            return Ok(ClipboardRestore::Changed);
        }
        restore_or_clear_clipboard(clipboard, self.previous, policy)?;
        Ok(match policy {
            ClipboardPolicy::RestorePrevious => ClipboardRestore::Restored,
            ClipboardPolicy::KeepTranscript => ClipboardRestore::KeptTranscript,
        })
    }
}

/// Observe the native clipboard without requesting its payload.
///
/// # Returns
///
/// A Windows/macOS write generation or an X11 selection owner window.
///
/// # Errors
///
/// Returns an error when the native observation is unavailable.
pub(super) fn platform_change_stamp() -> Result<u64> {
    #[cfg(target_os = "linux")]
    {
        use x11rb::protocol::xproto::ConnectionExt;
        let (connection, _) =
            x11rb::connect(None).context("open clipboard observation connection")?;
        let selection = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        let owner = connection.get_selection_owner(selection)?.reply()?.owner;
        anyhow::ensure!(owner != x11rb::NONE, "clipboard has no owner");
        Ok(u64::from(owner))
    }
    #[cfg(target_os = "windows")]
    {
        // SAFETY: This read-only Windows API takes no pointers or handles.
        let sequence =
            unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        anyhow::ensure!(sequence != 0, "clipboard sequence unavailable");
        Ok(u64::from(sequence))
    }
    #[cfg(target_os = "macos")]
    {
        objc2::rc::autoreleasepool(|_| {
            let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
            u64::try_from(pasteboard.changeCount()).context("invalid clipboard change count")
        })
    }
}
