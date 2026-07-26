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
//! ## Why value growth alone is not confirmation
//!
//! A focused element can grow for reasons unrelated to Parakit's paste:
//! asynchronous application output, autocomplete, a remote terminal update,
//! or another input source can all change `AXValue` during the confirmation
//! window. Treating any length increase as paste evidence can therefore
//! restore the previous clipboard even though the transcript never landed,
//! destroying the only remaining copy. Confirmation requires transcript-
//! specific evidence instead: a newly visible whole transcript, or a newly
//! visible leading/trailing window when a terminal or bounded field only
//! exposes part of it. If that evidence is unavailable, the transaction
//! deliberately leaves the transcript on the clipboard.
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

/// Longest normalized transcript retained for whole-transcript matching.
/// Longer transcripts retain only their leading/trailing evidence windows.
const MAX_WHOLE_TRANSCRIPT_CHARS: usize = 20_000;

/// Maximum number of non-whitespace `AXValue` characters retained per poll.
///
/// Values at or below the limit are matched whole. Larger values retain
/// equally sized leading and trailing segments, bounding both allocation and
/// matching work while preserving the portions exposed by terminals and
/// bounded text fields most often.
const MAX_NORMALIZED_AX_VALUE_CHARS: usize = 65_536;

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
/// Transcript-specific evidence is compared with
/// [`PasteConfirmationContext::baseline`], read before the chord was sent.
/// When no pre-chord read was possible this falls back to the first
/// post-chord read, which is strictly weaker — see the module docs.
///
/// # Returns
///
/// [`PasteConfirmation::Confirmed`] as soon as the element's value is
/// observed to show a newly visible transcript occurrence (ignoring the
/// target's own line wrapping).
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

    // Build transcript evidence once for the whole polling transaction. The
    // old implementation rebuilt this whitespace-normalized needle on every
    // 40ms tick.
    let matcher = TranscriptMatcher::new(ctx.transcript);
    let baseline =
        match ctx.baseline {
            Some(baseline) => Some(BoundedNormalizedValue::new(
                baseline,
                MAX_NORMALIZED_AX_VALUE_CHARS,
            )),
            None => element.poll_value().ok().flatten().map(|baseline| {
                BoundedNormalizedValue::new(&baseline, MAX_NORMALIZED_AX_VALUE_CHARS)
            }),
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
                let current = BoundedNormalizedValue::new(&current, MAX_NORMALIZED_AX_VALUE_CHARS);
                if matcher.indicates_insertion(baseline.as_ref(), &current) {
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

/// Pure decision logic: does `current` (a freshly read `AXValue`) contain
/// newly visible transcript-specific evidence relative to `baseline`?
///
/// Factored out of the FFI polling loop so it can be unit-tested without an
/// Accessibility permission grant; the FFI wrapper in
/// [`await_paste_confirmation`] stays thin.
///
/// # Arguments
///
/// * `baseline` - Pre-chord `AXValue` read, when available.
/// * `current` - Most recent `AXValue` read.
/// * `transcript` - Transcript text that was pasted.
#[cfg(test)]
fn value_indicates_insertion(baseline: Option<&str>, current: &str, transcript: &str) -> bool {
    let matcher = TranscriptMatcher::new(transcript);
    let baseline =
        baseline.map(|value| BoundedNormalizedValue::new(value, MAX_NORMALIZED_AX_VALUE_CHARS));
    let current = BoundedNormalizedValue::new(current, MAX_NORMALIZED_AX_VALUE_CHARS);
    matcher.indicates_insertion(baseline.as_ref(), &current)
}

/// Bounded whitespace-normalized representation of an Accessibility value.
///
/// Small values are kept whole in `head`. For a value over the requested
/// limit, `head` and `tail` hold disjoint leading/trailing halves. They stay
/// separate so concatenating them cannot manufacture a false match across
/// an omitted middle.
struct BoundedNormalizedValue {
    head: String,
    tail: Option<String>,
}

impl BoundedNormalizedValue {
    fn new(value: &str, max_chars: usize) -> Self {
        debug_assert!(max_chars >= 2);
        let mut leading: Vec<char> = value
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(max_chars + 1)
            .collect();
        if leading.len() <= max_chars {
            return Self {
                head: leading.into_iter().collect(),
                tail: None,
            };
        }

        let head_chars = max_chars / 2;
        leading.truncate(head_chars);
        let mut trailing: Vec<char> = value
            .chars()
            .rev()
            .filter(|c| !c.is_whitespace())
            .take(max_chars - head_chars)
            .collect();
        trailing.reverse();
        Self {
            head: leading.into_iter().collect(),
            tail: Some(trailing.into_iter().collect()),
        }
    }

    fn occurrence_count(&self, needle: &str) -> usize {
        if needle.is_empty() {
            return 0;
        }
        self.head.match_indices(needle).count()
            + self
                .tail
                .as_deref()
                .map_or(0, |tail| tail.match_indices(needle).count())
    }
}

#[derive(Default)]
struct EvidenceCounts {
    whole: usize,
    head: usize,
    tail: usize,
}

/// Transcript-specific evidence compiled once before the poll loop.
///
/// Matching ignores whitespace entirely rather than comparing verbatim. A
/// terminal's `AXValue` is its *rendered* screen, hard-wrapped at the column
/// width, so a pasted transcript comes back with newlines injected at the
/// wrap points. Dropping whitespace on both sides makes the comparison
/// independent of that layout.
///
/// When the whole transcript is not found, a leading or trailing window of
/// [`CONFIRM_WINDOW_CHARS`] still counts: a terminal scrolls the head of a
/// long paste off the top of the screen, and a bounded field truncates the
/// tail, but either end appearing verbatim is positive evidence the paste
/// landed.
///
struct TranscriptMatcher {
    whole: Option<String>,
    head: Option<String>,
    tail: Option<String>,
}

impl TranscriptMatcher {
    fn new(transcript: &str) -> Self {
        let normalized = BoundedNormalizedValue::new(transcript, MAX_WHOLE_TRANSCRIPT_CHARS);
        if normalized.head.is_empty() {
            return Self {
                whole: None,
                head: None,
                tail: None,
            };
        }

        let whole = normalized.tail.is_none().then(|| normalized.head.clone());
        let normalized_len = normalized.head.chars().count()
            + normalized
                .tail
                .as_deref()
                .map_or(0, |tail| tail.chars().count());
        if normalized_len <= CONFIRM_WINDOW_CHARS {
            return Self {
                whole,
                head: None,
                tail: None,
            };
        }

        let head: String = normalized.head.chars().take(CONFIRM_WINDOW_CHARS).collect();
        let tail_source = normalized.tail.as_deref().unwrap_or(&normalized.head);
        let mut tail: Vec<char> = tail_source
            .chars()
            .rev()
            .take(CONFIRM_WINDOW_CHARS)
            .collect();
        tail.reverse();
        Self {
            whole,
            head: Some(head),
            tail: Some(tail.into_iter().collect()),
        }
    }

    fn counts(&self, value: &BoundedNormalizedValue) -> EvidenceCounts {
        EvidenceCounts {
            whole: self
                .whole
                .as_deref()
                .map_or(0, |whole| value.occurrence_count(whole)),
            head: self
                .head
                .as_deref()
                .map_or(0, |head| value.occurrence_count(head)),
            tail: self
                .tail
                .as_deref()
                .map_or(0, |tail| value.occurrence_count(tail)),
        }
    }

    fn indicates_insertion(
        &self,
        baseline: Option<&BoundedNormalizedValue>,
        current: &BoundedNormalizedValue,
    ) -> bool {
        let current = self.counts(current);
        let baseline = baseline.map(|value| self.counts(value)).unwrap_or_default();
        current.whole > baseline.whole
            || current.head > baseline.head
            || current.tail > baseline.tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_transcript_evidence_confirms_with_or_without_baseline() {
        assert!(value_indicates_insertion(None, "hello world", "world"));
        assert!(value_indicates_insertion(
            Some("hello "),
            "hello world",
            "world"
        ));
    }

    #[test]
    fn unrelated_growth_does_not_confirm() {
        assert!(!value_indicates_insertion(Some("abc"), "abcdef", "xyz"));
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
    fn oversized_transcript_uses_bounded_windows_not_growth() {
        let huge = format!(
            "{}middle{}",
            "a".repeat(MAX_WHOLE_TRANSCRIPT_CHARS),
            "z".repeat(CONFIRM_WINDOW_CHARS)
        );
        assert!(value_indicates_insertion(Some(""), &huge, &huge));
        assert!(!value_indicates_insertion(
            Some("short"),
            "short-plus-more",
            &huge
        ));
        assert!(!value_indicates_insertion(Some("short"), "short", &huge));
    }

    #[test]
    fn empty_transcript_never_confirms() {
        assert!(!value_indicates_insertion(Some(""), "", ""));
        assert!(!value_indicates_insertion(Some(""), "x", ""));
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
        let same_length_baseline = "x".repeat(wrapped.len());
        assert!(value_indicates_insertion(
            Some(&same_length_baseline),
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

    #[test]
    fn evidence_already_present_in_baseline_does_not_confirm() {
        let transcript = "alpha bravo charlie delta echo foxtrot";
        assert!(!value_indicates_insertion(
            Some(transcript),
            transcript,
            transcript
        ));
    }

    #[test]
    fn an_additional_transcript_occurrence_confirms() {
        let transcript = "alpha bravo charlie delta echo foxtrot";
        let current = format!("{transcript}\n{transcript}");
        assert!(value_indicates_insertion(
            Some(transcript),
            &current,
            transcript
        ));
    }

    #[test]
    fn normalized_ax_values_are_bounded_and_keep_both_ends() {
        let value = format!("{}{}", "a".repeat(80), "z".repeat(80));
        let normalized = BoundedNormalizedValue::new(&value, 64);
        assert_eq!(normalized.head, "a".repeat(32));
        assert_eq!(normalized.tail.as_deref(), Some("z".repeat(32).as_str()));
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
