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
//! specific evidence instead: an exact baseline-to-current insertion for a
//! short transcript, a newly visible whole transcript or leading/trailing
//! window once the text is long enough to be distinctive, or the exact
//! selected-range and total-length transition that inserting the transcript
//! must produce. Selection geometry is trusted only while polling the same
//! Accessibility object; a replacement object must show transcript-specific
//! text. If neither form of evidence is available, the transaction
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

use crate::daemon::desktop::clipboard_restore::{
    PasteConfirmation, PasteConfirmationContext, PasteTargetSelection, PasteTargetValue,
};
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

/// Maximum number of UTF-16 code units copied from `AXValue` per poll.
///
/// Values at or below the limit are copied whole. Larger values retain
/// equally sized leading and trailing segments, bounding allocation before
/// UTF-8 conversion while preserving the portions exposed by terminals and
/// bounded text fields most often.
const MAX_AX_VALUE_UTF16_UNITS: usize = 65_536;

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
/// Bounded leading/trailing portions of the element's current value, or
/// `None` when there is no focus snapshot, the element withholds its value
/// (secure input fields), or the read fails. `None` is not an error:
/// confirmation degrades to a post-chord baseline.
pub(crate) fn capture_baseline(focus: Option<&FocusSnapshot>) -> Option<PasteTargetValue> {
    focus
        .and_then(FocusSnapshot::macos_ax_element)
        .filter(|element| element.supports_value_polling())
        .and_then(|element| element.poll_value(MAX_AX_VALUE_UTF16_UNITS).ok().flatten())
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
/// observed to show transcript-specific insertion evidence (ignoring the
/// target's own line wrapping), or its selection and length show the exact
/// insertion transition. Short transcripts require an exact value delta
/// against the pre-chord baseline; longer text can use distinctive occurrence
/// windows.
/// [`PasteConfirmation::Unverified`] after a fixed grace
/// sleep when Accessibility cannot expose a pollable value at all (no
/// focused element on this snapshot, or the field does not support value
/// polling). [`PasteConfirmation::NoEvidence`] when polling itself worked
/// but no insertion evidence appeared before the deadline. A dead captured
/// Accessibility object is reacquired from the same frontmost application so
/// Safari/WebKit object replacement does not become a false failure; a real
/// focus change still ends evidence gathering.
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
    let baseline = match ctx.baseline {
        Some(baseline) => Some(BoundedNormalizedValue::from_target_value(baseline)),
        None => element
            .poll_value(MAX_AX_VALUE_UTF16_UNITS)
            .ok()
            .flatten()
            .as_ref()
            .map(BoundedNormalizedValue::from_target_value),
    };

    let deadline = start + AX_CONFIRM_DEADLINE;
    let mut captured_element_usable = true;
    loop {
        let now = Instant::now();
        if now >= deadline {
            if poll_current_focus(ctx, &matcher, baseline.as_ref()).unwrap_or(false) {
                return PasteConfirmation::Confirmed {
                    elapsed: start.elapsed(),
                    kind: "ax_confirmed",
                };
            }
            return PasteConfirmation::NoEvidence {
                elapsed: start.elapsed(),
                kind: "no_evidence",
            };
        }
        thread::sleep(AX_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));

        let poll = if captured_element_usable {
            element.poll_value(MAX_AX_VALUE_UTF16_UNITS)
        } else {
            match poll_current_focus(ctx, &matcher, baseline.as_ref()) {
                Ok(true) => {
                    return PasteConfirmation::Confirmed {
                        elapsed: start.elapsed(),
                        kind: "ax_confirmed",
                    };
                }
                Ok(false) => continue,
                Err(()) => {
                    return PasteConfirmation::NoEvidence {
                        elapsed: start.elapsed(),
                        kind: "no_evidence",
                    };
                }
            }
        };

        match poll {
            Ok(Some(current)) => {
                let current = BoundedNormalizedValue::from_target_value(&current);
                if matcher.indicates_insertion(baseline.as_ref(), &current, true) {
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
                // WebKit can replace an AX object as an editable field is
                // updated. Continue with fresh focused-element reads from the
                // captured application; a real focus change is rejected by
                // `poll_current_focus`.
                captured_element_usable = false;
            }
        }
    }
}

