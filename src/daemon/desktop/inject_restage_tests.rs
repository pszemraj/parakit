//! Competing-clipboard and re-stage tests for the window before the paste chord is dispatched.

use super::*;

#[test]
fn competing_clipboard_during_final_focus_guard_restages_once_and_pastes() {
    for policy in [
        ClipboardPolicy::RestorePrevious,
        ClipboardPolicy::KeepTranscript,
    ] {
        for competing in competing_clipboard_payloads()
            .into_iter()
            .chain([MockClipboardContent::Text("dictated text".to_string())])
        {
            let mut clipboard = MockClipboard::new("old clipboard");
            let pending = Rc::clone(&clipboard.pending_external_write);
            let events = clipboard.events();
            let mut guard_calls = 0;
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || {
                    assert_eq!(
                        events
                            .borrow()
                            .iter()
                            .filter(|event| *event == "set:dictated text")
                            .count(),
                        2
                    );
                    Ok(PasteDispatch::Posted)
                },
                Duration::ZERO,
                restore_plan(&quiet_gate()),
                policy,
                None,
                || {
                    guard_calls += 1;
                    if guard_calls == 2 {
                        *pending.borrow_mut() = Some(competing.clone());
                    }
                    Ok(true)
                },
            )
            .expect("clipboard competition must not withhold a safe paste");
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert!(report.telemetry.paste_event_posted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert_eq!(clipboard.text(), Some("dictated text"));
            assert!(report.diagnostic.is_some());
            assert_eq!(guard_calls, 3, "re-stage requires one final focus recheck");
            assert_eq!(
                clipboard
                    .events
                    .borrow()
                    .iter()
                    .filter(|event| event.starts_with("set:"))
                    .count(),
                2
            );
        }
    }
}

#[test]
fn post_settle_clipboard_failure_restages_before_dispatch() {
    for competing in [
        None,
        Some(MockClipboardContent::Text("new copy".to_owned())),
        Some(MockClipboard::image().content),
    ] {
        for policy in [
            ClipboardPolicy::RestorePrevious,
            ClipboardPolicy::KeepTranscript,
        ] {
            let mut clipboard = MockClipboard::new("old clipboard");
            let pending = Rc::clone(&clipboard.pending_external_write);
            let unavailable = Rc::clone(&clipboard.text_unavailable);
            let events = clipboard.events();
            let queued = competing.clone();
            let gate = quiet_gate().on_baseline(move || {
                if let Some(content) = &queued {
                    *pending.borrow_mut() = Some(content.clone());
                } else {
                    unavailable.set(true);
                }
            });
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || {
                    assert_eq!(
                        events
                            .borrow()
                            .iter()
                            .filter(|event| *event == "set:dictated text")
                            .count(),
                        2
                    );
                    Ok(PasteDispatch::Posted)
                },
                Duration::ZERO,
                restore_plan(&gate),
                policy,
                None,
                || Ok(true),
            )
            .unwrap();
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert!(report.telemetry.paste_event_posted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert_eq!(clipboard.text(), Some("dictated text"));
            assert!(report.diagnostic.is_some());
            assert_eq!(
                events
                    .borrow()
                    .iter()
                    .filter(|event| *event == "set:dictated text")
                    .count(),
                2
            );
        }
    }
}

#[test]
fn failed_restage_does_not_paste_foreign_content() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let pending = Rc::clone(&clipboard.pending_external_write);
    let fail_write = Rc::clone(&clipboard.fail_next_set);
    let gate = quiet_gate().on_baseline(move || {
        *pending.borrow_mut() = Some(MockClipboardContent::Text("new copy".to_owned()));
        fail_write.set(true);
    });
    let error = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || panic!("a failed re-stage cannot authorize pasting foreign content"),
        Duration::ZERO,
        restore_plan(&gate),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .unwrap_err();
    assert!(error.to_string().contains("could not re-copy transcript"));
    assert_eq!(clipboard.text(), Some("new copy"));
}

