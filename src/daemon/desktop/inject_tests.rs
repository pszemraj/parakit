//! Unit tests for clipboard, paste, and XTest cleanup helpers.

use super::*;
use crate::daemon::desktop::clipboard_restore::default_paste_confirmation;
use std::borrow::Cow;
use std::cell::RefCell;
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
    events: Rc<RefCell<Vec<String>>>,
    fail_next_set: bool,
    fail_set_matching: Option<String>,
}

impl MockClipboard {
    fn with_content(content: MockClipboardContent) -> Self {
        Self {
            content,
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_set_matching: None,
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
}

impl ClipboardStore for MockClipboard {
    fn get_text(&mut self) -> Result<String> {
        self.events.borrow_mut().push("read".to_string());
        match &self.content {
            MockClipboardContent::Text(text) => Ok(text.clone()),
            MockClipboardContent::Html {
                alt_text: Some(text),
                ..
            } => Ok(text.clone()),
            MockClipboardContent::HtmlImage {
                alt_text: Some(text),
                ..
            } => Ok(text.clone()),
            _ => anyhow::bail!("clipboard is not text"),
        }
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        self.events.borrow_mut().push(format!("set:{text}"));
        self.fail_set_if_needed(Some(&text))?;
        self.content = MockClipboardContent::Text(text);
        Ok(())
    }

    fn get_html(&mut self) -> Result<String> {
        match &self.content {
            MockClipboardContent::Html { html, .. }
            | MockClipboardContent::HtmlImage { html, .. } => Ok(html.clone()),
            _ => anyhow::bail!("clipboard is not HTML"),
        }
    }

    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()> {
        self.events.borrow_mut().push(format!(
            "set-html:{html}:{}",
            alt_text.as_deref().unwrap_or("")
        ));
        self.fail_set_if_needed(None)?;
        self.content = MockClipboardContent::Html { html, alt_text };
        Ok(())
    }

    fn get_file_list(&mut self) -> Result<Vec<PathBuf>> {
        match &self.content {
            MockClipboardContent::FileList(paths) => Ok(paths.clone()),
            _ => anyhow::bail!("clipboard is not a file list"),
        }
    }

    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()> {
        self.events
            .borrow_mut()
            .push(format!("set-files:{}", files.len()));
        self.fail_set_if_needed(None)?;
        self.content = MockClipboardContent::FileList(files.to_vec());
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
            _ => anyhow::bail!("clipboard is not an image"),
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
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.events.borrow_mut().push("clear".to_string());
        self.content = MockClipboardContent::Empty;
        Ok(())
    }
}

#[derive(Clone)]
struct MockRestoreGate {
    events: Rc<RefCell<Vec<String>>>,
    after_sequence: u32,
    timeout: bool,
    confirmation_override: Option<PasteConfirmation>,
    record_baseline: bool,
}

impl MockRestoreGate {
    fn new(events: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            events,
            after_sequence: 11,
            timeout: false,
            confirmation_override: None,
            record_baseline: false,
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

#[cfg(target_os = "linux")]
#[test]
fn linux_xtest_paste_chord_steps_are_ordered() {
    assert_eq!(
        linux_paste_chord_steps(PasteMode::Standard),
        vec![
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, false),
        ]
    );
    assert_eq!(
        linux_paste_chord_steps(PasteMode::Terminal),
        vec![
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::SHIFT_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(crate::daemon::x11::SHIFT_L_KEYSYM, false),
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, false),
        ]
    );
}

#[cfg(target_os = "linux")]
fn x11_key_step(keysym: u32, press: bool) -> X11KeyStep {
    X11KeyStep { keysym, press }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct MockX11KeySink {
    events: Vec<(u8, bool)>,
    fail_on: Option<(u8, bool)>,
    fail_cleanup_on: Option<u8>,
    flushes: usize,
}

#[cfg(target_os = "linux")]
impl X11KeySink for MockX11KeySink {
    fn key(&mut self, keycode: u8, press: bool) -> Result<()> {
        self.events.push((keycode, press));
        if self.fail_on == Some((keycode, press)) {
            anyhow::bail!("primary failure {keycode}:{press}");
        }
        if !press && self.fail_cleanup_on == Some(keycode) {
            anyhow::bail!("cleanup failure {keycode}");
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn three_pressed_key_steps() -> [ResolvedX11KeyStep; 3] {
    [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: true,
        },
    ]
}

#[cfg(target_os = "linux")]
#[test]
fn xtest_cleanup_releases_pressed_keys_after_primary_error() {
    let mut sink = MockX11KeySink {
        fail_on: Some((3, true)),
        ..MockX11KeySink::default()
    };
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps())
        .expect_err("primary failure should be reported");

    assert!(format!("{err:#}").contains("primary failure"));
    assert_eq!(
        sink.events,
        vec![(1, true), (2, true), (3, true), (2, false), (1, false)]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn xtest_cleanup_reports_primary_and_cleanup_errors() {
    let mut sink = MockX11KeySink {
        fail_on: Some((3, true)),
        fail_cleanup_on: Some(2),
        ..MockX11KeySink::default()
    };
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps())
        .expect_err("primary and cleanup failures should be reported");
    let message = format!("{err:#}");
    assert!(message.contains("primary failure"));
    assert!(message.contains("cleanup while releasing pressed XTest keys failed"));
    assert!(message.contains("cleanup failure"));
}

#[cfg(target_os = "linux")]
#[test]
fn xtest_success_releases_only_chord_keys() {
    let mut sink = MockX11KeySink::default();
    let steps = [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 1,
            press: false,
        },
    ];

    send_x11_key_steps(&mut sink, &steps).expect("paste chord should succeed");

    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false)
        ]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn xtest_paste_chord_success_flushes_all_cleanup_modifiers() {
    let mut sink = MockX11KeySink::default();
    let steps = [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 1,
            press: false,
        },
    ];

    send_x11_paste_chord_with_modifier_flush(&mut sink, &steps, &[1, 3, 4])
        .expect("paste chord should succeed");

    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (2, false),
            (1, false),
            (1, false),
            (3, false),
            (4, false)
        ]
    );
    assert_eq!(sink.flushes, 2);
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
                        report.clipboard_restored,
                        Some(expected_restored),
                        "{}",
                        case.name
                    );
                }
            }
        }
        assert_eq!(clipboard.text(), case.expected_text, "{}", case.name);
        assert_eq!(
            events.borrow().as_slice(),
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
        events.borrow().as_slice(),
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
    assert!(!result.paste_event_posted);
    assert_eq!(result.clipboard_restored, Some(false));
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(
        events.borrow().as_slice(),
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
    assert!(!result.paste_event_posted);
    assert_eq!(result.acknowledgement_kind, "not_applicable");
    assert_eq!(result.clipboard_restored, Some(false));
    assert_eq!(clipboard.text(), Some("dictated text"));
    assert_eq!(
        events.borrow().as_slice(),
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
                "read",
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
            events
                .borrow()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
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
        assert_eq!(events.borrow().as_slice(), expected_events, "{name}");
    }
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
        events.borrow().as_slice(),
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
            result.acknowledgement_kind, case.expected_acknowledgement_kind,
            "{}",
            case.name
        );
        assert!(
            result.acknowledgement_ms.is_some(),
            "{}: acknowledgement_ms should be populated",
            case.name
        );
        assert_eq!(
            result.clipboard_restored, case.expected_clipboard_restored,
            "{}",
            case.name
        );
        assert!(result.paste_event_posted, "{}", case.name);
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
        assert!(result.paste_event_posted, "{}", case.name);
        assert_eq!(
            result.acknowledgement_kind, case.expected_acknowledgement_kind,
            "{}",
            case.name
        );
        // The restore attempt failed, so the transcript is still on the
        // clipboard rather than the previous "old clipboard" contents; this must
        // read as `Some(false)`, not silently as `Some(true)` or an `Err` that
        // would discard the already-landed paste.
        assert_eq!(result.clipboard_restored, Some(false), "{}", case.name);
        assert_eq!(clipboard.text(), Some("dictated text"), "{}", case.name);
    }
}
