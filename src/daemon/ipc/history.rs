//! Stable transcript lookup and copying for the interactive history browser.

use super::{HistoryReference, IpcResponse, SharedState, IPC_MAX_MESSAGE_SIZE};
use anyhow::{bail, Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_CONTEXT_CHARS: usize = 1500;

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
    /// Read recent stored dictations oldest first, retaining the newest 1500 characters.
    ///
    /// # Arguments
    /// * `count` - Maximum number of eligible history entries; zero disables context.
    /// * `max_age` - Maximum age of an entry at worker processing time.
    ///
    /// # Returns
    /// Prior text, or none when history has no eligible nonempty entries.
    pub(crate) fn recent_context(&self, count: usize, max_age: Duration) -> Option<String> {
        self.recent_context_at(count, max_age, Instant::now())
    }

    fn recent_context_at(&self, count: usize, max_age: Duration, now: Instant) -> Option<String> {
        let inner = self.inner.lock();
        let mut entries: Vec<_> = inner
            .history
            .iter()
            .filter(|entry| now.saturating_duration_since(entry.at) <= max_age)
            .map(|entry| entry.text.trim())
            .filter(|text| !text.is_empty())
            .take(count)
            .collect();
        entries.reverse();
        let mut text = entries.join("\n");
        let start = text
            .char_indices()
            .rev()
            .nth(MAX_CONTEXT_CHARS - 1)
            .map_or(0, |(byte, _)| byte);
        text.drain(..start);
        (!text.is_empty()).then_some(text)
    }

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
    fn context_selects_newest_eligible_entries_in_chronological_order() {
        let state = SharedState::with_history_limit(5);
        for text in ["expired", "boundary", "older", " newest "] {
            state.remember_transcript(text.into());
        }
        let now = Instant::now();
        {
            let mut inner = state.inner.lock();
            inner.history[3].at = now - Duration::from_secs(601);
            inner.history[2].at = now - Duration::from_secs(600);
        }
        let age = Duration::from_secs(600);
        assert_eq!(
            state.recent_context_at(3, age, now).as_deref(),
            Some("boundary\nolder\nnewest")
        );
        assert_eq!(
            state.recent_context_at(2, age, now).as_deref(),
            Some("older\nnewest")
        );
        assert_eq!(state.recent_context_at(0, age, now), None);
        assert_eq!(
            state.recent_context_at(5, age, now).as_deref(),
            Some("boundary\nolder\nnewest")
        );
        for entry in &mut state.inner.lock().history {
            entry.at = now - Duration::from_secs(601);
        }
        assert_eq!(state.recent_context_at(3, age, now), None);
    }

    #[test]
    fn context_is_bounded_by_retained_history_and_unicode_characters() {
        let disabled = SharedState::with_history_limit(0);
        disabled.remember_transcript("discarded".into());
        assert_eq!(disabled.recent_context(3, Duration::from_secs(600)), None);
        let state = SharedState::with_history_limit(1);
        assert_eq!(state.recent_context(3, Duration::from_secs(600)), None);
        state.remember_transcript("evicted".into());
        let text = format!("{}終", "雪".repeat(1600));
        state.remember_transcript(text);
        let context = state.recent_context(3, Duration::from_secs(600)).unwrap();
        assert_eq!(context, format!("{}終", "雪".repeat(1499)));
        assert_eq!(context.chars().count(), MAX_CONTEXT_CHARS);
        assert!(SharedState::new()
            .recent_context(3, Duration::from_secs(600))
            .is_none());
    }

    #[test]
    fn context_budget_includes_separators_and_prefers_newer_text() {
        let state = SharedState::new();
        state.remember_transcript("a".repeat(1000));
        state.remember_transcript("b".repeat(1000));
        assert_eq!(
            state.recent_context(3, Duration::from_secs(600)),
            Some(format!("{}\n{}", "a".repeat(499), "b".repeat(1000)))
        );
    }

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
