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
        false,
        &log
    ));
    assert_eq!(verification.get(), "matched");
    assert!(!focus_verification_allows_insertion(
        Ok(FocusVerification::Changed),
        &verification,
        false,
        &log
    ));
    assert_eq!(verification.get(), "changed");
    #[cfg(target_os = "macos")]
    {
        assert!(focus_verification_allows_insertion(
            Ok(FocusVerification::AxUnsupported),
            &verification,
            true,
            &log
        ));
        assert_eq!(verification.get(), "ax_unsupported");
    }

    assert!(!unavailable_focus_allows_insertion(true, &log));
    assert!(unavailable_focus_allows_insertion(false, &log));

    assert!(!focus_verification_allows_insertion(
        Err(anyhow::anyhow!("focus unavailable")),
        &verification,
        true,
        &log
    ));
    assert_eq!(verification.get(), "not_applicable");
    assert!(focus_verification_allows_insertion(
        Err(anyhow::anyhow!("temporary X11 failure")),
        &verification,
        false,
        &log
    ));
    assert_eq!(verification.get(), "not_applicable");
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
            "direct multiline paste",
            "first\nsecond".to_string(),
            PasteMode::Direct,
            PastePlan::Paste("first\nsecond".to_string()),
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
            typed_chars: None,
            failure_reason: None,
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

#[test]
fn direct_typing_failure_reports_progress_and_alerts() {
    let report = InsertReport::direct_failure(2, "focus changed".to_string());
    assert_eq!(report.outcome, InsertOutcome::Blocked);
    assert_eq!(report.typed_chars, Some(2));
    assert_eq!(report.failure_reason.as_deref(), Some("focus changed"));
    assert!(report.telemetry.paste_event_posted);
    assert!(report.needs_alert());

    let before_input = InsertReport::direct_failure(0, "modifier held".to_string());
    assert!(!before_input.telemetry.paste_event_posted);
    assert!(before_input.needs_alert());
}

#[test]
fn operational_direct_failure_keeps_progress_telemetry_on_the_error() {
    let report = InsertReport::direct_failure(3, "X11 key event failed".to_string());
    let error: anyhow::Error = ReportedInsertionError {
        message: "direct typing failed after 3 of 8 characters".to_string(),
        report: report.clone(),
    }
    .into();

    assert_eq!(insertion_error_report(&error), Some(&report));
    assert_eq!(report.typed_chars, Some(3));
    assert!(report.telemetry.paste_event_posted);
    assert!(format!("{error:#}").contains("3 of 8"));
}

#[test]
fn restore_failure_warning_does_not_claim_the_clipboard_was_preserved() {
    let report = crate::daemon::inject::PasteReport {
        outcome: crate::daemon::inject::PasteOutcome::Pasted,
        telemetry: InsertionTelemetry {
            paste_event_posted: true,
            acknowledgement_kind: "target_value_changed",
            acknowledgement_ms: Some(12),
            clipboard_restored: Some(false),
        },
        diagnostic: Some("clipboard restore write failed".to_string()),
    };

    let warning = clipboard_restore_warning(false, &report).expect("restore failure warning");
    assert!(warning.contains(crate::daemon::inject::CLIPBOARD_RESTORE_ERROR));
    assert!(warning.contains("clipboard restore write failed"));
    assert!(!warning.contains("current clipboard preserved"));
    assert_eq!(clipboard_restore_warning(true, &report), None);
}

#[test]
fn no_evidence_with_competing_clipboard_alerts_without_claiming_a_block() {
    let report = InsertReport {
        outcome: InsertOutcome::PastedUnverified,
        telemetry: InsertionTelemetry {
            paste_event_posted: true,
            acknowledgement_kind: "no_evidence",
            acknowledgement_ms: Some(1800),
            clipboard_restored: None,
        },
        typed_chars: None,
        failure_reason: None,
    };
    assert_eq!(report.outcome.log_label(), "pasted_unverified");
    assert!(report.needs_alert());
}

#[test]
fn clipboard_observation_failure_reaches_worker_diagnostics() {
    let paste_report = crate::daemon::inject::PasteReport {
        outcome: crate::daemon::inject::PasteOutcome::ClipboardChanged,
        telemetry: InsertionTelemetry::not_applicable(false, None),
        diagnostic: Some("could not read clipboard text: unavailable".to_string()),
    };
    let report = InsertReport::from_paste(InsertOutcome::Blocked, paste_report);
    assert_eq!(
        report.failure_reason.as_deref(),
        Some("could not read clipboard text: unavailable")
    );
    assert!(report.needs_alert());
}

