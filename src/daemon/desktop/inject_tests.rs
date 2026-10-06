//! Unit tests for clipboard, paste, and XTest cleanup helpers.

use super::*;
use crate::daemon::desktop::clipboard_restore::default_paste_confirmation;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
enum MockClipboardContent {
    Empty,
    Text(String),
    Html {
        html: String,
        alt_text: Option<String>,
    },
    HtmlImage {
        html: String,
        alt_text: Option<String>,
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    FileList(Vec<PathBuf>),
    Image {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    Unsupported,
}

#[derive(Debug)]
struct MockClipboard {
    content: MockClipboardContent,
    text_alternative: Option<String>,
    events: Rc<RefCell<Vec<String>>>,
    fail_next_set: bool,
    fail_set_matching: Option<String>,
    generation: u64,
    pending_external_write: Rc<RefCell<Option<MockClipboardContent>>>,
    stamp_unavailable: Rc<Cell<bool>>,
    text_unavailable: Rc<Cell<bool>>,
    guard_read: bool,
    after_guard_read: Option<MockClipboardContent>,
    after_snapshot_read: Option<MockClipboardContent>,
}

impl MockClipboard {
    fn with_content(content: MockClipboardContent) -> Self {
        Self {
            content,
            text_alternative: None,
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_set_matching: None,
            generation: 1,
            pending_external_write: Rc::new(RefCell::new(None)),
            stamp_unavailable: Rc::new(Cell::new(false)),
            text_unavailable: Rc::new(Cell::new(false)),
            guard_read: false,
            after_guard_read: None,
            after_snapshot_read: None,
        }
    }

    fn new(text: impl Into<String>) -> Self {
        Self::with_content(MockClipboardContent::Text(text.into()))
    }

    fn empty() -> Self {
        Self::with_content(MockClipboardContent::Empty)
    }

    fn html(html: impl Into<String>, alt_text: Option<&str>) -> Self {
        Self::with_content(MockClipboardContent::Html {
            html: html.into(),
            alt_text: alt_text.map(str::to_owned),
        })
    }

    fn html_image(html: impl Into<String>, alt_text: Option<&str>) -> Self {
        Self::with_content(MockClipboardContent::HtmlImage {
            html: html.into(),
            alt_text: alt_text.map(str::to_owned),
            width: 2,
            height: 1,
            bytes: vec![1, 2, 3, 4],
        })
    }

    fn file_list(paths: &[&str]) -> Self {
        Self::with_content(MockClipboardContent::FileList(
            paths.iter().map(PathBuf::from).collect(),
        ))
    }

    fn image() -> Self {
        Self::with_content(MockClipboardContent::Image {
            width: 2,
            height: 1,
            bytes: vec![1, 2, 3, 4],
        })
    }

    fn unsupported() -> Self {
        Self::with_content(MockClipboardContent::Unsupported)
    }

    fn fail_next_set(mut self) -> Self {
        self.fail_next_set = true;
        self
    }

    /// Fail every `set_text` call that writes exactly `text`, without
    /// consuming a one-shot flag. Used to make the *restore* write fail
    /// (which happens after the transcript's own `set_text` call already
    /// succeeded) without needing to count calls.
    fn fail_set_matching(mut self, text: impl Into<String>) -> Self {
        self.fail_set_matching = Some(text.into());
        self
    }

    fn events(&self) -> Rc<RefCell<Vec<String>>> {
        Rc::clone(&self.events)
    }

    fn text(&self) -> Option<&str> {
        match &self.content {
            MockClipboardContent::Text(text) => Some(text),
            _ => None,
        }
    }

    fn fail_set_if_needed(&mut self, text: Option<&str>) -> Result<()> {
        if self.fail_next_set {
            self.fail_next_set = false;
            anyhow::bail!("clipboard write failed");
        }
        if let Some(target) = self.fail_set_matching.as_deref() {
            if Some(target) == text {
                anyhow::bail!("clipboard restore write failed");
            }
        }
        Ok(())
    }

    fn apply_external_write(&mut self) {
        let content = self.pending_external_write.borrow_mut().take();
        if let Some(content) = content {
            self.content = content;
            self.generation += 1;
            self.events.borrow_mut().push("external-write".to_string());
        }
    }
}

impl ClipboardStore for MockClipboard {
    fn change_stamp(&mut self) -> Result<u64> {
        self.apply_external_write();
        // Guard reads follow a write; snapshot reads precede the first one.
        self.guard_read = self.generation > 1;
        if self.stamp_unavailable.get() {
            anyhow::bail!("clipboard stamp unavailable");
        }
        Ok(self.generation)
    }

    fn get_text(&mut self) -> Result<String> {
        self.apply_external_write();
        if self.text_unavailable.get() {
            anyhow::bail!("clipboard text unavailable");
        }
        self.events.borrow_mut().push(
            if self.guard_read {
                "guard-read"
            } else {
                "read"
            }
            .to_string(),
        );
        let text = match &self.content {
            MockClipboardContent::Text(text) => Ok(text.clone()),
            MockClipboardContent::Html {
                alt_text: Some(text),
                ..
            } => Ok(text.clone()),
            MockClipboardContent::HtmlImage {
                alt_text: Some(text),
                ..
            } => Ok(text.clone()),
            _ => self
                .text_alternative
                .clone()
                .ok_or_else(|| arboard::Error::ContentNotAvailable.into()),
        };
        let after_read = if self.after_snapshot_read.is_some() {
            self.after_snapshot_read.take()
        } else if self.guard_read {
            self.after_guard_read.take()
        } else {
            None
        };
        if let Some(content) = after_read {
            *self.pending_external_write.borrow_mut() = Some(content);
        }
        text
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        self.events.borrow_mut().push(format!("set:{text}"));
        self.fail_set_if_needed(Some(&text))?;
        // Unreadable text belongs to the replaced payload, not the clipboard.
        self.text_unavailable.set(false);
        self.content = MockClipboardContent::Text(text);
        self.generation += 1;
        self.guard_read = false;
        Ok(())
    }

    fn get_html(&mut self) -> Result<String> {
        match &self.content {
            MockClipboardContent::Html { html, .. }
            | MockClipboardContent::HtmlImage { html, .. } => Ok(html.clone()),
            _ => Err(arboard::Error::ContentNotAvailable.into()),
        }
    }

    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()> {
        self.events.borrow_mut().push(format!(
            "set-html:{html}:{}",
            alt_text.as_deref().unwrap_or("")
        ));
        self.fail_set_if_needed(None)?;
        self.content = MockClipboardContent::Html { html, alt_text };
        self.generation += 1;
        self.guard_read = false;
        Ok(())
    }

    fn get_file_list(&mut self) -> Result<Vec<PathBuf>> {
        match &self.content {
            MockClipboardContent::FileList(paths) => Ok(paths.clone()),
            _ => Err(arboard::Error::ContentNotAvailable.into()),
        }
    }

    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()> {
        self.events
            .borrow_mut()
            .push(format!("set-files:{}", files.len()));
        self.fail_set_if_needed(None)?;
        self.content = MockClipboardContent::FileList(files.to_vec());
        self.generation += 1;
        self.guard_read = false;
        Ok(())
    }

    fn get_image(&mut self) -> Result<ImageData<'static>> {
        match &self.content {
            MockClipboardContent::Image {
                width,
                height,
                bytes,
            }
            | MockClipboardContent::HtmlImage {
                width,
                height,
                bytes,
                ..
            } => Ok(ImageData {
                width: *width,
                height: *height,
                bytes: Cow::Owned(bytes.clone()),
            }),
            _ => Err(arboard::Error::ContentNotAvailable.into()),
        }
    }

    fn set_image(&mut self, image: ImageData<'static>) -> Result<()> {
        self.events.borrow_mut().push(format!(
            "set-image:{}x{}:{}",
            image.width,
            image.height,
            image.bytes.len()
        ));
        self.fail_set_if_needed(None)?;
        self.content = MockClipboardContent::Image {
            width: image.width,
            height: image.height,
            bytes: image.bytes.into_owned(),
        };
        self.generation += 1;
        self.guard_read = false;
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.events.borrow_mut().push("clear".to_string());
        self.content = MockClipboardContent::Empty;
        self.generation += 1;
        self.guard_read = false;
        Ok(())
    }
}

#[test]
fn snapshot_owner_change_before_staging_still_delivers_transcript() {
    for paste in [false, true] {
        let mut clipboard = MockClipboard::new("old clipboard");
        clipboard.after_snapshot_read = Some(MockClipboardContent::Text("new copy".to_owned()));
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
            .unwrap();
            assert!(dispatched, "a changed snapshot must not withhold the paste");
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert!(report
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("changed while it was being saved")));
        } else {
            let outcome = stage_text_without_paste(
                &mut clipboard,
                "dictated text",
                restore_plan(&quiet_gate()),
                ClipboardPolicy::RestorePrevious,
            )
            .unwrap();
            assert_eq!(outcome, StageOutcome::CopiedOnly);
        }
        assert_eq!(clipboard.text(), Some("dictated text"));
    }
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

