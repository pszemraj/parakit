//! macOS post-paste acknowledgement: poll the focused Accessibility
//! element's `AXValue` to confirm a transcript was actually consumed by the
//! paste chord before the previous clipboard contents are restored.
//!
//! Before this module existed, macOS paste always slept a fixed 200ms and
//! then restored whatever clipboard content preceded the transcript,
//! regardless of whether the target had consumed the paste yet. On a slow
//! or busy target that blind restore could destroy the just-dictated
//! transcript. This module turns that guess into a bounded, evidence-based
//! wait: uncertainty degrades to [`PasteConfirmation::Unverified`],
//! [`PasteConfirmation::UnverifiedFocusLost`], or
//! [`PasteConfirmation::NoEvidence`] — it never silently destroys the
//! transcript. `Unverified` still restores the previous clipboard contents,
//! since the target (or the absence of one) stayed the same throughout, just
//! inconclusive; `UnverifiedFocusLost` and `NoEvidence` both keep the
//! transcript on the clipboard instead, for different reasons — the target
//! became unobservable partway through, or evidence gathering worked the
//! whole window and simply never found anything.
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
//! specific evidence instead: an exact baseline-to-current insertion for the
//! very shortest transcripts, a whole-transcript occurrence-count increase
//! accompanied by target-value growth once the text is long enough to be
//! unlikely by coincidence, a newly visible whole transcript or leading/
//! trailing window with the same growth constraint once the text is long
//! enough to be distinctive on its own, or the exact selected-range and
//! total-length transition that inserting the transcript must produce.
//! Selection geometry is trusted only while polling the same Accessibility
//! object and only for transcripts long enough to be distinctive; a
//! replacement object must show transcript-specific text. If none of these
//! forms of evidence is available, the transaction keeps longer transcripts
//! on the clipboard. Very short transcripts instead resolve as unverified
//! after the grace window, avoiding a false manual-paste prompt in churning
//! targets where exact evidence is intentionally unavailable.
//!
//! ## Secure input fields are not a bug
//!
//! Password/secure-text fields deliberately withhold `AXValue` from
//! Accessibility clients as a macOS privacy boundary (this is part of what
//! backs "Secure Input" and keeps the Accessibility API from doubling as a
//! keylogger). A baseline or confirmation poll returning no string value for
//! such a field is expected, not a defect: [`PasteConfirmation::Unverified`]
//! is the correct degradation there. The paste chord already fired, so
//! Parakit cannot tell whether the transcript landed.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::daemon::desktop::clipboard_restore::{
    PasteConfirmation, PasteConfirmationContext, PasteTargetSelection, PasteTargetValue,
};
use crate::daemon::desktop::inject::{FocusSnapshot, PasteMode};

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

/// Minimum normalized (whitespace-stripped) transcript length for which a
/// whole-transcript occurrence-count *increase* over the pre-chord baseline
/// counts as confirmation evidence on its own, for a transcript at or under
/// [`CONFIRM_WINDOW_CHARS`] that gets no leading/trailing window (see
/// [`TranscriptMatcher`]).
///
/// Below this floor, a string is short enough that it could turn up in a
/// target's value by pure coincidence — a terminal's `AXValue` is its whole
/// rendered screen, so scrollback or unrelated output can easily contain a
/// short run of characters — so only an exact baseline-to-current insertion
/// is trusted there; occurrence-count and selection-geometry evidence are
/// not enough. At or above this floor the same reasoning that
/// justifies occurrence-count matching for longer transcripts already
/// applies: a natural-language run this long is unlikely to appear by
/// chance, and requiring an *increase* rather than mere presence (see
/// `evidence_already_present_in_baseline_does_not_confirm`) still rules out
/// a transcript that merely already existed in the target before the chord.
const SHORT_CONFIRM_MIN_OCCURRENCE_CHARS: usize = 12;

