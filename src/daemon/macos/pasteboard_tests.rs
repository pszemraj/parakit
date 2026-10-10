//! Paste-acknowledgement evidence policy regressions for transcript matching and deadlines.

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
            let actual = value_indicates_insertion(case.baseline, case.current, case.transcript);
            (actual != case.expect).then(|| {
                format!(
                    "{}: baseline={:?} current={:?} transcript={:?} expected {}, got {}",
                    case.name, case.baseline, case.current, case.transcript, case.expect, actual
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
    let wrapped = "prompt> Wait, what? How is the new rule set from the dic\ntation not in scope?";
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
