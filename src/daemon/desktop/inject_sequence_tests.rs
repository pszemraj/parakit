//! Insertion contract and transaction ordering tests: guards, modifiers, and restore gating.

use super::super::paste_transaction::report_from_stage_outcome;
use super::*;

#[test]
fn paste_mode_labels_are_stable() {
    assert_eq!(PasteMode::Terminal.label(), "terminal");
    assert_eq!(PasteMode::Standard.label(), "standard");
    assert_eq!(PasteMode::Direct.label(), "direct");
}

struct ClipboardCase {
    name: &'static str,
    initial: Option<&'static str>,
    transcript: &'static str,
    guard_allows: bool,
    /// When set, the `before_chord` guard closure returns this `Err`
    /// instead of `Ok(guard_allows)` on its (only) call. Per
    /// `paste_with_clipboard_swap_guarded` (paste_transaction.rs), an `Err`
    /// aborts immediately with no staging, unlike `Ok(false)` which still
    /// stages (and, under `RestorePrevious`, restores) before returning.
    guard_error: Option<&'static str>,
    paste_error: Option<&'static str>,
    fail_next_set: bool,
    policy: ClipboardPolicy,
    expected_text: Option<&'static str>,
    expected_events: &'static [&'static str],
    expected_outcome: Option<PasteOutcome>,
    error_contains: Option<&'static str>,
    /// Expected `PasteReport::clipboard_restored`. `None` means "don't
    /// check" (most rows never verified this field); `Some(x)` asserts the
    /// report's field equals `Some(x)`.
    expected_clipboard_restored: Option<bool>,
}

#[test]
fn clipboard_swap_cases_are_stable() {
    let cases = [
        ClipboardCase {
            name: "failed paste restores previous clipboard by default",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: true,
            guard_error: None,
            paste_error: Some("paste failed"),
            fail_next_set: false,
            policy: ClipboardPolicy::RestorePrevious,
            expected_text: Some("old clipboard"),
            expected_events: &[
                "guard",
                "read",
                "set:dictated text",
                "guard",
                "paste",
                "set:old clipboard",
            ],
            expected_outcome: None,
            error_contains: Some("paste failed"),
            expected_clipboard_restored: None,
        },
        ClipboardCase {
            name: "same clipboard text remains available after paste",
            initial: Some("dictated text"),
            transcript: "dictated text",
            guard_allows: true,
            guard_error: None,
            paste_error: None,
            fail_next_set: false,
            policy: ClipboardPolicy::RestorePrevious,
            expected_text: Some("dictated text"),
            expected_events: &[
                "guard",
                "read",
                "set:dictated text",
                "guard",
                "paste",
                "set:dictated text",
            ],
            expected_outcome: Some(PasteOutcome::Pasted),
            error_contains: None,
            expected_clipboard_restored: None,
        },
        ClipboardCase {
            name: "guard blocks paste after staging transcript",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: false,
            guard_error: None,
            paste_error: None,
            fail_next_set: false,
            policy: ClipboardPolicy::RestorePrevious,
            expected_text: Some("old clipboard"),
            expected_events: &["guard", "read", "set:dictated text", "set:old clipboard"],
            expected_outcome: Some(PasteOutcome::Blocked),
            error_contains: None,
            expected_clipboard_restored: None,
        },
        ClipboardCase {
            name: "transcript clipboard write failure does not paste or restore",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: true,
            guard_error: None,
            paste_error: None,
            fail_next_set: true,
            policy: ClipboardPolicy::RestorePrevious,
            expected_text: Some("old clipboard"),
            expected_events: &["guard", "read", "set:dictated text"],
            expected_outcome: None,
            error_contains: Some("could not copy transcript to clipboard"),
            expected_clipboard_restored: None,
        },
        // Folded from `clipboard_guard_error_before_staging_leaves_clipboard_untouched`:
        // an `Err` from the guard aborts before any staging happens at all,
        // unlike `Ok(false)` (the "guard blocks..." row above), which still
        // stages and restores. `expected_events` of exactly `["guard"]`
        // proves no clipboard read/set ever occurred.
        ClipboardCase {
            name: "guard error before staging leaves clipboard untouched",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: false, // unread: guard_error short-circuits first
            guard_error: Some("focus unavailable"),
            paste_error: None,
            fail_next_set: false,
            policy: ClipboardPolicy::RestorePrevious,
            expected_text: Some("old clipboard"),
            expected_events: &["guard"],
            expected_outcome: None,
            error_contains: Some("focus unavailable"),
            expected_clipboard_restored: None,
        },
        // Folded from `clipboard_keep_transcript_policy_leaves_text_after_guard_block`:
        // the guard blocks on its first (only) call, routing through
        // `stage_text_without_paste`'s `KeepTranscript` branch
        // (paste_transaction.rs), which sets the transcript and returns without ever
        // reading, restoring, or touching the restore-gate machinery.
        ClipboardCase {
            name: "keep-transcript policy leaves transcript on clipboard after guard block",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: false,
            guard_error: None,
            paste_error: None,
            fail_next_set: false,
            policy: ClipboardPolicy::KeepTranscript,
            expected_text: Some("dictated text"),
            expected_events: &["guard", "set:dictated text"],
            expected_outcome: Some(PasteOutcome::CopiedOnly),
            error_contains: None,
            expected_clipboard_restored: Some(false),
        },
    ];

    for case in cases {
        let mut clipboard = match case.initial {
            Some(text) => MockClipboard::new(text),
            None => MockClipboard::empty(),
        };
        if case.fail_next_set {
            clipboard = clipboard.fail_next_set();
        }

        let events = clipboard.events();
        let gate = quiet_gate();
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            case.transcript,
            PasteMode::Standard,
            || true,
            || {
                events.borrow_mut().push("paste".to_string());
                match case.paste_error {
                    Some(message) => Err(anyhow::anyhow!("{message}")),
                    None => Ok(PasteDispatch::Posted),
                }
            },
            Duration::ZERO,
            restore_plan(&gate),
            case.policy,
            None,
            || {
                events.borrow_mut().push("guard".to_string());
                if let Some(message) = case.guard_error {
                    return Err(anyhow::anyhow!("{message}"));
                }
                Ok(case.guard_allows)
            },
        );

        match case.error_contains {
            Some(fragment) => {
                let err = result.expect_err(case.name);
                assert!(format!("{err:#}").contains(fragment), "{}", case.name);
            }
            None => {
                let report = result.expect(case.name);
                assert_eq!(
                    report.outcome,
                    case.expected_outcome.unwrap(),
                    "{}",
                    case.name
                );
                if let Some(expected_restored) = case.expected_clipboard_restored {
                    assert_eq!(
                        report.telemetry.clipboard_restored,
                        Some(expected_restored),
                        "{}",
                        case.name
                    );
                }
            }
        }
        assert_eq!(clipboard.text(), case.expected_text, "{}", case.name);
        assert_eq!(
            transaction_events(&events).as_slice(),
            case.expected_events,
            "{}",
            case.name
        );
    }
}