fn poll_current_focus(
    ctx: &PasteConfirmationContext<'_>,
    matcher: &TranscriptMatcher,
    baseline: Option<&BoundedNormalizedValue>,
) -> Result<bool, ()> {
    let Some(focus) = ctx.focus else {
        return Ok(false);
    };
    let Some((current, same_element)) = focus.macos_poll_current_value(MAX_AX_VALUE_UTF16_UNITS)?
    else {
        return Ok(false);
    };
    let current = BoundedNormalizedValue::from_target_value(&current);
    Ok(matcher.indicates_insertion(baseline, &current, same_element))
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
        baseline.map(|value| BoundedNormalizedValue::new(value, MAX_AX_VALUE_UTF16_UNITS));
    let current = BoundedNormalizedValue::new(current, MAX_AX_VALUE_UTF16_UNITS);
    matcher.indicates_insertion(baseline.as_ref(), &current, true)
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
    utf16_units: usize,
    selection: Option<PasteTargetSelection>,
}

impl BoundedNormalizedValue {
    fn from_target_value(value: &PasteTargetValue) -> Self {
        Self {
            head: normalize_segment(&value.head),
            tail: value.tail.as_deref().map(normalize_segment),
            utf16_units: value.utf16_units,
            selection: value.selection,
        }
    }

