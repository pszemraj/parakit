//! Previous-clipboard snapshot, staged-clipboard currency, and payload restore tests.

use super::super::clipboard_guard::{ClipboardRestore, StagedClipboard};
use super::super::clipboard_store::clipboard_content_unavailable;
#[cfg(target_os = "linux")]
use super::super::clipboard_store::linux_file_list_paths;
use super::*;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt as _;

fn assert_failed_previous_capture_keeps_transcript(
    prepare: impl Fn(&mut MockClipboard),
    expected_diagnostic: &str,
) {
    for paste in [false, true] {
        let mut clipboard = MockClipboard::new("old clipboard");
        prepare(&mut clipboard);
        if paste {
            let mut dispatched = false;
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || {
                    dispatched = true;
                    Ok(PasteDispatch::Posted)
                },
                Duration::ZERO,
                restore_plan(&quiet_gate()),
                ClipboardPolicy::RestorePrevious,
                None,
                || Ok(true),
            )
            .expect("a failed previous capture must not fail the dictation");
            assert!(
                dispatched,
                "a failed previous capture must not withhold the paste"
            );
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert!(
                report
                    .diagnostic
                    .as_deref()
                    .is_some_and(|diagnostic| { diagnostic.contains(expected_diagnostic) }),
                "missing previous-capture failure: {:?}",
                report.diagnostic
            );
        } else {
            let outcome = stage_text_without_paste(
                &mut clipboard,
                "dictated text",
                restore_plan(&quiet_gate()),
                ClipboardPolicy::RestorePrevious,
            )
            .expect("a failed previous capture must not fail staging");
            assert_eq!(outcome, StageOutcome::CopiedOnly);
        }
        assert_eq!(clipboard.text(), Some("dictated text"));
    }
}

#[test]
fn snapshot_owner_change_before_staging_still_delivers_transcript() {
    assert_failed_previous_capture_keeps_transcript(
        |clipboard| {
            clipboard.after_snapshot_read = Some(MockClipboardContent::Text("new copy".to_owned()));
        },
        "changed while it was being saved",
    );
}

#[test]
fn mismatched_owner_format_counts_as_unavailable_content() {
    let mismatch = anyhow::Error::new(arboard::Error::Unknown {
        description: "incorrect type received from clipboard".to_owned(),
    })
    .context("could not read clipboard file list");
    assert!(clipboard_content_unavailable(&mismatch));

    let other = anyhow::Error::new(arboard::Error::Unknown {
        description: "selection owner timed out".to_owned(),
    });
    assert!(!clipboard_content_unavailable(&other));
    assert!(!clipboard_content_unavailable(&anyhow::Error::new(
        arboard::Error::ClipboardOccupied
    )));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_file_list_adapter_removes_crlf_separator_without_changing_path_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = crate::test_support::fixture_root("parakit-clipboard-test", "file-list-crlf");
    let normal = dir.join("document.txt");
    let unchanged = dir.join("other.txt");
    let both_normal = dir.join("both.txt");
    let both_cr = dir.join("both.txt\r");
    let dangling_normal = dir.join("dangling.txt");
    let dangling_cr = dir.join("dangling.txt\r");
    let non_utf8 = dir.join(OsString::from_vec(b"non-utf8-\xff.txt".to_vec()));
    for path in [
        &normal,
        &unchanged,
        &both_normal,
        &both_cr,
        &dangling_normal,
        &non_utf8,
    ] {
        std::fs::write(path, b"").unwrap();
    }
    std::os::unix::fs::symlink(dir.join("missing-target"), &dangling_cr).unwrap();
    let unresolved = dir.join("missing.txt\r");
    let paths = vec![
        dir.join("document.txt\r"),
        unchanged.clone(),
        both_cr.clone(),
        both_normal.clone(),
        dangling_cr.clone(),
        dir.join(OsString::from_vec(b"non-utf8-\xff.txt\r".to_vec())),
        non_utf8.clone(),
        unresolved.clone(),
    ];
    assert_eq!(
        linux_file_list_paths(paths),
        vec![
            normal,
            unchanged,
            both_cr,
            both_normal,
            dangling_cr,
            non_utf8.clone(),
            non_utf8,
            unresolved,
        ]
    );
}