#[derive(Clone)]
struct MockRestoreGate {
    events: Rc<RefCell<Vec<String>>>,
    after_sequence: u32,
    timeout: bool,
    confirmation_override: Option<PasteConfirmation>,
    record_baseline: bool,
    on_wait: Option<Rc<dyn Fn()>>,
    on_confirmation: Option<Rc<dyn Fn()>>,
}

impl MockRestoreGate {
    fn new(events: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            events,
            after_sequence: 11,
            timeout: false,
            confirmation_override: None,
            record_baseline: false,
            on_wait: None,
            on_confirmation: None,
        }
    }

    fn timeout(mut self) -> Self {
        self.timeout = true;
        self
    }

    fn without_sequence_advance(mut self) -> Self {
        self.after_sequence = 10;
        self
    }

    /// Make `await_paste_confirmation` return `confirmation` directly
    /// instead of falling through to the trait's default (sleep +
    /// `wait_before_restore`) behavior.
    fn confirmation(mut self, confirmation: PasteConfirmation) -> Self {
        self.confirmation_override = Some(confirmation);
        self
    }

    fn record_baseline(mut self) -> Self {
        self.record_baseline = true;
        self
    }

    fn on_wait(mut self, hook: impl Fn() + 'static) -> Self {
        self.on_wait = Some(Rc::new(hook));
        self
    }

    fn on_confirmation(mut self, hook: impl Fn() + 'static) -> Self {
        self.on_confirmation = Some(Rc::new(hook));
        self
    }
}

