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
//!
//! ## Reading the baseline before the chord
//!
//! [`capture_baseline`] is called on the paste path *before* the chord is
//! posted, and its result reaches [`await_paste_confirmation`] as
//! [`PasteConfirmationContext::baseline`].
//!
//! This matters more than it looks. An earlier version took its baseline
//! from the first *post*-chord read, which races the target: an app that
//! refreshes its accessibility tree coarsely (terminals especially — ghostty
//! confirms in ~300ms where Safari and Discord confirm in ~40ms) can already
//! have the pasted text in that first read. From then on the value never
//! grows, so a paste that landed perfectly is indistinguishable from one
//! that was dropped, and the transaction reports
//! [`PasteConfirmation::NoEvidence`] — an error chime and a withheld
//! clipboard restore on a completely successful dictation. Whether that
//! happened came down to timing, which made it look intermittent and
//! arbitrary from the outside.
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

/// Number of leading (and trailing) non-whitespace characters matched when
/// the whole transcript cannot be found in the target's value.
///
/// Long enough that a natural-language run of this many characters is
/// effectively unique against whatever the field held beforehand, so a match
/// is real evidence rather than coincidence.
const CONFIRM_WINDOW_CHARS: usize = 32;

/// Read the focused element's `AXValue` before a paste chord is sent.
///
/// Wired into the paste transaction through
/// [`ClipboardRestoreGate::capture_paste_baseline`](crate::daemon::desktop::clipboard_restore::ClipboardRestoreGate::capture_paste_baseline);
/// see the module docs for why the baseline must predate the chord.
///
/// # Arguments
///
/// * `focus` - Focus snapshot the chord is about to target.
///
/// # Returns
///
/// The element's current value, or `None` when there is no focus snapshot,
/// the element withholds its value (secure input fields), or the read fails.
/// `None` is not an error: confirmation degrades to a post-chord baseline.
pub(crate) fn capture_baseline(focus: Option<&FocusSnapshot>) -> Option<String> {
    focus
        .and_then(FocusSnapshot::macos_ax_element)
        .filter(|element| element.supports_value_polling())
        .and_then(|element| element.poll_value().ok().flatten())
}

/// Await confirmation that a just-sent paste chord was consumed by the
/// focused Accessibility element, by polling `AXValue`.
///
/// Runs on the calling (worker) thread with a plain sleep loop; no run loop
/// pump is required (see the module docs for why this does not use
/// `AXObserver` push notifications).
///
/// Growth is measured against [`PasteConfirmationContext::baseline`], read
/// before the chord was sent. When no pre-chord read was possible this falls
/// back to the first post-chord read, which is strictly weaker — see the
/// module docs.
///
/// # Returns
///
/// [`PasteConfirmation::Confirmed`] as soon as the element's value is
/// observed to show the transcript (ignoring the target's own line
/// wrapping), or to have grown past the baseline.
/// [`PasteConfirmation::Unverified`] after a fixed grace
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

    let baseline: Option<String> = match ctx.baseline {
        Some(baseline) => Some(baseline.to_owned()),
        None => element.poll_value().ok().flatten(),
    };

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
    if transcript_is_present(current, transcript) {
        return true;
    }
    match baseline {
        Some(baseline) => current.len() > baseline.len(),
        None => false,
    }
}