#[test]
fn modifier_wait_and_baseline_complete_before_final_focus_recheck() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let events = clipboard.events();
    let gate = MockRestoreGate::new(Rc::clone(&events)).record_baseline();
    let result = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || {
            events.borrow_mut().push("modifier-wait".to_string());
            true
        },
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
    .expect("safe modifiers and stable focus should allow paste");

    assert_eq!(result.outcome, PasteOutcome::Pasted);
    assert_eq!(
        transaction_events(&events).as_slice(),
        [
            "guard",
            "modifier-wait",
            "read",
            "before-write:10",
            "set:dictated text",
            "after-write:11",
            "baseline",
            "guard",
            "paste",
            "wait:10->11",
            "set:old clipboard"
        ]
    );
}

#[test]
fn modifier_wait_timeout_keeps_transcript_without_posting() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let events = clipboard.events();
    let result = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || {
            events.borrow_mut().push("modifier-wait".to_string());
            false
        },
        || {
            events.borrow_mut().push("paste".to_string());
            Ok(PasteDispatch::Posted)
        },
        Duration::ZERO,
        restore_plan(&quiet_gate()),
        ClipboardPolicy::RestorePrevious,
        None,
        || {
            events.borrow_mut().push("guard".to_string());
            Ok(true)
        },
    )
    .expect("withholding an unsafe chord should be recoverable");

    assert_eq!(result.outcome, PasteOutcome::UnsafeModifiers);
    assert!(!result.telemetry.paste_event_posted);
    assert_eq!(result.telemetry.clipboard_restored, Some(false));
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(
        transaction_events(&events).as_slice(),
        ["guard", "modifier-wait", "set:dictated text"]
    );
}