impl ClipboardRestoreGate for MockRestoreGate {
    fn before_transcript_write(&self) -> ClipboardWriteSnapshot {
        self.events.borrow_mut().push("before-write:10".to_string());
        ClipboardWriteSnapshot { sequence: Some(10) }
    }

    fn after_transcript_write(&self, before: ClipboardWriteSnapshot) -> ClipboardWriteToken {
        self.events
            .borrow_mut()
            .push(format!("after-write:{}", self.after_sequence));
        ClipboardWriteToken {
            before_sequence: before.sequence,
            after_sequence: Some(self.after_sequence),
        }
    }

    fn wait_before_restore(&self, token: ClipboardWriteToken, _fallback_delay: Duration) {
        if let Some(hook) = &self.on_wait {
            hook();
        }
        self.events.borrow_mut().push(format!(
            "{}:{}->{}",
            if self.timeout { "wait-timeout" } else { "wait" },
            token.before_sequence.unwrap_or_default(),
            token.after_sequence.unwrap_or_default()
        ));
    }

    fn capture_paste_baseline(&self, _focus: Option<&FocusSnapshot>) -> Option<PasteTargetValue> {
        if self.record_baseline {
            self.events.borrow_mut().push("baseline".to_string());
        }
        None
    }

    fn await_paste_confirmation(
        &self,
        token: ClipboardWriteToken,
        fallback_delay: Duration,
        paste_consume_delay: Duration,
        _ctx: &PasteConfirmationContext<'_>,
    ) -> PasteConfirmation {
        if let Some(hook) = &self.on_confirmation {
            hook();
        }
        match self.confirmation_override {
            Some(confirmation) => {
                self.events
                    .borrow_mut()
                    .push(format!("confirm:{}", confirmation_kind(confirmation)));
                confirmation
            }
            // No override configured: reproduce the exact trait-default
            // behavior so every test that does not opt into a confirmation
            // override is unaffected by this method's addition.
            None => default_paste_confirmation(self, token, fallback_delay, paste_consume_delay),
        }
    }
}