#[test]
fn clipboard_restage_does_not_override_final_focus_safety() {
    for fail_guard in [false, true] {
        let mut clipboard = MockClipboard::new("old clipboard");
        let pending = Rc::clone(&clipboard.pending_external_write);
        let gate = quiet_gate().on_baseline(move || {
            *pending.borrow_mut() = Some(MockClipboardContent::Text("new copy".to_owned()));
        });
        let mut guards = 0;
        let report = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || panic!("unsafe or unavailable focus must prevent dispatch"),
            Duration::ZERO,
            restore_plan(&gate),
            ClipboardPolicy::RestorePrevious,
            None,
            || {
                guards += 1;
                if guards == 2 && fail_guard {
                    anyhow::bail!("focus unavailable");
                }
                Ok(guards == 1)
            },
        )
        .unwrap();
        assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
        assert!(!report.telemetry.paste_event_posted);
        assert_eq!(clipboard.text(), Some("new copy"));
        assert_eq!(
            clipboard
                .events
                .borrow()
                .iter()
                .filter(|event| *event == "set:dictated text")
                .count(),
            1
        );
    }
}

// X11 stamps name the selection owner rather than a write; see
// `clipboard_manager_reown_after_restage_still_pastes`.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_new_copy_during_restaged_focus_check_prevents_dispatch() {
    for competing in [
        MockClipboardContent::Text("latest copy".to_owned()),
        MockClipboard::image().content,
    ] {
        for policy in [
            ClipboardPolicy::RestorePrevious,
            ClipboardPolicy::KeepTranscript,
        ] {
            let mut clipboard = MockClipboard::new("old clipboard");
            let pending = Rc::clone(&clipboard.pending_external_write);
            let mut guards = 0;
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || panic!("a copy during the restaged focus check must prevent dispatch"),
                Duration::ZERO,
                restore_plan(&quiet_gate()),
                policy,
                None,
                || {
                    guards += 1;
                    if guards == 2 {
                        *pending.borrow_mut() =
                            Some(MockClipboardContent::Text("first copy".to_owned()));
                    } else if guards == 3 {
                        *pending.borrow_mut() = Some(competing.clone());
                    }
                    Ok(true)
                },
            )
            .unwrap();
            assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
            assert!(!report.telemetry.paste_event_posted);
            assert_eq!(report.telemetry.clipboard_restored, None);
            assert_eq!(clipboard.content, competing);
            assert_eq!(guards, 3);
            assert_eq!(
                clipboard
                    .events
                    .borrow()
                    .iter()
                    .filter(|event| *event == "set:dictated text")
                    .count(),
                2,
                "do not retry staging over a second competing copy"
            );
        }
    }
}