#[test]
fn empty_clipboard_stamp_zero_can_be_staged_pasted_and_restored() {
    for paste in [false, true] {
        let mut clipboard = MockClipboard::empty();
        clipboard.generation = 0;
        if paste {
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
            .unwrap();
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert!(report.telemetry.paste_event_posted);
        } else {
            let outcome = stage_text_without_paste(
                &mut clipboard,
                "dictated text",
                restore_plan(&quiet_gate()),
                ClipboardPolicy::RestorePrevious,
            )
            .unwrap();
            assert!(matches!(outcome, StageOutcome::Blocked));
        }
        assert_eq!(clipboard.content, MockClipboardContent::Empty);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn unsupported_restore_preserves_identical_text_from_replacement_owner() {
    let mut clipboard = MockClipboard::new("dictated text");
    let staged = StagedClipboard::capture(
        &mut clipboard,
        ClipboardSnapshot::Unsupported,
        "dictated text",
    );
    *clipboard.pending_external_write.borrow_mut() =
        Some(MockClipboardContent::Text("dictated text".to_owned()));
    // A prior insertion check may have accepted a manager handoff. The
    // unsupported restore must still compare against our original owner.
    assert!(staged.is_current(&mut clipboard));
    assert!(matches!(
        staged
            .restore(&mut clipboard, ClipboardPolicy::RestorePrevious)
            .unwrap(),
        ClipboardRestore::Changed(_)
    ));
    assert_eq!(clipboard.text(), Some("dictated text"));
}

#[test]
fn clipboard_restore_policy_preserves_supported_non_text_payloads() {
    let cases = [
        (
            "html with alt text",
            MockClipboard::html("<b>old</b>", Some("old")),
            MockClipboardContent::Html {
                html: "<b>old</b>".to_string(),
                alt_text: Some("old".to_string()),
            },
            vec![
                "guard".to_string(),
                "read".to_string(),
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-html:<b>old</b>:old".to_string(),
            ],
        ),
        (
            "file list",
            MockClipboard::file_list(&["/tmp/a.txt", "/tmp/b.txt"]),
            MockClipboardContent::FileList(vec![
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/b.txt"),
            ]),
            vec![
                "guard".to_string(),
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-files:2".to_string(),
            ],
        ),
        (
            "image",
            MockClipboard::image(),
            MockClipboardContent::Image {
                width: 2,
                height: 1,
                bytes: vec![1, 2, 3, 4],
            },
            vec![
                "guard".to_string(),
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-image:2x1:4".to_string(),
            ],
        ),
        (
            "html image without text alternative",
            MockClipboard::html_image(r#"<img src="blob:chatgpt-image">"#, None),
            MockClipboardContent::Image {
                width: 2,
                height: 1,
                bytes: vec![1, 2, 3, 4],
            },
            vec![
                "guard".to_string(),
                "read".to_string(),
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-image:2x1:4".to_string(),
            ],
        ),
        (
            "html image with text alternative",
            MockClipboard::html_image(
                r#"<img src="https://example.invalid/image.webp">"#,
                Some("image alt"),
            ),
            MockClipboardContent::Html {
                html: r#"<img src="https://example.invalid/image.webp">"#.to_string(),
                alt_text: Some("image alt".to_string()),
            },
            vec![
                "guard".to_string(),
                "read".to_string(),
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-html:<img src=\"https://example.invalid/image.webp\">:image alt".to_string(),
            ],
        ),
    ];

    for (name, mut clipboard, expected_content, expected_events) in cases {
        let events = clipboard.events();
        let gate = quiet_gate();
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
        .expect(name);

        assert_eq!(result.outcome, PasteOutcome::Pasted, "{name}");
        assert_eq!(clipboard.content, expected_content, "{name}");
        assert_eq!(
            transaction_events(&events).as_slice(),
            expected_events,
            "{name}"
        );
    }
}

#[test]
fn empty_file_list_with_text_alternative_restores_text() {
    let mut clipboard = MockClipboard::file_list(&[]);
    clipboard.text_alternative = Some("copied URI text".to_string());
    let snapshot = ClipboardSnapshot::capture(&mut clipboard).unwrap();
    assert!(matches!(snapshot, ClipboardSnapshot::Text(ref text) if text == "copied URI text"));

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
    .unwrap();

    assert_eq!(report.outcome, PasteOutcome::Pasted);
    assert_eq!(clipboard.text(), Some("copied URI text"));
}

#[test]
fn unsupported_previous_clipboard_clears_staged_transcript_on_guard_block() {
    let mut clipboard = MockClipboard::unsupported();
    let events = clipboard.events();
    let mut guard_calls = 0;
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
        restore_plan(&PlatformClipboardRestoreGate::fallback()),
        ClipboardPolicy::RestorePrevious,
        None,
        || {
            events.borrow_mut().push("guard".to_string());
            guard_calls += 1;
            Ok(guard_calls == 1)
        },
    )
    .expect("unsupported clipboard should clear staged transcript on guard block");

    assert_eq!(result.outcome, PasteOutcome::Blocked);
    assert_eq!(clipboard.content, MockClipboardContent::Empty);
    assert_eq!(
        transaction_events(&events).as_slice(),
        ["guard", "read", "set:dictated text", "guard", "clear"]
    );
}

#[test]
fn empty_file_lists_do_not_hide_supported_payloads() {
    for (mut clipboard, expected_kind) in [
        (MockClipboard::empty(), "empty or unsupported"),
        (MockClipboard::new("copied URI text"), "text"),
        (MockClipboard::html("<b>copied</b>", Some("copied")), "HTML"),
        (MockClipboard::image(), "image"),
    ] {
        let expected = ClipboardSnapshot::capture(&mut clipboard).unwrap();
        clipboard.empty_file_list_available = true;
        let actual = ClipboardSnapshot::capture(&mut clipboard).unwrap();
        assert_eq!(actual.kind(), expected_kind);
        assert!(expected.same_payload(&actual), "{expected_kind}");
    }
}

#[test]
fn clipboard_snapshot_read_errors_are_not_unavailable_formats() {
    for (format, mut clipboard, expected_context) in [
        (
            "file-list",
            MockClipboard::new("text"),
            "file-list clipboard",
        ),
        ("HTML", MockClipboard::new("text"), "HTML clipboard"),
        ("image", MockClipboard::new("text"), "image clipboard"),
        ("text", MockClipboard::new("text"), "text clipboard"),
        (
            "text",
            MockClipboard::html("<b>text</b>", Some("text")),
            "clipboard text alternative",
        ),
        (
            "image",
            MockClipboard::html_image("<img>", None),
            "image clipboard",
        ),
    ] {
        clipboard.failed_read_format = Some(format);
        let error = ClipboardSnapshot::capture(&mut clipboard)
            .err()
            .expect("a failed format read must fail capture");
        let message = format!("{error:#}");
        assert!(message.contains(expected_context), "{message}");
        assert!(
            message.contains(&format!("{format} read failed")),
            "{message}"
        );
        assert!(!clipboard
            .events
            .borrow()
            .iter()
            .any(|event| { event.starts_with("set") || event == "clear" }));
    }
}

#[test]
fn failed_clipboard_restore_retains_the_snapshot_for_retry() {
    for (mut clipboard, expected_kind) in [
        (MockClipboard::new("previous text"), "text"),
        (MockClipboard::html("<b>old</b>", Some("old")), "HTML"),
        (
            MockClipboard::file_list(&["one.txt", "two.txt"]),
            "file list",
        ),
        (MockClipboard::image(), "image"),
    ] {
        let previous = ClipboardSnapshot::capture(&mut clipboard).unwrap();
        assert_eq!(previous.kind(), expected_kind);
        clipboard.set_text("staged sentinel".to_owned()).unwrap();
        clipboard.fail_next_set.set(true);
        let error = previous.restore(&mut clipboard).unwrap_err();
        assert!(error.is::<ClipboardRestoreError>());
        assert!(format!("{error:#}").contains("could not restore previous clipboard contents"));
        assert_eq!(clipboard.text(), Some("staged sentinel"));

        previous.restore(&mut clipboard).unwrap();
        let restored = ClipboardSnapshot::capture(&mut clipboard).unwrap();
        assert!(previous.same_payload(&restored), "{expected_kind}");
    }
}

#[test]
fn consumed_clipboard_restore_moves_text_and_image_allocations() {
    let text = "previous text".to_owned();
    let text_pointer = text.as_ptr();
    let mut clipboard = MockClipboard::empty();
    ClipboardSnapshot::Text(text)
        .restore_owned(&mut clipboard)
        .unwrap();
    let MockClipboardContent::Text(restored) = &clipboard.content else {
        panic!("text payload was not restored");
    };
    assert_eq!(restored, "previous text");
    assert_eq!(restored.as_ptr(), text_pointer);

    let bytes = vec![31_u8; 16];
    let image_pointer = bytes.as_ptr();
    ClipboardSnapshot::Image(ImageData {
        width: 2,
        height: 2,
        bytes: Cow::Owned(bytes),
    })
    .restore_owned(&mut clipboard)
    .unwrap();
    let MockClipboardContent::Image {
        width,
        height,
        bytes,
    } = &clipboard.content
    else {
        panic!("image payload was not restored");
    };
    assert_eq!((*width, *height, bytes.len()), (2, 2, 16));
    assert_eq!(bytes.as_ptr(), image_pointer);
}

#[test]
fn failure_cleanup_only_replaces_the_staged_sentinel() {
    let sentinel = "parakit-doctor-macos-smoke-123-1";
    assert!(ClipboardSnapshot::Text(sentinel.to_owned()).is_text(sentinel));
    assert!(!ClipboardSnapshot::Text("newer user value".to_owned()).is_text(sentinel));
    assert!(!ClipboardSnapshot::Html {
        html: "<b>newer payload</b>".to_owned(),
        alt_text: Some(sentinel.to_owned()),
    }
    .is_text(sentinel));
    assert!(!ClipboardSnapshot::Unsupported.is_text(sentinel));
}

#[test]
fn clipboard_snapshots_compare_complete_payload_contents() {
    let image = |width, height, bytes: &[u8]| {
        ClipboardSnapshot::Image(ImageData {
            width,
            height,
            bytes: Cow::Owned(bytes.to_vec()),
        })
    };
    let html = |html: &str, alt: Option<&str>| ClipboardSnapshot::Html {
        html: html.to_owned(),
        alt_text: alt.map(str::to_owned),
    };
    let files =
        |names: &[&str]| ClipboardSnapshot::FileList(names.iter().map(PathBuf::from).collect());
    for (expected, actual, matches) in [
        (
            html("<b>hello</b>", Some("hello")),
            html("<b>hello</b>", Some("hello")),
            true,
        ),
        (
            html("<b>hello</b>", Some("hello")),
            html("<i>hello</i>", Some("hello")),
            false,
        ),
        (
            html("<b>hello</b>", Some("hello")),
            html("<b>hello</b>", Some("changed")),
            false,
        ),
        (
            html("<b>hello</b>", None),
            html("<b>hello</b>", Some("")),
            false,
        ),
        (
            ClipboardSnapshot::Text("hello".to_owned()),
            html("<b>hello</b>", Some("hello")),
            false,
        ),
        (image(1, 1, &[0, 1, 2, 3]), image(1, 1, &[0, 1, 2, 3]), true),
        (
            image(1, 1, &[0, 1, 2, 3]),
            image(1, 1, &[3, 2, 1, 0]),
            false,
        ),
        (
            image(2, 1, &[0, 1, 2, 3]),
            image(1, 2, &[0, 1, 2, 3]),
            false,
        ),
        (
            files(&["one.txt", "two.txt"]),
            files(&["one.txt", "two.txt"]),
            true,
        ),
        (
            files(&["one.txt", "two.txt"]),
            files(&["two.txt", "one.txt"]),
            false,
        ),
        (files(&["one.txt"]), files(&["other.txt"]), false),
        (
            ClipboardSnapshot::Unsupported,
            ClipboardSnapshot::Unsupported,
            true,
        ),
        (
            ClipboardSnapshot::Unsupported,
            ClipboardSnapshot::Text(String::new()),
            false,
        ),
    ] {
        assert_eq!(
            expected.same_payload(&actual),
            matches,
            "{}",
            expected.kind()
        );
        assert_eq!(actual.same_payload(&expected), matches, "{}", actual.kind());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn clipboard_manager_rich_handoffs_with_identical_text_preserve_files_and_images() {
    for competing in [
        MockClipboard::file_list(&["/copied/document.txt"]).content,
        MockClipboard::image().content,
    ] {
        for policy in [
            ClipboardPolicy::RestorePrevious,
            ClipboardPolicy::KeepTranscript,
        ] {
            let mut clipboard = MockClipboard::new("dictated text");
            let staged = StagedClipboard::capture(
                &mut clipboard,
                ClipboardSnapshot::Text("old clipboard".to_string()),
                "dictated text",
            );
            clipboard.text_alternative = Some("dictated text".to_string());
            *clipboard.pending_external_write.borrow_mut() = Some(competing.clone());

            assert!(!staged.is_current(&mut clipboard));
            assert_eq!(
                staged.observation_error(),
                None,
                "an image/file clipboard without plain text is a competing payload, not a read failure"
            );
            assert!(matches!(
                staged.restore(&mut clipboard, policy).unwrap(),
                ClipboardRestore::Changed(_)
            ));
            assert_eq!(clipboard.content, competing);
            assert_eq!(clipboard.generation, 2);
            assert_eq!(
                clipboard.events.borrow().as_slice(),
                ["external-write", "guard-read", "guard-read"]
            );
        }
    }
}

#[test]
fn unchanged_clipboard_owner_with_changed_text_is_not_current() {
    let mut clipboard = MockClipboard::new("dictated text");
    let staged = StagedClipboard::capture(
        &mut clipboard,
        ClipboardSnapshot::Text("old clipboard".to_string()),
        "dictated text",
    );
    clipboard.content = MockClipboardContent::Text("new copy".to_string());
    assert!(!staged.is_current(&mut clipboard));
    assert!(matches!(
        staged
            .restore(&mut clipboard, ClipboardPolicy::RestorePrevious)
            .unwrap(),
        ClipboardRestore::Changed(_)
    ));
    assert_eq!(clipboard.text(), Some("new copy"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an X11 display with an existing clipboard owner; read-only"]
fn linux_clipboard_stamp_matches_selection_owner() {
    let (connection, _) = x11rb::connect(None).unwrap();
    let selection = connection
        .intern_atom(false, b"CLIPBOARD")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    let owner = connection
        .get_selection_owner(selection)
        .unwrap()
        .reply()
        .unwrap()
        .owner;
    assert_ne!(owner, x11rb::NONE);
    let mut clipboard = SystemClipboard::new().unwrap();
    assert_eq!(clipboard.change_stamp().unwrap(), u64::from(owner));
}

#[test]
fn recovered_stamp_read_preserves_a_failed_capture_diagnostic() {
    let mut clipboard = MockClipboard::new("dictated text");
    clipboard.stamp_unavailable.set(true);
    let staged = StagedClipboard::capture(
        &mut clipboard,
        ClipboardSnapshot::Text("old clipboard".to_string()),
        "dictated text",
    );

    clipboard.stamp_unavailable.set(false);
    assert!(!staged.is_current(&mut clipboard));
    assert!(
        staged
            .observation_error()
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("could not capture clipboard stamp")),
        "successful retry must not erase a failed capture diagnostic: {:?}",
        staged.observation_error()
    );
}

#[test]
fn unreadable_previous_clipboard_still_pastes_and_keeps_transcript() {
    assert_failed_previous_capture_keeps_transcript(
        |clipboard| clipboard.text_unavailable.set(true),
        "clipboard text unavailable",
    );
}

#[test]
fn successful_clipboard_recheck_clears_a_transient_read_error() {
    let mut clipboard = MockClipboard::new("dictated text");
    let staged = StagedClipboard::capture(
        &mut clipboard,
        ClipboardSnapshot::Text("old clipboard".to_string()),
        "dictated text",
    );
    clipboard.text_unavailable.set(true);
    assert!(!staged.is_current(&mut clipboard));
    assert!(staged.observation_error().is_some());

    clipboard.text_unavailable.set(false);
    assert!(staged.is_current(&mut clipboard));
    assert_eq!(staged.observation_error(), None);
}