fn confirmation_kind(confirmation: PasteConfirmation) -> &'static str {
    match confirmation {
        PasteConfirmation::Confirmed { kind, .. }
        | PasteConfirmation::Unverified { kind, .. }
        | PasteConfirmation::UnverifiedFocusLost { kind, .. }
        | PasteConfirmation::NoEvidence { kind, .. } => kind,
    }
}

fn restore_plan<'a, G: ClipboardRestoreGate + ?Sized>(gate: &'a G) -> ClipboardRestorePlan<'a, G> {
    ClipboardRestorePlan::new(Duration::ZERO, Duration::ZERO, gate)
}

/// A [`MockRestoreGate`] with no confirmation override and a throwaway event
/// sink, for tests that exercise generic clipboard-swap sequencing and don't
/// care about the paste-acknowledgement tier reached.
///
/// Deliberately *not* [`PlatformClipboardRestoreGate::fallback()`]: on
/// macOS, that type's `await_paste_confirmation` override performs real
/// `AXValue` polling (see `daemon::macos::pasteboard`). With `focus: None`
/// (as these tests pass), that always degrades to a 1.5s
/// [`crate::daemon::macos::pasteboard::UNVERIFIED_GRACE`] sleep and
/// [`PasteConfirmation::Unverified`], which would make swap-mechanics tests
/// slow, flaky across platforms, and dependent on Accessibility permissions
/// they don't hold. This gate always resolves through
/// [`default_paste_confirmation`] instead, matching the pre-acknowledgement
/// behavior these tests were written against, on every platform.
fn quiet_gate() -> MockRestoreGate {
    MockRestoreGate::new(Rc::new(RefCell::new(Vec::new())))
}

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
    /// `paste_with_clipboard_swap_guarded` (inject.rs ~978-1070), an `Err`
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
        // `stage_text_without_paste`'s `KeepTranscript` branch (inject.rs
        // ~1255-1260), which sets the transcript and returns without ever
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

