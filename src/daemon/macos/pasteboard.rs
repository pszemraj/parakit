//! macOS post-paste acknowledgement: poll the focused Accessibility
//! element's `AXValue` to confirm a transcript was actually consumed by the
//! paste chord before the previous clipboard contents are restored.
//!
//! Before this module existed, macOS paste always slept a fixed 200ms and
//! then restored whatever clipboard content preceded the transcript,
//! regardless of whether the target had consumed the paste yet. On a slow
//! or busy target that blind restore could destroy the just-dictated
//! transcript. This module turns that guess into a bounded, evidence-based
//! wait: uncertainty degrades to [`PasteConfirmation::Unverified`] or
//! [`PasteConfirmation::NoEvidence`], it never silently destroys the
//! transcript.
//!
//! ## Why polling on the worker thread, not `AXObserver` push notifications
//!
//! This intentionally polls with a plain sleep loop on the calling (worker)
//! thread rather than registering an `AXObserver` for
//! `kAXValueChangedNotification`. `AXObserver` delivery requires a
//! `CFRunLoop` pumped on the observing thread; that is a reasonable
//! follow-up (a bounded `CFRunLoopRunInMode` per tick would fit neatly where
//! this module's sleep currently sits) but is not required to make paste
//! acknowledgement honest, so it is deferred.
//!
//! ## Deferred follow-ups
//!
//! - **NSPasteboard / Carbon promise-keeper read evidence.** macOS can
//!   report when a pasteboard's contents were actually *read* by a
//!   consumer. That evidence would let confirmation resolve the moment the
//!   target reads the clipboard rather than waiting on `AXValue` polling,
//!   shortening the [`UNVERIFIED_GRACE`] path, and would populate
//!   `PasteReport`/`InsertionLogFields`'s `pasteboard_requested` field
//!   (always `None` today).
//! - **`AXObserver` push notifications.** Registering for
//!   `kAXValueChangedNotification` and pumping a bounded
//!   `CFRunLoopRunInMode` on the worker thread would shorten confirmation
//!   latency versus fixed-interval polling.
//! - **Pre-chord baseline.** [`await_paste_confirmation`] uses the first
//!   post-chord `AXValue` read as its baseline (see the function doc for
//!   why); capturing a genuine pre-chord baseline would need a channel back
//!   from the focus-recheck closure in
//!   `paste_with_clipboard_swap_guarded`'s `before_chord` callback, which is
//!   a plain `FnMut() -> Result<bool>` today.
//!
//! ## Secure input fields are not a bug
//!
//! Password/secure-text fields deliberately withhold `AXValue` from
//! Accessibility clients as a macOS privacy boundary (this is part of what
//! backs "Secure Input" and keeps the Accessibility API from doubling as a
//! keylogger). [`AxElementSnapshot::supports_value_polling`] being `false`
//! for such a field is expected, not a defect: [`PasteConfirmation::Unverified`]
//! is the correct degradation there. The paste chord already fired, so
//! Parakit cannot tell whether the transcript landed, but it also must not
//! guess by destroying the transcript.

use std::thread;
use std::time::{Duration, Instant};

use crate::daemon::desktop::clipboard_restore::{PasteConfirmation, PasteConfirmationContext};
use crate::daemon::desktop::inject::FocusSnapshot;

/// Deadline for `AXValue` confirmation polling after a paste chord is sent.
pub(crate) const AX_CONFIRM_DEADLINE: Duration = Duration::from_millis(1800);
/// Interval between `AXValue` polls while awaiting confirmation.
pub(crate) const AX_POLL_INTERVAL: Duration = Duration::from_millis(40);
/// Grace sleep applied when Accessibility cannot expose a pollable `AXValue`
/// at all (no focused element captured, or the field withholds its value —
/// see the secure-input-field note in the module docs).
pub(crate) const UNVERIFIED_GRACE: Duration = Duration::from_millis(1500);

/// Longest transcript length for which a freshly read `AXValue` is compared
/// with `.contains(transcript)`. Longer transcripts fall back to
/// length-growth evidence only, avoiding an `O(field_len * transcript_len)`
/// scan against a potentially huge accessibility value on every poll tick.
const MAX_CONTAINS_TRANSCRIPT_LEN: usize = 20_000;

