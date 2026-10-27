//! Stable transcript lookup and copying for the interactive history browser.

use super::{HistoryReference, IpcResponse, SharedState, IPC_MAX_MESSAGE_SIZE};
use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Return a token distinguishing daemon starts and isolated state instances.
///
/// # Returns
///
/// A process, start-time, and instance token for transcript references.
pub(super) fn session_token() -> String {
    static INSTANCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        INSTANCE.fetch_add(1, Ordering::Relaxed),
    )
}

impl SharedState {
    /// Resolve the original entry, rejecting restarted daemons and evicted entries.
    ///
    /// # Returns
    ///
    /// The complete remembered text for this reference.
    ///
    /// # Errors
    ///
    /// Returns an error when history is disabled or the reference has expired.
    pub(super) fn resolve_history_reference(&self, reference: &HistoryReference) -> Result<String> {
        self.ensure_history_enabled()?;
        if reference.session != self.history_session {
            bail!("daemon restarted; reopen history to select a transcript");
        }
        self.inner
            .lock()
            .history
            .iter()
            .find(|entry| entry.id == reference.entry)
            .map(|entry| entry.text.clone())
            .context("selected transcript is no longer in history; reopen history")
    }
}

/// Return full text when its encoded response fits the existing control frame.
///
/// # Arguments
///
/// * `state` - Daemon history holding the transcript.
/// * `reference` - Original session and entry selected by the client.
///
/// # Returns
///
/// A response containing the complete transcript.
///
/// # Errors
///
/// Returns lookup errors or an explicit oversized-response error without truncation.
pub(super) fn text_response(
    state: &SharedState,
    reference: &HistoryReference,
) -> Result<IpcResponse> {
    let response = IpcResponse::HistoryText {
        text: state.resolve_history_reference(reference)?,
    };
    if serde_json::to_vec(&response)?.len() > IPC_MAX_MESSAGE_SIZE {
        bail!("transcript is too large for the 64 KiB control response; copy the selected entry with Enter in the history picker");
    }
    Ok(response)
}

/// Copy the original selected entry through the shared insertion transaction.
///
/// # Arguments
///
/// * `state` - Daemon history and insertion lock.
/// * `reference` - Original selected session and entry.
/// * `copy` - Clipboard operation applied while insertion is serialized.
///
/// # Returns
///
/// Success when the selected text was copied.
///
/// # Errors
///
/// Returns lookup or clipboard errors without substituting another transcript.
pub(super) fn copy_selected(
    state: &SharedState,
    reference: &HistoryReference,
    copy: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    state.with_insertion_lock(|| copy(&state.resolve_history_reference(reference)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_survives_new_dictation_without_changing_text() {
        let state = SharedState::with_history_limit(2);
        state.remember_transcript("selected\ntext 雪".into());
        let reference = state.history_snapshot(None)[0].reference.clone();
        state.remember_transcript("newer".into());
        let mut copied = String::new();
        copy_selected(&state, &reference, |text| {
            copied = text.to_owned();
            Ok(())
        })
        .unwrap();
        assert_eq!(copied, "selected\ntext 雪");
    }

    #[test]
    fn eviction_and_restart_never_substitute_entries() {
        let state = SharedState::with_history_limit(1);
        state.remember_transcript("old".into());
        let reference = state.history_snapshot(None)[0].reference.clone();
        state.remember_transcript("replacement".into());
        let error =
            copy_selected(&state, &reference, |_| panic!("expired entry copied")).unwrap_err();
        assert!(error.to_string().contains("no longer in history"));
        let restarted = SharedState::with_history_limit(1);
        restarted.remember_transcript("restarted".into());
        let error = restarted.resolve_history_reference(&reference).unwrap_err();
        assert!(error.to_string().contains("daemon restarted"));
    }

    #[test]
    fn clipboard_failure_is_preserved() {
        let state = SharedState::new();
        state.remember_transcript("selected".into());
        let reference = state.history_snapshot(None)[0].reference.clone();
        let error =
            copy_selected(&state, &reference, |_| bail!("clipboard unavailable")).unwrap_err();
        assert_eq!(error.to_string(), "clipboard unavailable");
        assert_eq!(
            state.resolve_history_reference(&reference).unwrap(),
            "selected"
        );
    }

    #[test]
    fn oversized_preview_errors_without_truncating_stored_text() {
        let state = SharedState::new();
        let text = "\n".repeat(IPC_MAX_MESSAGE_SIZE / 2);
        state.remember_transcript(text.clone());
        let reference = state.history_snapshot(None)[0].reference.clone();
        state.remember_transcript("newest entry".into());
        let message = text_response(&state, &reference).unwrap_err().to_string();
        assert!(message.contains("64 KiB"));
        assert!(message.contains("Enter in the history picker"));
        assert!(!message.contains("copy-last"));
        assert_eq!(state.resolve_history_reference(&reference).unwrap(), text);
        let mut copied = String::new();
        copy_selected(&state, &reference, |selected| {
            copied = selected.to_owned();
            Ok(())
        })
        .unwrap();
        assert_eq!(copied, text);
    }
}