thread_local! {
    /// Per-thread abandonment signal consulted by [`await_paste_confirmation`].
    ///
    /// `None` on every thread that never calls [`install_abandonment_signal`]
    /// — in particular the production daemon's worker thread, which never
    /// installs one — so this is completely inert outside the one place that
    /// uses it (see that function's doc for why it exists).
    static ABANDONMENT_SIGNAL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Install a per-thread abandonment signal for [`await_paste_confirmation`]
/// to consult before letting a confirmation result reach the caller.
///
/// This exists solely for `doctor --deep`'s real paste-transaction smoke
/// test (see `daemon::macos::diagnostics::run_paste_transaction`), which
/// spawns a short-lived worker thread per attempt and enforces its own
/// `PASTE_TRANSACTION_TIMEOUT` on the main thread. If that timeout fires
/// while the worker thread is still blocked somewhere inside
/// [`await_paste_confirmation`] (a stuck-then-recovering AX call, most
/// likely), the harness's `JoinHandle` is dropped without joining and the
/// harness immediately starts its own clipboard cleanup — while the
/// abandoned worker thread is still running and, left unchecked, would
/// independently decide to restore or clear the same real macOS clipboard
/// once it finally returns. Two unilateral owners of one OS clipboard is the
/// race. The doctor harness calls this at the top of the worker closure,
/// before starting the paste attempt, and flips the flag from the main
/// thread the instant its timeout fires, *before* its own cleanup runs; see
/// the call sites inside [`await_paste_confirmation`] for where this is
/// consulted and the residual race window that leaves.
///
/// There is no matching uninstall: the thread this is called on is single-
/// use (one paste attempt, then the thread exits), so the thread-local just
/// drops with it.
pub(crate) fn install_abandonment_signal(flag: Arc<AtomicBool>) {
    ABANDONMENT_SIGNAL.with(|cell| *cell.borrow_mut() = Some(flag));
}

#[cfg(test)]
fn clear_abandonment_signal() {
    ABANDONMENT_SIGNAL.with(|cell| *cell.borrow_mut() = None);
}

/// Whether the current thread's installed abandonment signal, if any, has
/// been set.
fn is_abandoned() -> bool {
    ABANDONMENT_SIGNAL.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    })
}

