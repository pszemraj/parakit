//! Preserve competing clipboard writes during a staged insertion.
//!
//! Native stamps and text read-back bound the staging/restore races. They are
//! observations, not a lock on another application's future clipboard read.
//! In particular, X11 exposes an owner window rather than a content generation.

use super::{restore_or_clear_clipboard, ClipboardPolicy, ClipboardSnapshot, ClipboardStore};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::Result;
use std::cell::{Cell, RefCell};

/// Clipboard state displaced by a transcript, paired with its staged value.
pub(super) struct StagedClipboard {
    previous: ClipboardSnapshot,
    transcript: String,
    stamp: Cell<Option<u64>>,
    original_stamp: Option<u64>,
    observation_error: RefCell<Option<String>>,
}

/// Payload and owner observed before staging can replace the clipboard.
pub(super) struct ClipboardBeforeStaging {
    pub(super) snapshot: ClipboardSnapshot,
    stamp: Result<u64, String>,
}

impl ClipboardBeforeStaging {
    /// Capture the owner before reading the supported clipboard formats.
    ///
    /// # Returns
    ///
    /// The supported payload and its initial owner observation.
    ///
    /// # Errors
    ///
    /// Returns an error when a supported payload cannot be read.
    pub(super) fn capture<C: ClipboardStore>(clipboard: &mut C) -> Result<Self> {
        let stamp = clipboard.change_stamp().map_err(|err| format!("{err:#}"));
        let snapshot = ClipboardSnapshot::capture(clipboard)?;
        Ok(Self { snapshot, stamp })
    }

    /// Check the captured owner immediately before writing the transcript.
    ///
    /// # Returns
    ///
    /// Whether the owner is unchanged since snapshot capture began.
    ///
    /// # Errors
    ///
    /// Returns an error when either owner observation failed.
    pub(super) fn is_current<C: ClipboardStore>(&self, clipboard: &mut C) -> Result<bool> {
        let before = self
            .stamp
            .as_ref()
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(*before == clipboard.change_stamp()?)
    }
}

/// Whether the previous clipboard was restored, retained, or superseded.
pub(super) enum ClipboardRestore {
    Restored,
    KeptTranscript,
    Changed(Option<String>),
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
        let (stamp, observation_error) = match clipboard.change_stamp() {
            Ok(stamp) => (Some(stamp), None),
            Err(err) => (
                None,
                Some(format!("could not capture clipboard stamp: {err:#}")),
            ),
        };
        Self {
            previous,
            transcript: transcript.to_owned(),
            stamp: Cell::new(stamp),
            original_stamp: stamp,
            observation_error: RefCell::new(observation_error),
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
        let before = match clipboard.change_stamp() {
            Ok(stamp) => stamp,
            Err(err) => {
                self.record_observation_error("could not read clipboard stamp", err);
                return false;
            }
        };
        let Some(staged) = self.stamp.get() else {
            return false;
        };
        if before != staged && !cfg!(target_os = "linux") {
            return false;
        }
        let current_text = match clipboard.get_text() {
            Ok(text) => text,
            #[cfg(target_os = "linux")]
            Err(err) if super::clipboard_content_unavailable(&err) => return false,
            Err(err) => {
                self.record_observation_error("could not read clipboard text", err);
                return false;
            }
        };
        if current_text != self.transcript {
            return false;
        }
        // X11 managers may acquire the selection while retaining our text.
        // Accept that handoff, but preserve a newly copied rich payload even
        // when its plain-text alternative happens to equal the transcript.
        if before != staged
            && (clipboard.get_html().is_ok()
                || clipboard.get_file_list().is_ok()
                || clipboard.get_image().is_ok())
        {
            return false;
        }
        match clipboard.change_stamp() {
            Ok(after) if after == before => {}
            Ok(_) => return false,
            Err(err) => {
                self.record_observation_error("could not recheck clipboard stamp", err);
                return false;
            }
        }
        self.stamp.set(Some(before));
        self.clear_observation_error();
        true
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
        let Some(staged) = self.stamp.get() else {
            return false;
        };
        match clipboard.change_stamp() {
            Ok(current) => {
                self.clear_observation_error();
                current == staged
            }
            Err(err) => {
                self.record_observation_error("could not recheck clipboard stamp", err);
                false
            }
        }
    }

    /// Most recent clipboard observation error, when a read failed.
    ///
    /// # Returns
    ///
    /// The latest formatted read failure, or `None` when every observation
    /// succeeded.
    pub(super) fn observation_error(&self) -> Option<String> {
        self.observation_error.borrow().clone()
    }

    fn record_observation_error(&self, context: &str, err: anyhow::Error) {
        *self.observation_error.borrow_mut() = Some(format!("{context}: {err:#}"));
    }

    fn clear_observation_error(&self) {
        self.observation_error.borrow_mut().take();
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
            return Ok(ClipboardRestore::Changed(self.observation_error()));
        }
        if cfg!(target_os = "linux")
            && policy == ClipboardPolicy::RestorePrevious
            && matches!(self.previous, ClipboardSnapshot::Unsupported)
        {
            // Clearing an unsupported snapshot is safe only while our own
            // original write owns the selection, even after a manager handoff.
            match clipboard.change_stamp() {
                Ok(stamp) if Some(stamp) == self.original_stamp => {}
                Ok(_) => return Ok(ClipboardRestore::Changed(self.observation_error())),
                Err(err) => {
                    self.record_observation_error("could not recheck clipboard stamp", err);
                    return Ok(ClipboardRestore::Changed(self.observation_error()));
                }
            }
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
        // NONE is a valid observation of an empty selection before staging.
        Ok(u64::from(owner))
    }
    #[cfg(target_os = "windows")]
    {
        // SAFETY: This read-only Windows API takes no pointers or handles.
        let sequence =
            unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        // Treat the counter as an observation, including zero before staging.
        // Snapshot/read/write operations still report actual access failures.
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