    fn new(value: &str, max_chars: usize) -> Self {
        debug_assert!(max_chars >= 2);
        let utf16_units = value.encode_utf16().count();
        let mut leading: Vec<char> = value
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(max_chars + 1)
            .collect();
        if leading.len() <= max_chars {
            return Self {
                head: leading.into_iter().collect(),
                tail: None,
                utf16_units,
                selection: None,
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
            utf16_units,
            selection: None,
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

    /// Return whether this complete value is exactly `baseline` with one
    /// normalized `inserted` substring added.
    ///
    /// This is the safe text-only acknowledgement for short transcripts:
    /// merely seeing another `"ok"` occurrence can collide with unrelated
    /// growth such as `"token"`, while removing the alleged insertion and
    /// recovering the exact baseline demonstrates the expected value delta.
    fn is_exact_insertion_of(&self, baseline: &Self, inserted: &str) -> bool {
        if inserted.is_empty() || self.tail.is_some() || baseline.tail.is_some() {
            return false;
        }
        if self.head.len() != baseline.head.len() + inserted.len() {
            return false;
        }

        self.head.match_indices(inserted).any(|(index, _)| {
            let suffix_index = index + inserted.len();
            baseline.head.get(..index).is_some_and(|prefix| {
                prefix == &self.head[..index]
                    && baseline
                        .head
                        .get(index..)
                        .is_some_and(|suffix| suffix == &self.head[suffix_index..])
            })
        })
    }
}

fn normalize_segment(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
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
/// A transcript at or below [`CONFIRM_WINDOW_CHARS`] requires an exact
/// baseline-to-current insertion instead of a substring count increase; short
/// strings collide too easily with unrelated target updates. For longer text,
/// a leading or trailing window still counts: a terminal scrolls the head of
/// a long paste off the top of the screen, and a bounded field truncates the
/// tail, but either distinctive end appearing verbatim is positive evidence
/// the paste landed.
///
struct TranscriptMatcher {
    whole: Option<String>,
    head: Option<String>,
    tail: Option<String>,
    utf16_units: usize,
}

impl TranscriptMatcher {
    fn new(transcript: &str) -> Self {
        let utf16_units = transcript.encode_utf16().count();
        let normalized = BoundedNormalizedValue::new(transcript, MAX_WHOLE_TRANSCRIPT_CHARS);
        if normalized.head.is_empty() {
            return Self {
                whole: None,
                head: None,
                tail: None,
                utf16_units,
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
                utf16_units,
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
            utf16_units,
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
        allow_selection_evidence: bool,
    ) -> bool {
        let short_exact_insertion = match (&self.whole, &self.head, &self.tail, baseline) {
            (Some(whole), None, None, Some(baseline)) => {
                current.is_exact_insertion_of(baseline, whole)
            }
            _ => false,
        };
        let distinctive_occurrence = match baseline {
            Some(baseline) if self.head.is_some() || self.tail.is_some() => {
                let current_counts = self.counts(current);
                let baseline_counts = self.counts(baseline);
                current_counts.whole > baseline_counts.whole
                    || current_counts.head > baseline_counts.head
                    || current_counts.tail > baseline_counts.tail
            }
            _ => false,
        };

        short_exact_insertion
            || distinctive_occurrence
            || (allow_selection_evidence
                && baseline
                    .is_some_and(|baseline| self.selection_indicates_insertion(baseline, current)))
    }

    fn selection_indicates_insertion(
        &self,
        baseline: &BoundedNormalizedValue,
        current: &BoundedNormalizedValue,
    ) -> bool {
        if self.utf16_units == 0 {
            return false;
        }
        let (Some(before), Some(after)) = (baseline.selection, current.selection) else {
            return false;
        };
        let Some(selection_end) = before.location.checked_add(before.length) else {
            return false;
        };
        if selection_end > baseline.utf16_units {
            return false;
        }
        let Some(expected_caret) = before.location.checked_add(self.utf16_units) else {
            return false;
        };
        let Some(expected_value_units) = baseline
            .utf16_units
            .checked_sub(before.length)
            .and_then(|units| units.checked_add(self.utf16_units))
        else {
            return false;
        };
        after
            == (PasteTargetSelection {
                location: expected_caret,
                length: 0,
            })
            && current.utf16_units == expected_value_units
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_transcript_requires_baseline_and_exact_delta() {
        assert!(!value_indicates_insertion(None, "hello world", "world"));
        assert!(value_indicates_insertion(
            Some("hello "),
            "hello world",
            "world"
        ));
    }

    #[test]
    fn short_transcript_substring_collision_does_not_confirm() {
        assert!(!value_indicates_insertion(
            Some("draft"),
            "draft token",
            "ok"
        ));
    }

    #[test]
    fn short_transcript_exact_insertion_confirms_at_any_position() {
        assert!(value_indicates_insertion(
            Some("draft token"),
            "draft token ok",
            "ok"
        ));
        assert!(value_indicates_insertion(Some("token"), "tokoken", "ok"));
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
    fn missing_baseline_does_not_confirm_a_preexisting_long_transcript() {
        let transcript = "alpha bravo charlie delta echo foxtrot golf hotel";
        assert!(transcript.chars().count() > CONFIRM_WINDOW_CHARS);
        assert!(!value_indicates_insertion(None, transcript, transcript));
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

    #[test]
    fn omitted_ax_middle_stays_separate_during_normalization() {
        let value = PasteTargetValue {
            head: "prefix alpha ".to_owned(),
            tail: Some(" omega suffix".to_owned()),
            utf16_units: 25,
            selection: None,
        };
        let normalized = BoundedNormalizedValue::from_target_value(&value);
        assert_eq!(normalized.head, "prefixalpha");
        assert_eq!(normalized.tail.as_deref(), Some("omegasuffix"));
        assert_eq!(normalized.occurrence_count("alphaomega"), 0);
    }

    #[test]
    fn exact_selection_geometry_confirms_reformatted_safari_text() {
        let transcript = "say \"hi\"";
        let matcher = TranscriptMatcher::new(transcript);
        let baseline = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "draft".to_owned(),
            tail: None,
            utf16_units: 5,
            selection: Some(PasteTargetSelection {
                location: 0,
                length: 5,
            }),
        });
        let current = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "say \u{201c}hi\u{201d}".to_owned(),
            tail: None,
            utf16_units: 8,
            selection: Some(PasteTargetSelection {
                location: 8,
                length: 0,
            }),
        });

        assert!(!current.head.contains(&normalize_segment(transcript)));
        assert!(matcher.indicates_insertion(Some(&baseline), &current, true));
    }

    #[test]
    fn replacement_ax_object_requires_text_evidence() {
        let matcher = TranscriptMatcher::new("say \"hi\"");
        let baseline = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "draft".to_owned(),
            tail: None,
            utf16_units: 5,
            selection: Some(PasteTargetSelection {
                location: 0,
                length: 5,
            }),
        });
        let current = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "say \u{201c}hi\u{201d}".to_owned(),
            tail: None,
            utf16_units: 8,
            selection: Some(PasteTargetSelection {
                location: 8,
                length: 0,
            }),
        });

        assert!(!matcher.indicates_insertion(Some(&baseline), &current, false));
    }

    #[test]
    fn selection_motion_without_expected_value_length_does_not_confirm() {
        let matcher = TranscriptMatcher::new("hello");
        let baseline = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "draft".to_owned(),
            tail: None,
            utf16_units: 5,
            selection: Some(PasteTargetSelection {
                location: 0,
                length: 5,
            }),
        });
        let current = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "unrelated".to_owned(),
            tail: None,
            utf16_units: 9,
            selection: Some(PasteTargetSelection {
                location: 5,
                length: 0,
            }),
        });

        assert!(!matcher.indicates_insertion(Some(&baseline), &current, true));
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