/// Await confirmation that a just-sent paste chord was consumed by the
/// focused Accessibility element, by polling `AXValue`.
///
/// Runs on the calling (worker) thread with a plain sleep loop; no run loop
/// pump is required (see the module docs for why this does not use
/// `AXObserver` push notifications).
///
/// The first post-chord `AXValue` read is used as the growth baseline
/// (rather than a pre-chord read) because plumbing a pre-chord snapshot
/// through today's `before_chord` recheck closure would require a broader
/// signature change than this fix needs; see the module docs' deferred
/// follow-ups. In practice this is a reasonable baseline: synthetic paste
/// delivery and the target's own event-loop dispatch take at least a few
/// milliseconds, so this first read almost always lands before the target
/// has processed the paste.
///
/// # Returns
///
/// [`PasteConfirmation::Confirmed`] as soon as the element's value is
/// observed to contain the transcript, or to have grown since the first
/// post-chord read. [`PasteConfirmation::Unverified`] after a fixed grace
/// sleep when Accessibility cannot expose a pollable value at all (no
/// focused element on this snapshot, or the field does not support value
/// polling). [`PasteConfirmation::NoEvidence`] when polling itself worked
/// but no insertion evidence appeared before the deadline, including when
/// the focused element died or focus moved mid-poll (no more evidence can
/// be gathered at that point, so polling stops early rather than waiting
/// out the rest of the deadline).
pub(crate) fn await_paste_confirmation(ctx: &PasteConfirmationContext<'_>) -> PasteConfirmation {
    let start = Instant::now();

    let element = ctx.focus.and_then(FocusSnapshot::macos_ax_element);
    let Some(element) = element.filter(|element| element.supports_value_polling()) else {
        thread::sleep(UNVERIFIED_GRACE);
        return PasteConfirmation::Unverified {
            elapsed: start.elapsed(),
            kind: "unverified_timeout",
        };
    };

    let baseline = element.poll_value().ok().flatten();

    let deadline = start + AX_CONFIRM_DEADLINE;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return PasteConfirmation::NoEvidence {
                elapsed: start.elapsed(),
                kind: "no_evidence",
            };
        }
        thread::sleep(AX_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));

        match element.poll_value() {
            Ok(Some(current)) => {
                if value_indicates_insertion(baseline.as_deref(), &current, ctx.transcript) {
                    return PasteConfirmation::Confirmed {
                        elapsed: start.elapsed(),
                        kind: "ax_confirmed",
                    };
                }
            }
            Ok(None) => {
                // Value is not (or no longer) a string; no new evidence this
                // tick. Keep polling until the deadline.
            }
            Err(()) => {
                // Element died or focus moved: no more evidence to gather.
                return PasteConfirmation::NoEvidence {
                    elapsed: start.elapsed(),
                    kind: "no_evidence",
                };
            }
        }
    }
}

/// Pure decision logic: does `current` (a freshly read `AXValue`) look like
/// it now contains the inserted transcript, relative to `baseline` (the
/// first post-chord read, when one was available)?
///
/// Factored out of the FFI polling loop so it can be unit-tested without an
/// Accessibility permission grant; the FFI wrapper in
/// [`await_paste_confirmation`] stays thin.
///
/// # Arguments
///
/// * `baseline` - First post-chord `AXValue` read, when available.
/// * `current` - Most recent `AXValue` read.
/// * `transcript` - Transcript text that was pasted.
fn value_indicates_insertion(baseline: Option<&str>, current: &str, transcript: &str) -> bool {
    if !transcript.is_empty()
        && transcript.len() <= MAX_CONTAINS_TRANSCRIPT_LEN
        && current.contains(transcript)
    {
        return true;
    }
    match baseline {
        Some(baseline) => current.len() > baseline.len(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_transcript_confirms_regardless_of_baseline() {
        assert!(value_indicates_insertion(None, "hello world", "world"));
        assert!(value_indicates_insertion(
            Some("hello "),
            "hello world",
            "world"
        ));
    }

    #[test]
    fn growth_confirms_when_transcript_not_found_verbatim() {
        assert!(value_indicates_insertion(Some("abc"), "abcdef", "xyz"));
    }

    #[test]
    fn no_change_does_not_confirm() {
        assert!(!value_indicates_insertion(Some("abc"), "abc", "xyz"));
    }

    #[test]
    fn missing_baseline_without_transcript_match_does_not_confirm() {
        assert!(!value_indicates_insertion(None, "some field text", "xyz"));
    }

    #[test]
    fn missing_baseline_with_transcript_match_confirms() {
        assert!(value_indicates_insertion(None, "hello world", "hello"));
    }

    #[test]
    fn oversized_transcript_falls_back_to_growth_only() {
        let huge = "a".repeat(MAX_CONTAINS_TRANSCRIPT_LEN + 1);
        assert!(value_indicates_insertion(
            Some("short"),
            "short-plus-more",
            &huge
        ));
        assert!(!value_indicates_insertion(Some("short"), "short", &huge));
    }

    #[test]
    fn empty_transcript_relies_on_growth_only() {
        assert!(!value_indicates_insertion(Some(""), "", ""));
        assert!(value_indicates_insertion(Some(""), "x", ""));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod ffi_tests {
    use super::*;

    /// `await_paste_confirmation` must degrade to `Unverified` rather than
    /// touching any live Accessibility API when the context has no focus
    /// snapshot at all (the common case for a unit test process, which
    /// holds no Accessibility permission).
    #[test]
    fn no_focus_snapshot_is_unverified_without_ax_permission() {
        let ctx = PasteConfirmationContext {
            focus: None,
            transcript: "hello",
        };
        match await_paste_confirmation(&ctx) {
            PasteConfirmation::Unverified { kind, .. } => {
                assert_eq!(kind, "unverified_timeout");
            }
            other => panic!("expected Unverified, got {other:?}"),
        }
    }
}