#[test]
fn unsafe_modifier_skip_keeps_staged_transcript_without_posting() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let events = clipboard.events();
    let result = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || {
            events.borrow_mut().push("paste-attempt".to_string());
            Ok(PasteDispatch::SkippedUnsafeModifiers)
        },
        Duration::ZERO,
        restore_plan(&PlatformClipboardRestoreGate::fallback()),
        ClipboardPolicy::RestorePrevious,
        None,
        || {
            events.borrow_mut().push("guard".to_string());
            Ok(true)
        },
    )
    .expect("withholding an unsafe chord should be recoverable");

    assert_eq!(result.outcome, PasteOutcome::UnsafeModifiers);
    assert!(!result.telemetry.paste_event_posted);
    assert_eq!(result.telemetry.acknowledgement_kind, "not_applicable");
    assert_eq!(result.telemetry.clipboard_restored, Some(false));
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(
        transaction_events(&events).as_slice(),
        [
            "guard",
            "read",
            "set:dictated text",
            "guard",
            "paste-attempt"
        ]
    );
}

#[derive(Clone, Copy)]
enum ClipboardRestoreAction {
    Paste,
    StageOnly,
}

#[derive(Clone, Copy)]
enum MockGateMode {
    Normal,
    Timeout,
    UnadvancedSequence,
}

impl MockGateMode {
    fn gate(self, events: Rc<RefCell<Vec<String>>>) -> MockRestoreGate {
        let gate = MockRestoreGate::new(events);
        match self {
            Self::Normal => gate,
            Self::Timeout => gate.timeout(),
            Self::UnadvancedSequence => gate.without_sequence_advance(),
        }
    }
}

struct ClipboardRestoreCase {
    name: &'static str,
    initial: &'static str,
    action: ClipboardRestoreAction,
    policy: ClipboardPolicy,
    gate: MockGateMode,
    expected_outcome: PasteOutcome,
    expected_text: &'static str,
    expected_events: &'static [&'static str],
}

#[test]
fn clipboard_restore_gate_cases_are_stable() {
    let cases = [
        ClipboardRestoreCase {
            name: "waits for confirmation before restore",
            initial: "old clipboard",
            action: ClipboardRestoreAction::Paste,
            policy: ClipboardPolicy::RestorePrevious,
            gate: MockGateMode::Normal,
            expected_outcome: PasteOutcome::Pasted,
            expected_text: "old clipboard",
            expected_events: &[
                "guard",
                "read",
                "before-write:10",
                "set:dictated text",
                "after-write:11",
                "guard",
                "paste",
                "wait:10->11",
                "set:old clipboard",
            ],
        },
        ClipboardRestoreCase {
            name: "timeout path still restores",
            initial: "old clipboard",
            action: ClipboardRestoreAction::Paste,
            policy: ClipboardPolicy::RestorePrevious,
            gate: MockGateMode::Timeout,
            expected_outcome: PasteOutcome::Pasted,
            expected_text: "old clipboard",
            expected_events: &[
                "guard",
                "read",
                "before-write:10",
                "set:dictated text",
                "after-write:11",
                "guard",
                "paste",
                "wait-timeout:10->11",
                "set:old clipboard",
            ],
        },
        ClipboardRestoreCase {
            name: "stage-only waits for confirmation before restore",
            initial: "old clipboard",
            action: ClipboardRestoreAction::StageOnly,
            policy: ClipboardPolicy::RestorePrevious,
            gate: MockGateMode::Normal,
            expected_outcome: PasteOutcome::Blocked,
            expected_text: "old clipboard",
            expected_events: &[
                "read",
                "before-write:10",
                "set:dictated text",
                "after-write:11",
                "wait:10->11",
                "set:old clipboard",
            ],
        },
        ClipboardRestoreCase {
            // Confirmation still runs under `KeepTranscript` (the outcome
            // variant, e.g. `Pasted` vs `PastedUnverified` vs a no-evidence
            // `CopiedOnly`, is meaningful telemetry/UX regardless of
            // clipboard policy), but it never restores the previous
            // clipboard contents.
            name: "keep transcript still awaits confirmation but never restores",
            initial: "old clipboard",
            action: ClipboardRestoreAction::Paste,
            policy: ClipboardPolicy::KeepTranscript,
            gate: MockGateMode::Normal,
            expected_outcome: PasteOutcome::Pasted,
            expected_text: "dictated text",
            expected_events: &[
                "guard",
                "before-write:10",
                "set:dictated text",
                "after-write:11",
                "guard",
                "paste",
                "wait:10->11",
            ],
        },
        ClipboardRestoreCase {
            name: "unadvanced sequence completes without hanging",
            initial: "dictated text",
            action: ClipboardRestoreAction::Paste,
            policy: ClipboardPolicy::RestorePrevious,
            gate: MockGateMode::UnadvancedSequence,
            expected_outcome: PasteOutcome::Pasted,
            expected_text: "dictated text",
            expected_events: &[
                "guard",
                "read",
                "before-write:10",
                "set:dictated text",
                "after-write:10",
                "guard",
                "paste",
                "wait:10->10",
                "set:dictated text",
            ],
        },
    ];

    for case in cases {
        let mut clipboard = MockClipboard::new(case.initial);
        let events = clipboard.events();
        let gate = case.gate.gate(Rc::clone(&events));
        let result = match case.action {
            ClipboardRestoreAction::Paste => paste_with_clipboard_swap_guarded(
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
            ),
            ClipboardRestoreAction::StageOnly => stage_text_without_paste(
                &mut clipboard,
                "dictated text",
                restore_plan(&gate),
                case.policy,
            )
            .map(report_from_stage_outcome),
        }
        .expect(case.name);

        assert_eq!(result.outcome, case.expected_outcome, "{}", case.name);
        assert_eq!(clipboard.text(), Some(case.expected_text), "{}", case.name);
        assert_eq!(
            transaction_events(&events),
            case.expected_events,
            "{}",
            case.name
        );
    }
}

