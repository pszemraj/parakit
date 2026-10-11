//! Post-paste acknowledgement, failed restore, and competing-clipboard cleanup tests.

use super::*;

struct AcknowledgementCase {
    name: &'static str,
    confirmation: PasteConfirmation,
    policy: ClipboardPolicy,
    expected_outcome: PasteOutcome,
    expected_acknowledgement_kind: &'static str,
    expected_clipboard_restored: Option<bool>,
    expected_text: &'static str,
}

#[test]
fn post_paste_acknowledgement_tiers_drive_outcome_and_clipboard_policy() {
    let cases = [
        AcknowledgementCase {
            name: "confirmed restores clipboard and reports ax_confirmed",
            confirmation: PasteConfirmation::Confirmed {
                elapsed: Duration::from_millis(42),
                kind: "ax_confirmed",
            },
            policy: ClipboardPolicy::RestorePrevious,
            expected_outcome: PasteOutcome::Pasted,
            expected_acknowledgement_kind: "ax_confirmed",
            expected_clipboard_restored: Some(true),
            expected_text: "old clipboard",
        },
        AcknowledgementCase {
            name: "unverified still restores clipboard but is not Pasted",
            confirmation: PasteConfirmation::Unverified {
                elapsed: Duration::from_millis(1500),
                kind: "unverified_timeout",
            },
            policy: ClipboardPolicy::RestorePrevious,
            expected_outcome: PasteOutcome::PastedUnverified,
            expected_acknowledgement_kind: "unverified_timeout",
            expected_clipboard_restored: Some(true),
            expected_text: "old clipboard",
        },
        AcknowledgementCase {
            name: "no evidence leaves transcript on the clipboard instead of restoring",
            confirmation: PasteConfirmation::NoEvidence {
                elapsed: Duration::from_millis(1800),
                kind: "no_evidence",
            },
            policy: ClipboardPolicy::RestorePrevious,
            expected_outcome: PasteOutcome::CopiedOnly,
            expected_acknowledgement_kind: "no_evidence",
            expected_clipboard_restored: Some(false),
            expected_text: "dictated text",
        },
        AcknowledgementCase {
            name: "unverified focus lost is treated as pasted but keeps the transcript",
            confirmation: PasteConfirmation::UnverifiedFocusLost {
                elapsed: Duration::from_millis(900),
                kind: "unverified_focus_lost",
            },
            policy: ClipboardPolicy::RestorePrevious,
            expected_outcome: PasteOutcome::PastedUnverified,
            expected_acknowledgement_kind: "unverified_focus_lost",
            expected_clipboard_restored: Some(false),
            expected_text: "dictated text",
        },
        AcknowledgementCase {
            name: "confirmed with keep-transcript policy leaves transcript on the clipboard",
            confirmation: PasteConfirmation::Confirmed {
                elapsed: Duration::from_millis(10),
                kind: "ax_confirmed",
            },
            policy: ClipboardPolicy::KeepTranscript,
            expected_outcome: PasteOutcome::Pasted,
            expected_acknowledgement_kind: "ax_confirmed",
            expected_clipboard_restored: Some(false),
            expected_text: "dictated text",
        },
    ];

    for case in cases {
        let mut clipboard = MockClipboard::new("old clipboard");
        let events = clipboard.events();
        let gate = MockRestoreGate::new(Rc::clone(&events)).confirmation(case.confirmation);
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(PasteDispatch::Posted)
            },
            Duration::ZERO,
            restore_plan(&gate),
            case.policy,
            None,
            || {
                events.borrow_mut().push("guard".to_string());
                Ok(true)
            },
        )
        .expect(case.name);

        assert_eq!(result.outcome, case.expected_outcome, "{}", case.name);
        assert_eq!(
            result.telemetry.acknowledgement_kind, case.expected_acknowledgement_kind,
            "{}",
            case.name
        );
        assert!(
            result.telemetry.acknowledgement_ms.is_some(),
            "{}: acknowledgement_ms should be populated",
            case.name
        );
        assert_eq!(
            result.telemetry.clipboard_restored, case.expected_clipboard_restored,
            "{}",
            case.name
        );
        assert!(result.telemetry.paste_event_posted, "{}", case.name);
        assert_eq!(clipboard.text(), Some(case.expected_text), "{}", case.name);
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| event.starts_with("confirm:")),
            "{}: expected a confirm: event, got {:?}",
            case.name,
            events.borrow()
        );
    }
}