/// Return a paste confirmation unless this thread's abandonment signal (see
/// [`install_abandonment_signal`]) is set, in which case it degrades to
/// [`PasteConfirmation::NoEvidence`] instead.
///
/// Every call site is a point where [`await_paste_confirmation`] is about to
/// resume from a potentially long blocking AX or pasteboard read and hand
/// back a result that would make its caller (`finish_confirmed_paste` in
/// `daemon::desktop::inject`) restore the clipboard. `NoEvidence` is the one
/// outcome that caller leaves alone, which is exactly what an abandoned
/// worker should do once the doctor harness has taken over cleanup.
///
/// This is a check-then-act race with the harness's timeout path, not a
/// full fix: the signal could still flip immediately after this check
/// returns `false`, in the instant before the caller acts on `Confirmed`.
/// That shrinks the exposure from the whole multi-second
/// `PASTE_TRANSACTION_TIMEOUT` window down to a handful of instructions —
/// microseconds, not seconds. Closing it completely would need a clipboard
/// mutex shared with the production paste path, which is out of proportion
/// for a diagnostics-only harness; the residual window is accepted instead.
fn unless_abandoned(confirmation: PasteConfirmation) -> PasteConfirmation {
    if is_abandoned() {
        let elapsed = match confirmation {
            PasteConfirmation::Confirmed { elapsed, .. }
            | PasteConfirmation::Unverified { elapsed, .. }
            | PasteConfirmation::UnverifiedFocusLost { elapsed, .. }
            | PasteConfirmation::NoEvidence { elapsed, .. } => elapsed,
        };
        return PasteConfirmation::NoEvidence {
            elapsed,
            kind: "no_evidence",
        };
    }
    confirmation
}

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
/// insertion transition. The very shortest transcripts require an exact
/// value delta against the pre-chord baseline; once long enough to be
/// unlikely by coincidence, a whole-transcript occurrence-count increase
/// counts too; longer text still can use distinctive leading/trailing
/// occurrence windows (see [`TranscriptMatcher`] for the exact length
/// bands).
/// [`PasteConfirmation::Unverified`] after a fixed grace
/// sleep when Accessibility cannot expose a pollable value at all (no
/// focused element on this snapshot, or the field does not support value
/// polling), and also when no baseline could be captured at all — neither
/// the pre-chord read nor the immediate post-chord fallback succeeded. Every
/// evidence form in [`TranscriptMatcher::indicates_insertion`] requires a
/// baseline to compare against, so polling without one could never confirm
/// anything and would otherwise spin to the deadline reporting a guaranteed
/// false [`PasteConfirmation::NoEvidence`] alarm. Transcripts below
/// [`SHORT_CONFIRM_MIN_OCCURRENCE_CHARS`] also resolve as `Unverified` after
/// the full confirmation window when their required exact-delta evidence is
/// defeated by unrelated target churn.
/// [`PasteConfirmation::UnverifiedFocusLost`] when the originally captured
/// Accessibility object dies and the reacquire fallback then loses the
/// ability to read focus entirely — the frontmost application changed, or
/// its focus state can no longer be read — before any evidence appeared: the
/// chord was posted into a verified-focused target and very likely landed,
/// but with focus no longer observable at all there is no way to keep
/// gathering evidence.
/// [`PasteConfirmation::NoEvidence`] when polling worked the whole window but
/// no insertion evidence ever appeared before the deadline. A dead captured
/// Accessibility object is otherwise reacquired from the same frontmost
/// application so ordinary Safari/WebKit object replacement does not become
/// a false failure.
///
/// Every point below that resumes from a blocking sleep or AX/pasteboard
/// read and is about to report `Confirmed`, `Unverified`, or
/// `UnverifiedFocusLost` first checks this thread's abandonment signal (see
/// [`install_abandonment_signal`]) and degrades to `NoEvidence` instead when
/// it is set, so an abandoned `doctor --deep` worker thread never restores
/// the clipboard out from under that harness's own cleanup.
pub(crate) fn await_paste_confirmation(ctx: &PasteConfirmationContext<'_>) -> PasteConfirmation {
    let start = Instant::now();

    let element = ctx.focus.and_then(FocusSnapshot::macos_ax_element);
    let Some(element) = element else {
        thread::sleep(UNVERIFIED_GRACE);
        return unless_abandoned(PasteConfirmation::Unverified {
            elapsed: start.elapsed(),
            kind: "unverified_timeout",
        });
    };

    // Build transcript evidence once for the whole polling transaction. The
    // old implementation rebuilt this whitespace-normalized needle on every
    // 40ms tick.
    let matcher = TranscriptMatcher::new(ctx.transcript);
    let (baseline, mut captured_element_usable) = match ctx.baseline {
        Some(baseline) => (
            Some(BoundedNormalizedValue::from_target_value(baseline)),
            true,
        ),
        None => match element.poll_value(MAX_AX_VALUE_UTF16_UNITS) {
            Ok(Some(value)) => (
                Some(BoundedNormalizedValue::from_target_value(&value)),
                true,
            ),
            Ok(None) => (None, true),
            Err(()) => match poll_current_value(ctx) {
                Ok(Some((value, _same_element))) => (Some(value), false),
                Ok(None) => (None, false),
                Err(()) => {
                    return unless_abandoned(PasteConfirmation::UnverifiedFocusLost {
                        elapsed: start.elapsed(),
                        kind: "unverified_focus_lost",
                    });
                }
            },
        },
    };

    let Some(baseline) = baseline else {
        // Neither the pre-chord read nor the immediate post-chord fallback
        // (including focused-element reacquisition when the captured WebKit
        // object died) produced a baseline. Every evidence form in
        // `TranscriptMatcher::indicates_insertion` requires one, so the poll
        // loop below could never confirm anything — it would just spin to
        // the deadline and report a guaranteed false `NoEvidence` alarm.
        // Epistemically this is the same situation as the no-pollable-value
        // case above, so it degrades the same way.
        thread::sleep(UNVERIFIED_GRACE.saturating_sub(start.elapsed()));
        return unless_abandoned(PasteConfirmation::Unverified {
            elapsed: start.elapsed(),
            kind: "unverified_no_baseline",
        });
    };

    let deadline = start + AX_CONFIRM_DEADLINE;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return unless_abandoned(deadline_confirmation(
                poll_current_focus(ctx, &matcher, Some(&baseline)),
                &matcher,
                start.elapsed(),
            ));
        }
        thread::sleep(AX_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));

        let poll = if captured_element_usable {
            element.poll_value(MAX_AX_VALUE_UTF16_UNITS)
        } else {
            match poll_current_focus(ctx, &matcher, Some(&baseline)) {
                Ok(true) => {
                    return unless_abandoned(PasteConfirmation::Confirmed {
                        elapsed: start.elapsed(),
                        kind: "ax_confirmed",
                    });
                }
                Ok(false) => continue,
                Err(()) => {
                    // The captured element had already died (WebKit object
                    // replacement or similar) and the reacquire fallback has
                    // now lost focus entirely: the frontmost application
                    // changed, or its focus state can no longer be read. The
                    // chord was posted into a verified-focused target and
                    // very likely landed, but that target can never be
                    // re-observed, unlike a full-deadline `NoEvidence` where
                    // polling worked the whole window and simply found
                    // nothing.
                    return unless_abandoned(PasteConfirmation::UnverifiedFocusLost {
                        elapsed: start.elapsed(),
                        kind: "unverified_focus_lost",
                    });
                }
            }
        };

        match poll {
            Ok(Some(current)) => {
                let current = BoundedNormalizedValue::from_target_value(&current);
                if matcher.indicates_insertion(
                    Some(&baseline),
                    &current,
                    true,
                    ctx.mode != PasteMode::Terminal,
                ) {
                    return unless_abandoned(PasteConfirmation::Confirmed {
                        elapsed: start.elapsed(),
                        kind: "ax_confirmed",
                    });
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

/// Decide the terminal state of the polling loop after its final focused-
/// element read. Kept pure so focus loss at the deadline and the special
/// short-transcript fallback are regression-testable without live AX access.
fn deadline_confirmation(
    final_poll: Result<bool, ()>,
    matcher: &TranscriptMatcher,
    elapsed: Duration,
) -> PasteConfirmation {
    match final_poll {
        Ok(true) => PasteConfirmation::Confirmed {
            elapsed,
            kind: "ax_confirmed",
        },
        Ok(false) if !matcher.meets_evidence_floor => PasteConfirmation::Unverified {
            elapsed,
            kind: "unverified_short_transcript",
        },
        Ok(false) => PasteConfirmation::NoEvidence {
            elapsed,
            kind: "no_evidence",
        },
        Err(()) => PasteConfirmation::UnverifiedFocusLost {
            elapsed,
            kind: "unverified_focus_lost",
        },
    }
}

/// Reacquire-fallback poll used once the originally captured Accessibility
/// object stops answering (see the `Err(())` arm in
/// [`await_paste_confirmation`]'s loop). This is the highest-frequency
/// caller of `frontmost_application_window`'s `NSWorkspace` query on the
/// worker thread: once triggered, it re-runs on every remaining
/// [`AX_POLL_INTERVAL`] tick for the rest of the confirmation window (up to
/// ~45 calls over [`AX_CONFIRM_DEADLINE`]) rather than once. See the
/// thread-safety note on `frontmost_application_window` in `focus.rs` for
/// why calling it this often off the main thread is accepted.
fn poll_current_focus(
    ctx: &PasteConfirmationContext<'_>,
    matcher: &TranscriptMatcher,
    baseline: Option<&BoundedNormalizedValue>,
) -> Result<bool, ()> {
    let Some((current, same_element)) = poll_current_value(ctx)? else {
        return Ok(false);
    };
    Ok(matcher.indicates_insertion(
        baseline,
        &current,
        same_element,
        ctx.mode != PasteMode::Terminal,
    ))
}

/// Reacquire the current focused value without applying transcript evidence.
/// This is also used to establish a post-chord baseline when the originally
/// captured WebKit object dies before the first confirmation poll.
fn poll_current_value(
    ctx: &PasteConfirmationContext<'_>,
) -> Result<Option<(BoundedNormalizedValue, bool)>, ()> {
    let Some(focus) = ctx.focus else {
        return Ok(None);
    };
    focus
        .macos_poll_current_value(MAX_AX_VALUE_UTF16_UNITS)
        .map(|value| {
            value.map(|(value, same_element)| {
                (
                    BoundedNormalizedValue::from_target_value(&value),
                    same_element,
                )
            })
        })
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
    value_indicates_insertion_for_mode(baseline, current, transcript, PasteMode::Standard)
}

#[cfg(test)]
fn value_indicates_insertion_for_mode(
    baseline: Option<&str>,
    current: &str,
    transcript: &str,
    mode: PasteMode,
) -> bool {
    let matcher = TranscriptMatcher::new(transcript);
    let baseline =
        baseline.map(|value| BoundedNormalizedValue::new(value, MAX_AX_VALUE_UTF16_UNITS));
    let current = BoundedNormalizedValue::new(current, MAX_AX_VALUE_UTF16_UNITS);
    matcher.indicates_insertion(
        baseline.as_ref(),
        &current,
        true,
        mode != PasteMode::Terminal,
    )
}

/// Bounded whitespace-normalized representation of an Accessibility value.
///
/// Small values are kept whole in `head`. For a value over the requested
/// limit, `head` and `tail` hold disjoint leading/trailing halves. They stay
/// separate so concatenating them cannot manufacture a false match across
/// an omitted middle.
///
/// Intentionally separate from `focus::cfstring_to_bounded_value`: this type
/// bounds normalized chars (whitespace-stripped, for layout-independent
/// insertion matching), while that function bounds raw UTF-16 units at the
/// AX FFI boundary. Different unit systems for different jobs — do not merge.
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
/// Evidence requirements fall into three length bands, measured on the
/// normalized (whitespace-stripped) transcript.
///
/// Below [`SHORT_CONFIRM_MIN_OCCURRENCE_CHARS`], only an exact
/// baseline-to-current insertion counts: a string this short collides too
/// easily with unrelated target updates, so occurrence-count and selection-
/// geometry evidence are not trusted. From there through
/// [`CONFIRM_WINDOW_CHARS`], a whole-transcript occurrence-count *increase*
/// over the pre-chord baseline counts too — long enough to be unlikely by
/// coincidence, but still too short to have a reliable leading/trailing
/// window of its own. Above [`CONFIRM_WINDOW_CHARS`], a leading or trailing
/// window also counts on its own: a terminal scrolls the head of a long
/// paste off the top of the screen, and a bounded field truncates the tail,
/// but either distinctive end appearing as a new occurrence (not merely
/// already present, see `evidence_already_present_in_baseline_does_not_confirm`)
/// is positive evidence the paste landed.
struct TranscriptMatcher {
    whole: Option<String>,
    head: Option<String>,
    tail: Option<String>,
    utf16_units: usize,
    /// Whether non-text-specific evidence is safe for this transcript's
    /// length. Below the floor, occurrence and selection geometry collide
    /// too easily with unrelated target churn.
    meets_evidence_floor: bool,
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
                meets_evidence_floor: false,
            };
        }

        let whole = normalized.tail.is_none().then(|| normalized.head.clone());
        let normalized_len = normalized.head.chars().count()
            + normalized
                .tail
                .as_deref()
                .map_or(0, |tail| tail.chars().count());
        let meets_evidence_floor = normalized_len >= SHORT_CONFIRM_MIN_OCCURRENCE_CHARS;
        if normalized_len <= CONFIRM_WINDOW_CHARS {
            return Self {
                whole,
                head: None,
                tail: None,
                utf16_units,
                meets_evidence_floor,
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
            meets_evidence_floor,
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
        allow_occurrence_evidence: bool,
    ) -> bool {
        let exact_insertion = match (&self.whole, baseline) {
            (Some(whole), Some(baseline)) => current.is_exact_insertion_of(baseline, whole),
            _ => false,
        };
        let distinctive_occurrence = match baseline {
            // Occurrence counts alone are unsafe over a fixed-size rendered
            // AX window: scrolling can reveal an older identical string and
            // produce a false 0->1 increase. Require enough total value
            // growth to prove this is not merely a same-size window shift.
            Some(baseline)
                if allow_occurrence_evidence
                    && self.meets_evidence_floor
                    && current.utf16_units > baseline.utf16_units =>
            {
                let current_counts = self.counts(current);
                let baseline_counts = self.counts(baseline);
                current_counts.whole > baseline_counts.whole
                    || current_counts.head > baseline_counts.head
                    || current_counts.tail > baseline_counts.tail
            }
            _ => false,
        };

        exact_insertion
            || distinctive_occurrence
            || (self.meets_evidence_floor
                && allow_selection_evidence
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
    fn insertion_evidence_matrix() {
        struct InsertionCase {
            name: &'static str,
            baseline: Option<&'static str>,
            current: &'static str,
            transcript: &'static str,
            expect: bool,
        }

        let cases = [
            InsertionCase {
                name: "short_transcript_no_baseline_does_not_confirm",
                baseline: None,
                current: "hello world",
                transcript: "world",
                expect: false,
            },
            InsertionCase {
                name: "short_transcript_exact_delta_confirms",
                baseline: Some("hello "),
                current: "hello world",
                transcript: "world",
                expect: true,
            },
            InsertionCase {
                name: "short_transcript_substring_collision_does_not_confirm",
                baseline: Some("draft"),
                current: "draft token",
                transcript: "ok",
                expect: false,
            },
            InsertionCase {
                name: "short_transcript_exact_insertion_confirms_at_end",
                baseline: Some("draft token"),
                current: "draft token ok",
                transcript: "ok",
                expect: true,
            },
            InsertionCase {
                name: "short_transcript_exact_insertion_confirms_mid_word",
                baseline: Some("token"),
                current: "tokoken",
                transcript: "ok",
                expect: true,
            },
            InsertionCase {
                name: "unrelated_growth_does_not_confirm",
                baseline: Some("abc"),
                current: "abcdef",
                transcript: "xyz",
                expect: false,
            },
            InsertionCase {
                name: "no_change_does_not_confirm",
                baseline: Some("abc"),
                current: "abc",
                transcript: "xyz",
                expect: false,
            },
            InsertionCase {
                name: "empty_transcript_never_confirms_with_growth",
                baseline: Some(""),
                current: "x",
                transcript: "",
                expect: false,
            },
            // Windowed (leading/trailing) matching must not fire on an
            // unrelated field that merely happens to hold text.
            InsertionCase {
                name: "unrelated_value_does_not_confirm_via_windowing",
                baseline: Some("some unrelated field contents"),
                current: "some unrelated field contents",
                transcript: "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo",
                expect: false,
            },
            InsertionCase {
                name: "evidence_already_present_in_baseline_does_not_confirm",
                baseline: Some("alpha bravo charlie delta echo foxtrot"),
                current: "alpha bravo charlie delta echo foxtrot",
                transcript: "alpha bravo charlie delta echo foxtrot",
                expect: false,
            },
        ];

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let actual =
                    value_indicates_insertion(case.baseline, case.current, case.transcript);
                (actual != case.expect).then(|| {
                    format!(
                        "{}: baseline={:?} current={:?} transcript={:?} expected {}, got {}",
                        case.name,
                        case.baseline,
                        case.current,
                        case.transcript,
                        case.expect,
                        actual
                    )
                })
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
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
        assert!(value_indicates_insertion_for_mode(
            Some("prompt> "),
            wrapped,
            transcript,
            PasteMode::Terminal,
        ));
    }

    /// A terminal's rendered AX window is not stable occurrence evidence:
    /// scrolling can reveal an older identical transcript without the paste
    /// chord landing. Even when unrelated churn grows the window, occurrence
    /// counts alone must remain disabled in terminal mode.
    #[test]
    fn terminal_sliding_window_does_not_confirm_from_occurrence_shift() {
        let transcript = "the quick brown fox jumps over the lazy dog every single morning";
        let baseline = "prompt one";
        let current = format!("prompt two {transcript}");
        assert!(
            value_indicates_insertion(Some(baseline), &current, transcript),
            "precondition: a stable GUI value may use the occurrence increase"
        );
        assert!(!value_indicates_insertion_for_mode(
            Some(baseline),
            &current,
            transcript,
            PasteMode::Terminal,
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

    /// A transcript at or under [`CONFIRM_WINDOW_CHARS`] gets no windowed
    /// (leading/trailing) fallback: a partial overlap must not be mistaken
    /// for insertion when the whole transcript is short enough to have been
    /// matched outright. This transcript's normalized length (16) actually
    /// clears [`SHORT_CONFIRM_MIN_OCCURRENCE_CHARS`], so the whole-occurrence
    /// fallback added for that band is live here too; this still returns
    /// `false` because the occurrence count does not increase (baseline and
    /// current both hold zero occurrences of the transcript), not because
    /// the fallback is unavailable. See
    /// `short_transcript_confirms_via_occurrence_increase_despite_unrelated_churn`
    /// for the case where that fallback does fire.
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

    /// The live regression behind a false error chime in Discord: a short
    /// transcript (16 normalized chars, above
    /// [`SHORT_CONFIRM_MIN_OCCURRENCE_CHARS`] but at/under
    /// [`CONFIRM_WINDOW_CHARS`], so it gets no leading/trailing window)
    /// landed, but unrelated churn around it (Slate re-rendering, a
    /// zero-width character, a redrawn spinner frame) broke the exact
    /// baseline-to-current insertion delta. A whole-transcript
    /// occurrence-count increase over the baseline is still real evidence
    /// here, even without an exact match.
    #[test]
    fn short_transcript_confirms_via_occurrence_increase_despite_unrelated_churn() {
        let transcript = "hello there friend";
        assert!(transcript.chars().filter(|c| !c.is_whitespace()).count() >= 12);
        assert!(value_indicates_insertion(
            Some("prompt one"),
            "prompt two hello there friend",
            transcript
        ));
    }

    /// Below [`SHORT_CONFIRM_MIN_OCCURRENCE_CHARS`], an occurrence-count
    /// increase alone must not confirm: `"okay"` is short enough to appear in
    /// unrelated growth by coincidence, so only an exact insertion delta is
    /// trusted for it, even though the count here genuinely goes from zero
    /// to one.
    #[test]
    fn below_floor_transcript_does_not_confirm_via_occurrence_increase() {
        let transcript = "okay";
        assert!(transcript.chars().count() < 12);
        assert!(!value_indicates_insertion(
            Some("prompt one"),
            "prompt two okay friend",
            transcript
        ));
    }

    /// A short transcript already present once in the baseline that is
    /// merely still present once in the current value — with unrelated
    /// churn added around it — must not confirm: the occurrence count did
    /// not increase, so this is mere presence, not evidence of a new
    /// insertion (compare `evidence_already_present_in_baseline_does_not_confirm`,
    /// the equivalent case for long transcripts).
    #[test]
    fn short_transcript_present_once_in_both_does_not_confirm_despite_churn() {
        let transcript = "hello there friend";
        assert!(!value_indicates_insertion(
            Some(transcript),
            &format!("xyz {transcript} abc"),
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
    fn selection_geometry_confirmation_matrix() {
        struct SelectionCase {
            name: &'static str,
            transcript: &'static str,
            current_head: &'static str,
            current_utf16_units: usize,
            current_selection: PasteTargetSelection,
            allow_selection_evidence: bool,
            expect: bool,
        }

        // Shared baseline for every row: a 5-unit "draft" field with the
        // whole word selected, as if about to be replaced by dictation.
        let baseline = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: "draft".to_owned(),
            tail: None,
            utf16_units: 5,
            selection: Some(PasteTargetSelection {
                location: 0,
                length: 5,
            }),
        });

        let cases = [
            SelectionCase {
                name: "exact_selection_geometry_confirms_reformatted_safari_text",
                transcript: "please say \"hi\"",
                current_head: "please say \u{201c}hi\u{201d}",
                current_utf16_units: 15,
                current_selection: PasteTargetSelection {
                    location: 15,
                    length: 0,
                },
                allow_selection_evidence: true,
                expect: true,
            },
            SelectionCase {
                name: "replacement_ax_object_requires_text_evidence",
                transcript: "please say \"hi\"",
                current_head: "please say \u{201c}hi\u{201d}",
                current_utf16_units: 15,
                current_selection: PasteTargetSelection {
                    location: 15,
                    length: 0,
                },
                allow_selection_evidence: false,
                expect: false,
            },
            SelectionCase {
                name: "short_transcript_does_not_use_selection_geometry",
                transcript: "ok",
                current_head: "ok",
                current_utf16_units: 2,
                current_selection: PasteTargetSelection {
                    location: 2,
                    length: 0,
                },
                allow_selection_evidence: true,
                expect: false,
            },
            SelectionCase {
                name: "selection_motion_without_expected_value_length_does_not_confirm",
                transcript: "hello",
                current_head: "unrelated",
                current_utf16_units: 9,
                current_selection: PasteTargetSelection {
                    location: 5,
                    length: 0,
                },
                allow_selection_evidence: true,
                expect: false,
            },
        ];

        // Precondition for the reformatted-Safari-text row: Safari can
        // reformat straight quotes to curly quotes on insertion, so the
        // current value's normalized head must NOT contain the normalized
        // transcript outright — otherwise this row would be trivially
        // covered by plain substring matching, not the selection-geometry
        // evidence path it exists to exercise.
        let safari_case = cases
            .iter()
            .find(|case| case.name == "exact_selection_geometry_confirms_reformatted_safari_text")
            .expect("safari row must be present");
        let safari_current = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
            head: safari_case.current_head.to_owned(),
            tail: None,
            utf16_units: safari_case.current_utf16_units,
            selection: Some(safari_case.current_selection),
        });
        assert!(
            !safari_current
                .head
                .contains(&normalize_segment(safari_case.transcript)),
            "precondition: reformatted text must not contain the transcript verbatim"
        );

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let matcher = TranscriptMatcher::new(case.transcript);
                let current = BoundedNormalizedValue::from_target_value(&PasteTargetValue {
                    head: case.current_head.to_owned(),
                    tail: None,
                    utf16_units: case.current_utf16_units,
                    selection: Some(case.current_selection),
                });
                let actual = matcher.indicates_insertion(
                    Some(&baseline),
                    &current,
                    case.allow_selection_evidence,
                    true,
                );
                (actual != case.expect)
                    .then(|| format!("{}: expected {}, got {}", case.name, case.expect, actual))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn deadline_confirmation_matrix() {
        let elapsed = Duration::from_millis(AX_CONFIRM_DEADLINE.as_millis() as u64);
        let short = TranscriptMatcher::new("okay");
        let long = TranscriptMatcher::new("hello there friend");

        assert!(matches!(
            deadline_confirmation(Ok(true), &long, elapsed),
            PasteConfirmation::Confirmed {
                kind: "ax_confirmed",
                ..
            }
        ));
        assert!(matches!(
            deadline_confirmation(Ok(false), &short, elapsed),
            PasteConfirmation::Unverified {
                kind: "unverified_short_transcript",
                ..
            }
        ));
        assert!(matches!(
            deadline_confirmation(Ok(false), &long, elapsed),
            PasteConfirmation::NoEvidence {
                kind: "no_evidence",
                ..
            }
        ));
        assert!(matches!(
            deadline_confirmation(Err(()), &long, elapsed),
            PasteConfirmation::UnverifiedFocusLost {
                kind: "unverified_focus_lost",
                ..
            }
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
            mode: PasteMode::Standard,
            baseline: None,
        };
        let confirmation = await_paste_confirmation(&ctx);
        clear_abandonment_signal();
        match confirmation {
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

    /// Once this thread's abandonment signal (see
    /// [`install_abandonment_signal`]) is set, `await_paste_confirmation`
    /// must degrade what would otherwise be `Unverified` to `NoEvidence`
    /// instead, so its caller in `daemon::desktop::inject` skips restoring
    /// the clipboard for an abandoned `doctor --deep` worker thread. Uses
    /// the same no-focus-snapshot path as
    /// `no_focus_snapshot_is_unverified_without_ax_permission` so this stays
    /// exercisable without a live Accessibility grant.
    #[test]
    fn abandoned_signal_degrades_unverified_to_no_evidence() {
        install_abandonment_signal(Arc::new(AtomicBool::new(true)));
        let ctx = PasteConfirmationContext {
            focus: None,
            transcript: "hello",
            mode: PasteMode::Standard,
            baseline: None,
        };
        let confirmation = await_paste_confirmation(&ctx);
        clear_abandonment_signal();
        match confirmation {
            PasteConfirmation::NoEvidence { kind, .. } => {
                assert_eq!(kind, "no_evidence");
            }
            other => panic!("expected NoEvidence once abandoned, got {other:?}"),
        }
    }

    /// As [`abandoned_signal_degrades_unverified_to_no_evidence`], for the
    /// sibling [`PasteConfirmation::UnverifiedFocusLost`] path. Driving
    /// `await_paste_confirmation` itself into that branch needs a live,
    /// then-broken captured Accessibility object, which is unavailable
    /// without an Accessibility permission grant, so this calls the helper
    /// directly instead — exactly as
    /// `no_focus_snapshot_is_unverified_without_ax_permission` does for the
    /// no-pollable-value case above.
    #[test]
    fn abandoned_signal_degrades_unverified_focus_lost_to_no_evidence() {
        install_abandonment_signal(Arc::new(AtomicBool::new(true)));
        let confirmation = unless_abandoned(PasteConfirmation::UnverifiedFocusLost {
            elapsed: Duration::from_millis(5),
            kind: "unverified_focus_lost",
        });
        clear_abandonment_signal();
        match confirmation {
            PasteConfirmation::NoEvidence { kind, .. } => {
                assert_eq!(kind, "no_evidence");
            }
            other => panic!("expected NoEvidence once abandoned, got {other:?}"),
        }
    }

    /// A thread that never calls [`install_abandonment_signal`] — every
    /// production daemon worker thread — must be completely unaffected: the
    /// signal is inert by default.
    #[test]
    fn no_installed_signal_is_never_treated_as_abandoned() {
        assert!(!is_abandoned());
    }
}