#[test]
fn competing_clipboard_during_skipped_modifier_dispatch_restages_transcript() {
    for (restage_before_dispatch, unreadable, foreign_write_on_skip) in [
        (false, false, true),
        (false, true, true),
        (true, false, true),
        (true, true, false),
        (true, true, true),
    ] {
        let mut clipboard = MockClipboard::new("old clipboard");
        let pending = Rc::clone(&clipboard.pending_external_write);
        let unavailable = Rc::clone(&clipboard.stamp_unavailable);
        let baseline_pending = Rc::clone(&pending);
        let baseline_unavailable = Rc::clone(&unavailable);
        let gate = quiet_gate().on_baseline(move || {
            if restage_before_dispatch {
                baseline_unavailable.set(unreadable);
                *baseline_pending.borrow_mut() =
                    Some(MockClipboardContent::Text("before dispatch".to_string()));
            }
        });
        let events = clipboard.events();
        let mut dispatch_attempts = 0;
        let mut guards = 0;
        let report = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || {
                dispatch_attempts += 1;
                assert_eq!(
                    events
                        .borrow()
                        .iter()
                        .filter(|event| *event == "set:dictated text")
                        .count(),
                    1 + usize::from(restage_before_dispatch)
                );
                unavailable.set(unreadable);
                if foreign_write_on_skip {
                    *pending.borrow_mut() =
                        Some(MockClipboardContent::Text("new copy".to_string()));
                }
                Ok(PasteDispatch::SkippedUnsafeModifiers)
            },
            Duration::ZERO,
            restore_plan(&gate),
            ClipboardPolicy::RestorePrevious,
            None,
            || {
                guards += 1;
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(report.outcome, PasteOutcome::UnsafeModifiers);
        assert!(!report.telemetry.paste_event_posted);
        assert_eq!(report.telemetry.acknowledgement_kind, "not_applicable");
        assert_eq!(report.telemetry.clipboard_restored, Some(false));
        assert_eq!(clipboard.text(), Some("dictated text"));
        assert!(report.diagnostic.is_some());
        assert_eq!(dispatch_attempts, 1);
        assert_eq!(guards, 2 + usize::from(restage_before_dispatch));
        assert_eq!(
            clipboard
                .events
                .borrow()
                .iter()
                .filter(|event| *event == "set:dictated text")
                .count(),
            2 + usize::from(restage_before_dispatch),
            "one pre-chord re-stage and one recovery after skipped dispatch are bounded"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn direct_control_rejection_does_not_initialize_clipboard_or_paste_backend() {
    let mut injector = Injector {
        enigo: None,
        clipboard: None,
        x11_paste: None,
        x11_direct: None,
    };
    let error = injector
        .paste_text_guarded(
            "a\nb",
            PasteMode::Direct,
            ClipboardPolicy::RestorePrevious,
            None,
            || panic!("control rejection precedes focus checks"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("control characters"));
    assert!(injector.enigo.is_none());
    assert!(injector.clipboard.is_none());
    assert!(injector.x11_paste.is_none());
    assert!(injector.x11_direct.is_none());
}

#[test]
fn stage_only_restore_wait_preserves_new_clipboard() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let pending = Rc::clone(&clipboard.pending_external_write);
    let competing = MockClipboardContent::Text("new copy".to_string());
    let queued = competing.clone();
    let gate = quiet_gate().on_wait(move || {
        *pending.borrow_mut() = Some(queued.clone());
    });
    let outcome = stage_text_without_paste(
        &mut clipboard,
        "dictated text",
        restore_plan(&gate),
        ClipboardPolicy::RestorePrevious,
    )
    .expect("clipboard competition is recoverable");
    assert!(matches!(outcome, StageOutcome::ClipboardChanged(_)));
    assert_eq!(clipboard.content, competing);
}