struct FailedClipboardRestoreCase {
    name: &'static str,
    confirmation: PasteConfirmation,
    expected_outcome: PasteOutcome,
    expected_acknowledgement_kind: &'static str,
}

#[test]
fn paste_survives_a_failed_clipboard_restore() {
    let cases = [
        FailedClipboardRestoreCase {
            name: "confirmed paste survives a failed clipboard restore",
            confirmation: PasteConfirmation::Confirmed {
                elapsed: Duration::from_millis(42),
                kind: "ax_confirmed",
            },
            expected_outcome: PasteOutcome::Pasted,
            expected_acknowledgement_kind: "ax_confirmed",
        },
        FailedClipboardRestoreCase {
            name: "unverified paste survives a failed clipboard restore",
            confirmation: PasteConfirmation::Unverified {
                elapsed: Duration::from_millis(1500),
                kind: "unverified_timeout",
            },
            expected_outcome: PasteOutcome::PastedUnverified,
            expected_acknowledgement_kind: "unverified_timeout",
        },
    ];

    for case in cases {
        let mut clipboard = MockClipboard::new("old clipboard").fail_set_matching("old clipboard");
        let events = clipboard.events();
        let gate = MockRestoreGate::new(Rc::clone(&events)).confirmation(case.confirmation);
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(PasteDispatch::Posted)
            },
            Duration::ZERO,
            restore_plan(&gate),
            ClipboardPolicy::RestorePrevious,
            None,
            || {
                events.borrow_mut().push("guard".to_string());
                Ok(true)
            },
        )
        .expect("a failed restore after a landed paste must not turn success into an error");

        assert_eq!(result.outcome, case.expected_outcome, "{}", case.name);
        assert!(result.telemetry.paste_event_posted, "{}", case.name);
        assert_eq!(
            result.telemetry.acknowledgement_kind, case.expected_acknowledgement_kind,
            "{}",
            case.name
        );
        // The restore attempt failed, so the transcript is still on the
        // clipboard rather than the previous "old clipboard" contents; this must
        // read as `Some(false)`, not silently as `Some(true)` or an `Err` that
        // would discard the already-landed paste.
        assert_eq!(
            result.telemetry.clipboard_restored,
            Some(false),
            "{}",
            case.name
        );
        assert_eq!(clipboard.text(), Some("dictated text"), "{}", case.name);
        assert!(
            result
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("clipboard restore write failed")),
            "{}: missing restore failure: {:?}",
            case.name,
            result.diagnostic
        );
    }
}

#[test]
fn a_new_copy_after_restaged_paste_is_preserved() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let pending = Rc::clone(&clipboard.pending_external_write);
    let later_pending = Rc::clone(&pending);
    let gate = quiet_gate()
        .on_baseline(move || {
            *pending.borrow_mut() = Some(MockClipboardContent::Text("before paste".to_owned()));
        })
        .on_confirmation(move || {
            *later_pending.borrow_mut() =
                Some(MockClipboardContent::Text("after paste".to_owned()));
        });
    let report = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || Ok(PasteDispatch::Posted),
        Duration::ZERO,
        restore_plan(&gate),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .unwrap();
    assert_eq!(report.outcome, PasteOutcome::Pasted);
    assert!(report.telemetry.paste_event_posted);
    assert_eq!(report.telemetry.clipboard_restored, None);
    assert_eq!(clipboard.text(), Some("after paste"));
}

