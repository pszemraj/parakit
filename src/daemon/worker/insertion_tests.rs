//! Regression coverage for focus guards, paste sanitization, and outcome cues.

use super::*;
use crate::daemon::logging::LogLevel;

#[test]
fn unavailable_or_unverified_focus_uses_platform_policy() {
    let log = Logger::new(LogLevel::Quiet);
    let verification = Cell::new("unset");

    assert!(focus_verification_allows_insertion(
        Ok(FocusVerification::Matched),
        &verification,
        &log
    ));
    assert_eq!(verification.get(), "matched");
    assert!(!focus_verification_allows_insertion(
        Ok(FocusVerification::Changed),
        &verification,
        &log
    ));
    assert_eq!(verification.get(), "changed");
    #[cfg(target_os = "macos")]
    {
        assert!(focus_verification_allows_insertion(
            Ok(FocusVerification::AxUnsupported),
            &verification,
            &log
        ));
        assert_eq!(verification.get(), "ax_unsupported");
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let verification = Cell::new("not_applicable");
        let focus = FocusCheck {
            snapshot: None,
            verification: &verification,
        };
        assert!(!focus_allows_insertion(focus, &log));
        assert_eq!(verification.get(), "unavailable");
        assert!(!focus_verification_allows_insertion(
            Err(anyhow::anyhow!("focus unavailable")),
            &verification,
            &log
        ));
        assert_eq!(verification.get(), "not_applicable");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let verification = Cell::new("not_applicable");
        let focus = FocusCheck {
            snapshot: None,
            verification: &verification,
        };
        assert!(focus_allows_insertion(focus, &log));
        assert_eq!(verification.get(), "unavailable");
        assert!(focus_verification_allows_insertion(
            Err(anyhow::anyhow!("temporary X11 failure")),
            &verification,
            &log
        ));
        assert_eq!(verification.get(), "not_applicable");
    }
}

#[test]
fn paste_failures_use_clipboard_fallback_only_when_safe() {
    let paste_error = anyhow::anyhow!("could not send paste shortcut");
    assert!(paste_failure_uses_clipboard_fallback(
        PasteMode::Terminal,
        &paste_error,
        true
    ));
    assert!(!paste_failure_uses_clipboard_fallback(
        PasteMode::Terminal,
        &paste_error,
        false
    ));

    let restore_error = anyhow::anyhow!("{}: lost", crate::daemon::inject::CLIPBOARD_RESTORE_ERROR);
    assert!(!paste_failure_uses_clipboard_fallback(
        PasteMode::Terminal,
        &restore_error,
        true
    ));

    let direct_error = anyhow::anyhow!("could not type text at cursor");
    assert!(!paste_failure_uses_clipboard_fallback(
        PasteMode::Direct,
        &direct_error,
        true
    ));
}

#[test]
fn insertion_failures_remember_transcript_for_ipc_recovery() {
    fn report(outcome: InsertOutcome) -> InsertReport {
        InsertReport::placeholder(outcome, false)
    }

    /// Expected `insertion_result_remembers_transcript` result for
    /// `outcome`.
    ///
    /// Exhaustive with no wildcard arm: a new `InsertOutcome` variant
    /// fails compilation here until this function states its
    /// expectation.
    fn expected_remembers_transcript(outcome: InsertOutcome) -> bool {
        match outcome {
            InsertOutcome::Pasted
            | InsertOutcome::PastedUnverified
            | InsertOutcome::CopiedOnly
            | InsertOutcome::Blocked => true,
            InsertOutcome::Skipped => false,
        }
    }

    let outcomes = [
        InsertOutcome::Pasted,
        InsertOutcome::PastedUnverified,
        InsertOutcome::CopiedOnly,
        InsertOutcome::Blocked,
        InsertOutcome::Skipped,
    ];

    let failures: Vec<String> = outcomes
        .iter()
        .filter_map(|&outcome| {
            let expected = expected_remembers_transcript(outcome);
            let actual = insertion_result_remembers_transcript(&Ok(report(outcome)));
            (actual != expected).then(|| format!("{outcome:?}: expected {expected}, got {actual}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    assert!(insertion_result_remembers_transcript(&Err(
        anyhow::anyhow!("paste failed")
    )));
}

#[test]
fn paste_sanitizer_cases_are_stable() {
    let cases = [
        (
            "standard controls",
            "hello\0\r\nworld\x07".to_string(),
            PasteMode::Standard,
            PastePlan::Paste("hello\nworld".to_string()),
        ),
        (
            "terminal trailing newlines",
            "cargo test\n\n".to_string(),
            PasteMode::Terminal,
            PastePlan::Paste("cargo test".to_string()),
        ),
        (
            "terminal multiline copy-only",
            "first\nsecond".to_string(),
            PasteMode::Terminal,
            PastePlan::CopyOnly {
                text: "first\nsecond".to_string(),
                reason: PasteBlockReason::MultilineTerminal,
            },
        ),
        (
            "empty standard skip",
            "\0\x07\n".to_string(),
            PasteMode::Standard,
            PastePlan::Skip {
                reason: PasteBlockReason::EmptyAfterSanitization,
            },
        ),
    ];

    for (name, raw, mode, expected) in cases {
        assert_eq!(sanitize_for_paste(&raw, mode), expected, "{name}");
    }
}

#[test]
fn needs_alert_matches_outcome_and_paste_event_posted() {
    fn report(outcome: InsertOutcome, paste_event_posted: bool) -> InsertReport {
        InsertReport {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(paste_event_posted, None),
        }
    }

    /// Expected `needs_alert()` results for `outcome`, as
    /// `(paste_event_posted, expected)` pairs.
    ///
    /// Exhaustive with no wildcard arm: a new `InsertOutcome` variant
    /// fails compilation here until this function states its alert
    /// expectation.
    fn needs_alert_cases(outcome: InsertOutcome) -> &'static [(bool, bool)] {
        match outcome {
            InsertOutcome::Pasted => &[(true, false)],
            InsertOutcome::PastedUnverified => &[(true, false)],
            InsertOutcome::Skipped => &[(false, false)],
            InsertOutcome::Blocked => &[(false, true), (true, true)],
            InsertOutcome::CopiedOnly => &[
                // pre-chord CopiedOnly (guard-blocked, focus changed, or
                // unsafe modifiers) stays quiet
                (false, false),
                // post-chord CopiedOnly (chord sent but never confirmed)
                // must still alert
                (true, true),
            ],
        }
    }

    let outcomes = [
        InsertOutcome::Pasted,
        InsertOutcome::PastedUnverified,
        InsertOutcome::CopiedOnly,
        InsertOutcome::Blocked,
        InsertOutcome::Skipped,
    ];

    let failures: Vec<String> = outcomes
            .iter()
            .flat_map(|&outcome| {
                needs_alert_cases(outcome)
                    .iter()
                    .filter_map(move |&(paste_event_posted, expected)| {
                        let actual = report(outcome, paste_event_posted).needs_alert();
                        (actual != expected).then(|| {
                            format!(
                                "{outcome:?} paste_event_posted={paste_event_posted}: expected {expected}, got {actual}"
                            )
                        })
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