#[test]
fn paste_completion_sends_outcome_notifications() {
    use crate::daemon::inject::{PasteOutcome, PasteReport};

    let cases = [
        (
            PasteOutcome::Pasted,
            true,
            "target_value_changed",
            InsertOutcome::Pasted,
            None,
            false,
        ),
        (
            PasteOutcome::PastedUnverified,
            true,
            "unverified_focus_lost",
            InsertOutcome::PastedUnverified,
            None,
            false,
        ),
        (
            PasteOutcome::PastedUnverified,
            true,
            "no_evidence",
            InsertOutcome::PastedUnverified,
            Some((
                "Paste unconfirmed",
                PasteBlockReason::UnconfirmedClipboardChanged.notice(),
            )),
            true,
        ),
        (
            PasteOutcome::CopiedOnly,
            false,
            "not_applicable",
            InsertOutcome::CopiedOnly,
            Some((
                "Transcript copied",
                PasteBlockReason::FocusChangedBeforePaste.notice(),
            )),
            false,
        ),
        (
            PasteOutcome::CopiedOnly,
            true,
            "no_evidence",
            InsertOutcome::CopiedOnly,
            Some(("Paste unconfirmed", PasteBlockReason::Unconfirmed.notice())),
            true,
        ),
        (
            PasteOutcome::Blocked,
            false,
            "not_applicable",
            InsertOutcome::Blocked,
            Some((
                "Paste blocked",
                PasteBlockReason::FocusChangedBeforePaste.notice(),
            )),
            true,
        ),
        (
            PasteOutcome::ClipboardChanged,
            false,
            "not_applicable",
            InsertOutcome::Blocked,
            Some(("Paste blocked", PasteBlockReason::ClipboardChanged.notice())),
            true,
        ),
    ];
    for (outcome, posted, acknowledgement_kind, expected_outcome, notice, alert) in cases {
        let log = Arc::new(Logger::new(LogLevel::Quiet));
        let (notifier, messages) = Notifier::test_channel(Arc::clone(&log));
        let report = finish_paste(
            PasteReport {
                outcome,
                telemetry: InsertionTelemetry {
                    paste_event_posted: posted,
                    acknowledgement_kind,
                    acknowledgement_ms: posted.then_some(12),
                    clipboard_restored: None,
                },
                diagnostic: None,
            },
            false,
            &log,
            &notifier,
        );
        assert_eq!(report.outcome, expected_outcome, "{outcome:?}");
        assert_eq!(report.needs_alert(), alert, "{outcome:?}");
        if let Some((summary, body)) = notice {
            let actual = messages.try_recv().expect("outcome notification");
            assert_eq!(actual.summary, summary, "{outcome:?}");
            assert_eq!(actual.body, body, "{outcome:?}");
        }
        assert!(matches!(
            messages.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }
}

#[test]
fn held_modifiers_warn_even_in_quiet_mode_and_notify_copy_only() {
    // A child captures actual stderr without replacing the production logger.
    const CHILD: &str = "PARAKIT_TEST_HELD_MODIFIERS_WARNING";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::worker::insertion::tests::held_modifiers_warn_even_in_quiet_mode_and_notify_copy_only",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("warning:"), "{stderr:?}");
        assert!(
            stderr.contains(PasteBlockReason::UnsafeModifiers.notice()),
            "{stderr:?}"
        );
        return;
    }

    let log = Arc::new(Logger::new(LogLevel::Quiet));
    let (notifier, messages) = Notifier::test_channel(Arc::clone(&log));
    let report = finish_paste(
        crate::daemon::inject::PasteReport {
            outcome: crate::daemon::inject::PasteOutcome::UnsafeModifiers,
            telemetry: InsertionTelemetry::not_applicable(false, Some(false)),
            diagnostic: None,
        },
        false,
        &log,
        &notifier,
    );
    assert_eq!(report.outcome, InsertOutcome::CopiedOnly);
    assert!(!report.telemetry.paste_event_posted);
    assert!(!report.needs_alert());
    let notice = messages.try_recv().expect("held-modifier notification");
    assert_eq!(notice.summary, "Transcript copied");
    assert_eq!(notice.body, PasteBlockReason::UnsafeModifiers.notice());
    assert!(matches!(
        messages.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
}