#[test]
fn focus_lost_during_restage_keeps_transcript_without_dispatch() {
    for fail_guard in [false, true] {
        let mut clipboard = MockClipboard::new("old clipboard");
        let pending = Rc::clone(&clipboard.pending_external_write);
        let events = clipboard.events();
        let gate = quiet_gate().on_baseline(move || {
            *pending.borrow_mut() = Some(MockClipboardContent::Text("new copy".to_owned()));
        });
        let mut guards = 0;
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || panic!("focus lost during re-stage must prevent dispatch"),
            Duration::ZERO,
            restore_plan(&gate),
            ClipboardPolicy::RestorePrevious,
            None,
            || {
                guards += 1;
                if guards == 3 {
                    assert_eq!(
                        events
                            .borrow()
                            .iter()
                            .filter(|event| *event == "set:dictated text")
                            .count(),
                        2
                    );
                    if fail_guard {
                        anyhow::bail!("focus unavailable after re-stage");
                    }
                    return Ok(false);
                }
                Ok(true)
            },
        );
        if fail_guard {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("focus unavailable"));
        } else {
            let report = result.unwrap();
            assert_eq!(report.outcome, PasteOutcome::CopiedOnly);
            assert!(!report.telemetry.paste_event_posted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
        }
        assert_eq!(guards, 3);
        assert_eq!(clipboard.text(), Some("dictated text"));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn clipboard_manager_handoffs_restage_before_paste_and_allow_restore_after_paste() {
    for handoff_before_paste in [true, false] {
        let mut clipboard = MockClipboard::new("old clipboard");
        let pending = Rc::clone(&clipboard.pending_external_write);
        let confirmation_pending = Rc::clone(&pending);
        let gate = quiet_gate().on_confirmation(move || {
            if !handoff_before_paste {
                *confirmation_pending.borrow_mut() =
                    Some(MockClipboardContent::Text("dictated text".to_string()));
            }
        });
        let mut guards = 0;
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
            || {
                guards += 1;
                if handoff_before_paste && guards == 2 {
                    *pending.borrow_mut() =
                        Some(MockClipboardContent::Text("dictated text".to_string()));
                }
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(report.outcome, PasteOutcome::Pasted);
        if handoff_before_paste {
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert_eq!(clipboard.text(), Some("dictated text"));
        } else {
            assert_eq!(report.telemetry.clipboard_restored, Some(true));
            assert_eq!(clipboard.text(), Some("old clipboard"));
        }
        assert_eq!(guards, if handoff_before_paste { 3 } else { 2 });
    }
}

#[cfg(target_os = "linux")]
#[test]
fn clipboard_manager_reown_after_restage_still_pastes() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let pending = Rc::clone(&clipboard.pending_external_write);
    let posted = Cell::new(false);
    let mut guards = 0;
    let report = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || {
            posted.set(true);
            Ok(PasteDispatch::Posted)
        },
        Duration::ZERO,
        restore_plan(&quiet_gate()),
        ClipboardPolicy::RestorePrevious,
        None,
        || {
            guards += 1;
            // The manager re-owns the transcript during both final focus checks.
            if guards >= 2 {
                *pending.borrow_mut() =
                    Some(MockClipboardContent::Text("dictated text".to_owned()));
            }
            Ok(true)
        },
    )
    .unwrap();
    assert_eq!(report.outcome, PasteOutcome::Pasted);
    assert!(posted.get());
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(guards, 3);
}

#[test]
fn unreadable_clipboard_stamp_still_pastes_and_leaves_transcript() {
    for fail_capture in [true, false] {
        let mut clipboard = MockClipboard::new("old clipboard");
        clipboard.stamp_unavailable.set(fail_capture);
        let unavailable = Rc::clone(&clipboard.stamp_unavailable);
        let mut guards = 0;
        let report = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            PasteMode::Standard,
            || true,
            || Ok(PasteDispatch::Posted),
            Duration::ZERO,
            restore_plan(&quiet_gate()),
            ClipboardPolicy::RestorePrevious,
            None,
            || {
                guards += 1;
                if guards == 2 {
                    unavailable.set(true);
                }
                Ok(true)
            },
        )
        .expect("failed observation must leave the transcript rather than fail");
        assert_eq!(report.outcome, PasteOutcome::Pasted);
        assert_eq!(report.telemetry.clipboard_restored, None);
        assert_eq!(clipboard.text(), Some("dictated text"));
        assert!(report.telemetry.paste_event_posted);
        assert_eq!(guards, 3);
        assert_eq!(
            clipboard
                .events
                .borrow()
                .iter()
                .filter(|event| *event == "set:dictated text")
                .count(),
            2,
            "an unreadable stamp must cause exactly one re-stage"
        );
        assert!(
            report
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("clipboard stamp unavailable")),
            "missing stamp failure for fail_capture={fail_capture}: {:?}",
            report.diagnostic
        );
    }
}

#[test]
fn clipboard_stamp_recheck_restages_same_plain_payload_changed_during_read() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let competing = MockClipboardContent::Html {
        html: "<i>dictated text</i>".to_string(),
        alt_text: Some("dictated text".to_string()),
    };
    clipboard.after_guard_read = Some(competing.clone());
    let report = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || Ok(PasteDispatch::Posted),
        Duration::ZERO,
        restore_plan(&quiet_gate()),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .expect("read-back race must still deliver transcript");
    assert_eq!(report.outcome, PasteOutcome::Pasted);
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(report.telemetry.clipboard_restored, Some(false));
}