/// Is `transcript` visible in `current`, allowing for the target having
/// re-laid-out the text?
///
/// Matching ignores whitespace entirely rather than comparing verbatim. A
/// terminal's `AXValue` is its *rendered* screen, hard-wrapped at the column
/// width, so a pasted transcript comes back with newlines injected at the
/// wrap points — and terminals wrap mid-word, so collapsing runs of
/// whitespace is not enough to repair it. Dropping whitespace on both sides
/// makes the comparison independent of how the target chose to lay the text
/// out.
///
/// When the whole transcript is not found, a leading or trailing window of
/// [`CONFIRM_WINDOW_CHARS`] still counts: a terminal scrolls the head of a
/// long paste off the top of the screen, and a bounded field truncates the
/// tail, but either end appearing verbatim is positive evidence the paste
/// landed.
///
/// # Arguments
///
/// * `current` - Most recent `AXValue` read.
/// * `transcript` - Transcript text that was pasted.
fn transcript_is_present(current: &str, transcript: &str) -> bool {
    if transcript.is_empty() || transcript.len() > MAX_CONTAINS_TRANSCRIPT_LEN {
        return false;
    }
    let needle: Vec<char> = transcript.chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return false;
    }
    let haystack: String = current.chars().filter(|c| !c.is_whitespace()).collect();

    let whole: String = needle.iter().collect();
    if haystack.contains(&whole) {
        return true;
    }
    if needle.len() <= CONFIRM_WINDOW_CHARS {
        // Already covered by the whole-transcript check above, and too short
        // to window down further without inviting coincidental matches.
        return false;
    }
    let head: String = needle[..CONFIRM_WINDOW_CHARS].iter().collect();
    let tail: String = needle[needle.len() - CONFIRM_WINDOW_CHARS..]
        .iter()
        .collect();
    haystack.contains(&head) || haystack.contains(&tail)
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

    /// The regression behind the false error chime in ghostty: a terminal
    /// reports its *rendered* screen, hard-wrapped at the column width, and
    /// it wraps mid-word. A verbatim `contains` misses that entirely.
    #[test]
    fn hard_wrapped_transcript_still_confirms() {
        let transcript = "Wait, what? How is the new rule set from the dictation not in scope?";
        let wrapped =
            "prompt> Wait, what? How is the new rule set from the dic\ntation not in scope?";
        assert!(
            !wrapped.contains(transcript),
            "precondition: verbatim fails"
        );
        assert!(value_indicates_insertion(
            Some("prompt> "),
            wrapped,
            transcript
        ));
    }

    /// Growth is unavailable when the target re-rendered to the same length
    /// (a fixed-size terminal grid). Layout-independent matching has to
    /// carry the confirmation on its own.
    #[test]
    fn wrapped_transcript_confirms_without_any_growth_evidence() {
        let transcript = "the quick brown fox jumps over the lazy dog every single morning";
        let wrapped = "the quick brown fox jumps over the\nlazy dog every single morning";
        assert!(value_indicates_insertion(
            Some(wrapped),
            wrapped,
            transcript
        ));
    }

    /// A terminal scrolls the head of a long paste off the top of the
    /// screen; a bounded field truncates the tail. Either surviving end is
    /// still evidence the paste landed.
    #[test]
    fn partially_visible_transcript_confirms_from_either_end() {
        let transcript = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo";
        let head_only = &transcript[..48];
        let tail_only = &transcript[18..];
        assert!(value_indicates_insertion(Some(""), head_only, transcript));
        assert!(value_indicates_insertion(Some(""), tail_only, transcript));
    }

    /// Windowed matching must not fire on an unrelated field that merely
    /// happens to hold text.
    #[test]
    fn unrelated_value_does_not_confirm_via_windowing() {
        let transcript = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo";
        assert!(!value_indicates_insertion(
            Some("some unrelated field contents"),
            "some unrelated field contents",
            transcript
        ));
    }

    /// A transcript at or under [`CONFIRM_WINDOW_CHARS`] gets no windowed
    /// fallback: a partial overlap must not be mistaken for insertion when
    /// the whole transcript is short enough to have been matched outright.
    #[test]
    fn short_transcript_has_no_windowed_fallback() {
        let transcript = "hello there friend";
        assert!(transcript.chars().count() <= CONFIRM_WINDOW_CHARS);
        assert!(!value_indicates_insertion(
            Some("hello there"),
            "hello there",
            transcript
        ));
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
            baseline: None,
        };
        match await_paste_confirmation(&ctx) {
            PasteConfirmation::Unverified { kind, .. } => {
                assert_eq!(kind, "unverified_timeout");
            }
            other => panic!("expected Unverified, got {other:?}"),
        }
    }

    /// `capture_baseline` runs on the paste hot path in a process that may
    /// hold no Accessibility grant; with no focus snapshot it must resolve
    /// to `None` without touching a live Accessibility API.
    #[test]
    fn capture_baseline_without_focus_is_none() {
        assert!(capture_baseline(None).is_none());
    }
}