#[test]
fn competing_clipboard_during_confirmation_is_preserved_after_posted_paste() {
    for policy in [
        ClipboardPolicy::RestorePrevious,
        ClipboardPolicy::KeepTranscript,
    ] {
        for (confirmation, expected) in [
            (
                PasteConfirmation::Confirmed {
                    elapsed: Duration::ZERO,
                    kind: "not_applicable",
                },
                PasteOutcome::Pasted,
            ),
            (
                PasteConfirmation::NoEvidence {
                    elapsed: Duration::from_millis(1500),
                    kind: "no_evidence",
                },
                PasteOutcome::PastedUnverified,
            ),
        ] {
            for competing in competing_clipboard_payloads() {
                let mut clipboard = MockClipboard::new("old clipboard");
                let pending = Rc::clone(&clipboard.pending_external_write);
                let queued = competing.clone();
                let gate = quiet_gate()
                    .confirmation(confirmation)
                    .on_confirmation(move || {
                        *pending.borrow_mut() = Some(queued.clone());
                    });
                let report = paste_with_clipboard_swap_guarded(
                    &mut clipboard,
                    "dictated text",
                    PasteMode::Standard,
                    || true,
                    || Ok(PasteDispatch::Posted),
                    Duration::ZERO,
                    restore_plan(&gate),
                    policy,
                    None,
                    || Ok(true),
                )
                .expect("already posted paste must remain successful");
                assert_eq!(report.outcome, expected);
                assert!(report.telemetry.paste_event_posted);
                assert_eq!(report.telemetry.clipboard_restored, None);
                assert_eq!(clipboard.content, competing);
            }
        }
    }
}

#[test]
fn competing_clipboard_during_error_cleanup_suppresses_fallback() {
    for fail_guard in [true, false] {
        for policy in [
            ClipboardPolicy::RestorePrevious,
            ClipboardPolicy::KeepTranscript,
        ] {
            let mut clipboard = MockClipboard::new("old clipboard");
            let guard_pending = Rc::clone(&clipboard.pending_external_write);
            let paste_pending = Rc::clone(&clipboard.pending_external_write);
            let competing = MockClipboardContent::Text("new copy".to_string());
            let mut guard_calls = 0;
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || {
                    assert!(!fail_guard);
                    *paste_pending.borrow_mut() = Some(competing.clone());
                    anyhow::bail!("dispatch failed")
                },
                Duration::ZERO,
                restore_plan(&quiet_gate()),
                policy,
                None,
                || {
                    guard_calls += 1;
                    if fail_guard && guard_calls == 2 {
                        *guard_pending.borrow_mut() = Some(competing.clone());
                        anyhow::bail!("focus unavailable");
                    }
                    Ok(true)
                },
            )
            .expect("competing clipboard cleanup must not trigger fallback copying");
            assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
            assert_eq!(report.telemetry.clipboard_restored, None);
            assert_eq!(clipboard.content, competing);
            let expected = if fail_guard {
                "focus unavailable"
            } else {
                "dispatch failed"
            };
            assert!(
                report
                    .diagnostic
                    .as_deref()
                    .is_some_and(|diagnostic| diagnostic.contains(expected)),
                "missing primary {expected:?} diagnostic: {:?}",
                report.diagnostic
            );
        }
    }
}

#[test]
fn unreadable_clipboard_stamp_after_confirmation_prevents_restoration() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let unavailable = Rc::clone(&clipboard.stamp_unavailable);
    let gate = quiet_gate().on_confirmation(move || unavailable.set(true));
    let report = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || Ok(PasteDispatch::Posted),
        Duration::ZERO,
        restore_plan(&gate),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .expect("already posted paste remains successful");
    assert_eq!(report.outcome, PasteOutcome::Pasted);
    assert_eq!(report.telemetry.clipboard_restored, None);
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert!(
        report
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("clipboard stamp unavailable")),
        "missing post-confirmation stamp failure: {:?}",
        report.diagnostic
    );
}