// These transaction-order checks retain all staging, focus, dispatch and
// restoration events. Race tests below separately verify the read-back guard.
fn transaction_events(events: &Rc<RefCell<Vec<String>>>) -> Vec<String> {
    events
        .borrow()
        .iter()
        .filter(|event| event.as_str() != "guard-read")
        .cloned()
        .collect()
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

fn competing_clipboard_payloads() -> [MockClipboardContent; 3] {
    [
        MockClipboardContent::Text("new copy".to_string()),
        MockClipboardContent::Image {
            width: 1,
            height: 1,
            bytes: vec![1, 2, 3, 255],
        },
        // A new rich payload can have the same plain text as our transcript.
        // Text comparison alone must not authorize pasting/restoring it.
        MockClipboardContent::Html {
            html: "<b>dictated text</b>".to_string(),
            alt_text: Some("dictated text".to_string()),
        },
    ]
}

#[test]
fn competing_clipboard_during_final_focus_guard_skips_chord_and_preserves_payload() {
    for policy in [
        ClipboardPolicy::RestorePrevious,
        ClipboardPolicy::KeepTranscript,
    ] {
        for competing in competing_clipboard_payloads() {
            let mut clipboard = MockClipboard::new("old clipboard");
            let pending = Rc::clone(&clipboard.pending_external_write);
            let mut guard_calls = 0;
            let report = paste_with_clipboard_swap_guarded(
                &mut clipboard,
                "dictated text",
                PasteMode::Standard,
                || true,
                || panic!("a competing clipboard must never be pasted"),
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
            .expect("clipboard competition is recoverable without fallback");
            assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
            assert!(!report.telemetry.paste_event_posted);
            assert_eq!(report.telemetry.clipboard_restored, None);
            assert_eq!(clipboard.content, competing);
            assert_eq!(
                clipboard
                    .events
                    .borrow()
                    .iter()
                    .filter(|event| event.starts_with("set:"))
                    .count(),
                1
            );
        }
    }
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

#[cfg(target_os = "linux")]
#[test]
fn clipboard_manager_handoffs_keep_identical_text_pasteable_and_restorable() {
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
        assert_eq!(report.telemetry.clipboard_restored, Some(true));
        assert_eq!(clipboard.text(), Some("old clipboard"));
        if handoff_before_paste {
            assert_eq!(
                guards, 3,
                "focus must be rechecked after reading the new owner"
            );
        }
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

#[test]
fn competing_clipboard_during_skipped_modifier_dispatch_is_preserved() {
    let mut clipboard = MockClipboard::new("old clipboard");
    let pending = Rc::clone(&clipboard.pending_external_write);
    let report = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        PasteMode::Standard,
        || true,
        || {
            *pending.borrow_mut() = Some(MockClipboardContent::Text("new copy".to_string()));
            Ok(PasteDispatch::SkippedUnsafeModifiers)
        },
        Duration::ZERO,
        restore_plan(&quiet_gate()),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .unwrap();
    assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
    assert!(!report.telemetry.paste_event_posted);
    assert_eq!(report.telemetry.clipboard_restored, None);
    assert_eq!(clipboard.text(), Some("new copy"));
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
    assert_eq!(
        clipboard_guard::platform_change_stamp().unwrap(),
        u64::from(owner)
    );
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

#[test]
fn unreadable_clipboard_stamp_withholds_chord_but_leaves_transcript() {
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
            || panic!("unreadable stamp must prevent dispatch"),
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
        // Without a readable stamp the staged text cannot be verified, so no
        // chord is sent, but the transcript stays on the clipboard.
        assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
        assert_eq!(report.telemetry.clipboard_restored, None);
        assert_eq!(clipboard.text(), Some("dictated text"));
        assert!(!report.telemetry.paste_event_posted);
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
    for paste in [false, true] {
        let mut clipboard = MockClipboard::new("old clipboard");
        clipboard.text_unavailable.set(true);
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
            .expect("an unreadable previous clipboard must not fail the dictation");
            assert!(
                dispatched,
                "an unreadable snapshot must not withhold the paste"
            );
            assert_eq!(report.outcome, PasteOutcome::Pasted);
            assert_eq!(report.telemetry.clipboard_restored, Some(false));
            assert!(
                report
                    .diagnostic
                    .as_deref()
                    .is_some_and(|diagnostic| diagnostic.contains("clipboard text unavailable")),
                "missing text-read failure: {:?}",
                report.diagnostic
            );
        } else {
            let outcome = stage_text_without_paste(
                &mut clipboard,
                "dictated text",
                restore_plan(&quiet_gate()),
                ClipboardPolicy::RestorePrevious,
            )
            .expect("an unreadable previous clipboard must not fail staging");
            assert_eq!(outcome, StageOutcome::CopiedOnly);
        }
        assert_eq!(clipboard.text(), Some("dictated text"));
    }
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

#[test]
fn clipboard_stamp_recheck_detects_same_plain_payload_changed_during_read() {
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
        || panic!("a payload changed during read must never be pasted"),
        Duration::ZERO,
        restore_plan(&quiet_gate()),
        ClipboardPolicy::RestorePrevious,
        None,
        || Ok(true),
    )
    .expect("read-back race must preserve competing data");
    assert_eq!(report.outcome, PasteOutcome::ClipboardChanged);
    assert_eq!(clipboard.content, competing);
    assert_eq!(report.telemetry.clipboard_restored, None);
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
