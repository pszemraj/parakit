//! Doctor sentinel, clipboard snapshot comparison, and paste-report verdict regressions.

use super::*;

#[test]
fn sentinel_is_unique_per_pid_and_nonce() {
    let a = build_sentinel(123, 1);
    let b = build_sentinel(123, 2);
    let c = build_sentinel(124, 1);
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert!(a.starts_with("parakit-doctor-macos-smoke-123-"));
}

#[test]
fn failure_cleanup_only_replaces_the_staged_sentinel() {
    let sentinel = "parakit-doctor-macos-smoke-123-1";
    assert!(DoctorClipboardSnapshot::Text(sentinel.to_owned()).is_text(sentinel));
    assert!(
        !DoctorClipboardSnapshot::Text("a newer user clipboard value".to_owned()).is_text(sentinel)
    );
    assert!(!DoctorClipboardSnapshot::Html {
        html: "<b>newer payload</b>".to_owned(),
        alt_text: Some(sentinel.to_owned()),
    }
    .is_text(sentinel));
}

#[test]
fn non_text_clipboard_snapshots_compare_payload_contents() {
    let html = DoctorClipboardSnapshot::Html {
        html: "<b>hello</b>".to_owned(),
        alt_text: Some("hello".to_owned()),
    };
    let changed_html = DoctorClipboardSnapshot::Html {
        html: "<b>hello</b>".to_owned(),
        alt_text: Some("different".to_owned()),
    };
    assert!(html.same_payload(&DoctorClipboardSnapshot::Html {
        html: "<b>hello</b>".to_owned(),
        alt_text: Some("hello".to_owned()),
    }));
    assert!(!html.same_payload(&changed_html));

    let image = DoctorClipboardSnapshot::Image(ImageData {
        width: 1,
        height: 1,
        bytes: Cow::Owned(vec![0, 1, 2, 3]),
    });
    let changed_image = DoctorClipboardSnapshot::Image(ImageData {
        width: 1,
        height: 1,
        bytes: Cow::Owned(vec![3, 2, 1, 0]),
    });
    assert!(!image.same_payload(&changed_image));

    let files = DoctorClipboardSnapshot::FileList(vec![PathBuf::from("one.txt")]);
    assert!(
        files.same_payload(&DoctorClipboardSnapshot::FileList(vec![PathBuf::from(
            "one.txt"
        )]))
    );
}

fn report(outcome: PasteOutcome, acknowledgement_kind: &'static str) -> PasteReport {
    PasteReport {
        outcome,
        telemetry: crate::daemon::desktop::inject::InsertionTelemetry {
            paste_event_posted: true,
            acknowledgement_kind,
            acknowledgement_ms: Some(42),
            clipboard_restored: Some(true),
        },
        diagnostic: None,
    }
}

/// One (kind, ax_focused) input paired with the expected
/// `check_paste_report` result, grouped per [`PasteOutcome`] variant by
/// [`report_rows`].
struct ReportRow {
    name: &'static str,
    kind: &'static str,
    ax_focused: bool,
    expect_ok: bool,
}

/// Rows exercised for `outcome`, keyed through an exhaustive match (no
/// wildcard arm) so adding a new [`PasteOutcome`] variant fails
/// compilation here until its `check_paste_report` coverage is stated as
/// data. [`PasteOutcome`]'s variants are all fieldless (see
/// `daemon::desktop::inject::PasteOutcome`), so matching on `outcome`
/// alone is exhaustive without binding any payload.
fn report_rows(outcome: PasteOutcome) -> &'static [ReportRow] {
    match outcome {
        PasteOutcome::Pasted => &[
            ReportRow {
                name: "ax_confirmed_paste_passes_ax_available",
                kind: "ax_confirmed",
                ax_focused: true,
                expect_ok: true,
            },
            ReportRow {
                name: "ax_confirmed_paste_passes_ax_unavailable",
                kind: "ax_confirmed",
                ax_focused: false,
                expect_ok: true,
            },
            ReportRow {
                name: "pasted_without_ax_confirmed_kind_fails_ax_available",
                kind: "not_applicable",
                ax_focused: true,
                expect_ok: false,
            },
            ReportRow {
                name: "pasted_without_ax_confirmed_kind_fails_ax_unavailable",
                kind: "not_applicable",
                ax_focused: false,
                expect_ok: false,
            },
        ],
        PasteOutcome::PastedUnverified => &[
            ReportRow {
                name: "unverified_paste_passes_when_ax_capture_failed",
                kind: "unverified_timeout",
                ax_focused: false,
                expect_ok: true,
            },
            ReportRow {
                name: "unverified_paste_fails_when_ax_capture_available",
                kind: "unverified_timeout",
                ax_focused: true,
                expect_ok: false,
            },
        ],
        PasteOutcome::CopiedOnly => &[
            ReportRow {
                name: "copied_only_fails_ax_available",
                kind: "no_evidence",
                ax_focused: true,
                expect_ok: false,
            },
            ReportRow {
                name: "copied_only_fails_ax_unavailable",
                kind: "no_evidence",
                ax_focused: false,
                expect_ok: false,
            },
        ],
        // New coverage: `PasteOutcome::UnsafeModifiers` previously had
        // zero test coverage despite a dedicated production arm in
        // `check_paste_report` (~diagnostics.rs:815-819). That arm
        // returns `Err` unconditionally — unlike the `PastedUnverified`
        // arm, it carries no `if !ax_focused_element_available` guard —
        // so both rows below pin `expect_ok: false` regardless of
        // `ax_focused`.
        PasteOutcome::UnsafeModifiers => &[
            ReportRow {
                name: "unsafe_modifiers_fails_ax_available",
                kind: "not_applicable",
                ax_focused: true,
                expect_ok: false,
            },
            ReportRow {
                name: "unsafe_modifiers_fails_ax_unavailable",
                kind: "not_applicable",
                ax_focused: false,
                expect_ok: false,
            },
        ],
        PasteOutcome::Blocked => &[
            ReportRow {
                name: "blocked_fails_ax_available",
                kind: "not_applicable",
                ax_focused: true,
                expect_ok: false,
            },
            ReportRow {
                name: "blocked_fails_ax_unavailable",
                kind: "not_applicable",
                ax_focused: false,
                expect_ok: false,
            },
        ],
        PasteOutcome::ClipboardChanged => &[
            ReportRow {
                name: "changed_clipboard_fails_ax_available",
                kind: "not_applicable",
                ax_focused: true,
                expect_ok: false,
            },
            ReportRow {
                name: "changed_clipboard_fails_ax_unavailable",
                kind: "not_applicable",
                ax_focused: false,
                expect_ok: false,
            },
        ],
    }
}

#[test]
fn check_paste_report_matches_expected_ok_per_outcome_and_kind() {
    let outcomes = [
        PasteOutcome::Pasted,
        PasteOutcome::PastedUnverified,
        PasteOutcome::CopiedOnly,
        PasteOutcome::UnsafeModifiers,
        PasteOutcome::Blocked,
        PasteOutcome::ClipboardChanged,
    ];

    let failures: Vec<String> = outcomes
        .iter()
        .flat_map(|&outcome| report_rows(outcome).iter().map(move |row| (outcome, row)))
        .filter_map(|(outcome, row)| {
            let paste_report = report(outcome, row.kind);
            let actual = check_paste_report(&paste_report, row.ax_focused).is_ok();
            (actual != row.expect_ok).then(|| {
                format!(
                    "{}: outcome={outcome:?} kind={:?} ax_focused={} expected \
                     is_ok()={}, got {actual}",
                    row.name, row.kind, row.ax_focused, row.expect_ok
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
