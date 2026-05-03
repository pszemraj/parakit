//! Unit tests for clipboard, paste, and XTest cleanup helpers.

use super::*;
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
    fail_on_set: Option<String>,
}

impl MockClipboard {
    fn new(text: impl Into<String>) -> Self {
        Self {
            content: MockClipboardContent::Text(text.into()),
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn empty() -> Self {
        Self {
            content: MockClipboardContent::Empty,
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn html(html: impl Into<String>, alt_text: Option<&str>) -> Self {
        Self {
            content: MockClipboardContent::Html {
                html: html.into(),
                alt_text: alt_text.map(str::to_owned),
            },
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn file_list(paths: &[&str]) -> Self {
        Self {
            content: MockClipboardContent::FileList(paths.iter().map(PathBuf::from).collect()),
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn image() -> Self {
        Self {
            content: MockClipboardContent::Image {
                width: 2,
                height: 1,
                bytes: vec![1, 2, 3, 4],
            },
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn unsupported() -> Self {
        Self {
            content: MockClipboardContent::Unsupported,
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn fail_next_set(mut self) -> Self {
        self.fail_next_set = true;
        self
    }

    fn fail_on_set(mut self, text: impl Into<String>) -> Self {
        self.fail_on_set = Some(text.into());
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
        if let Some(text) = text {
            if self.fail_on_set.as_deref() == Some(text) {
                anyhow::bail!("clipboard write failed for {text}");
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
            MockClipboardContent::Html { html, .. } => Ok(html.clone()),
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
}

#[test]
fn paste_mode_labels_are_stable() {
    assert_eq!(PasteMode::Terminal.label(), "terminal");
    assert_eq!(PasteMode::Standard.label(), "standard");
    assert_eq!(PasteMode::Direct.label(), "direct");
}

#[test]
fn linux_standard_paste_does_not_need_enigo() {
    #[cfg(target_os = "linux")]
    assert!(!insertion_needs_enigo(PasteMode::Standard));
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
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps(), &[9, 8])
        .expect_err("primary failure should be reported");

    assert!(format!("{err:#}").contains("primary failure"));
    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (3, true),
            (2, false),
            (1, false),
            (9, false),
            (8, false)
        ]
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
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps(), &[9, 8])
        .expect_err("primary and cleanup failures should be reported");
    let message = format!("{err:#}");
    assert!(message.contains("primary failure"));
    assert!(message.contains("cleanup while releasing pressed XTest keys failed"));
    assert!(message.contains("cleanup failure"));
}

#[cfg(target_os = "linux")]
#[test]
fn xtest_success_flushes_modifiers_after_chord() {
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

    send_x11_key_steps(&mut sink, &steps, &[1, 2, 9]).expect("paste chord should succeed");

    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false),
            (1, false),
            (2, false),
            (9, false)
        ]
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn direct_mode_has_no_paste_modifiers() {
    assert!(paste_modifiers(PasteMode::Direct).is_empty());
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
struct MockPasteShortcutSink {
    events: Vec<String>,
    fail_press: Option<&'static str>,
    fail_release: Option<&'static str>,
    fail_paste: bool,
}

#[cfg(not(target_os = "linux"))]
impl PasteShortcutSink for MockPasteShortcutSink {
    fn key(&mut self, key: Key, direction: Direction) -> Result<()> {
        let key = mock_key_label(key);
        let direction = mock_direction_label(direction);
        self.events.push(format!("{direction}:{key}"));
        if direction == "press" && self.fail_press == Some(key) {
            anyhow::bail!("press failed for {key}");
        }
        if direction == "release" && self.fail_release == Some(key) {
            anyhow::bail!("release failed for {key}");
        }
        Ok(())
    }

    fn paste_key(&mut self) -> Result<()> {
        self.events.push("paste".to_string());
        if self.fail_paste {
            anyhow::bail!("paste failed");
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn mock_key_label(key: Key) -> &'static str {
    match key {
        Key::Control => "control",
        Key::Shift => "shift",
        Key::Meta => "meta",
        _ => "other",
    }
}

#[cfg(not(target_os = "linux"))]
fn mock_direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Press => "press",
        Direction::Release => "release",
        Direction::Click => "click",
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn paste_shortcut_releases_only_successfully_pressed_modifiers() {
    let mut sink = MockPasteShortcutSink {
        fail_press: Some("shift"),
        ..MockPasteShortcutSink::default()
    };
    let err = send_paste_shortcut_with_cleanup(&mut sink, &[Key::Control, Key::Shift])
        .expect_err("failed modifier press should be reported");

    assert!(format!("{err:#}").contains("press failed for shift"));
    assert_eq!(
        sink.events,
        vec!["press:control", "press:shift", "release:control"]
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn paste_shortcut_reports_primary_and_modifier_cleanup_errors() {
    let mut sink = MockPasteShortcutSink {
        fail_release: Some("control"),
        fail_paste: true,
        ..MockPasteShortcutSink::default()
    };
    let err = send_paste_shortcut_with_cleanup(&mut sink, &[Key::Control])
        .expect_err("paste and cleanup failures should be reported");
    let message = format!("{err:#}");

    assert!(message.contains("paste failed"));
    assert!(message.contains("paste modifier cleanup also failed"));
    assert!(message.contains("release failed for control"));
    assert_eq!(
        sink.events,
        vec!["press:control", "paste", "release:control"]
    );
}

struct ClipboardCase {
    name: &'static str,
    initial: Option<&'static str>,
    transcript: &'static str,
    guard_allows: bool,
    paste_error: Option<&'static str>,
    fail_next_set: bool,
    fail_on_set: Option<&'static str>,
    expected_text: Option<&'static str>,
    expected_events: &'static [&'static str],
    expected_outcome: Option<PasteOutcome>,
    error_contains: Option<&'static str>,
}

#[test]
fn clipboard_swap_cases_are_stable() {
    let cases = [
        ClipboardCase {
            name: "failed paste restores previous clipboard by default",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: true,
            paste_error: Some("paste failed"),
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &[
                "read",
                "set:dictated text",
                "guard",
                "paste",
                "set:old clipboard",
            ],
            expected_outcome: None,
            error_contains: Some("paste failed"),
        },
        ClipboardCase {
            name: "successful paste restores previous clipboard",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: true,
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &[
                "read",
                "set:dictated text",
                "guard",
                "paste",
                "set:old clipboard",
            ],
            expected_outcome: Some(PasteOutcome::Pasted),
            error_contains: None,
        },
        ClipboardCase {
            name: "same clipboard text remains available after paste",
            initial: Some("dictated text"),
            transcript: "dictated text",
            guard_allows: true,
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("dictated text"),
            expected_events: &[
                "read",
                "set:dictated text",
                "guard",
                "paste",
                "set:dictated text",
            ],
            expected_outcome: Some(PasteOutcome::Pasted),
            error_contains: None,
        },
        ClipboardCase {
            name: "guard blocks paste and restores previous clipboard",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: false,
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &["read", "set:dictated text", "guard", "set:old clipboard"],
            expected_outcome: Some(PasteOutcome::Blocked),
            error_contains: None,
        },
        ClipboardCase {
            name: "empty text does not touch clipboard or paste",
            initial: None,
            transcript: "",
            guard_allows: true,
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: None,
            expected_events: &[],
            expected_outcome: Some(PasteOutcome::Pasted),
            error_contains: None,
        },
        ClipboardCase {
            name: "transcript clipboard write failure does not paste or restore",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            guard_allows: true,
            paste_error: None,
            fail_next_set: true,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &["read", "set:dictated text"],
            expected_outcome: None,
            error_contains: Some("could not copy transcript to clipboard"),
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
        if let Some(text) = case.fail_on_set {
            clipboard = clipboard.fail_on_set(text);
        }

        let events = clipboard.events();
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            case.transcript,
            || {
                events.borrow_mut().push("paste".to_string());
                match case.paste_error {
                    Some(message) => Err(anyhow::anyhow!("{message}")),
                    None => Ok(()),
                }
            },
            Duration::ZERO,
            Duration::ZERO,
            ClipboardPolicy::RestorePrevious,
            || {
                events.borrow_mut().push("guard".to_string());
                Ok(case.guard_allows)
            },
        );

        match case.error_contains {
            Some(fragment) => {
                let err = result.expect_err(case.name);
                assert!(format!("{err:#}").contains(fragment), "{}", case.name);
            }
            None => assert_eq!(result.expect(case.name), case.expected_outcome.unwrap()),
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
fn clipboard_keep_transcript_policy_leaves_text_after_paste_and_guard_block() {
    for guard_allows in [true, false] {
        let mut clipboard = MockClipboard::new("old clipboard");
        let events = clipboard.events();
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
            ClipboardPolicy::KeepTranscript,
            || {
                events.borrow_mut().push("guard".to_string());
                Ok(guard_allows)
            },
        )
        .expect("clipboard keep policy should not fail");

        assert_eq!(clipboard.text(), Some("dictated text"));
        assert_eq!(
            result,
            if guard_allows {
                PasteOutcome::Pasted
            } else {
                PasteOutcome::CopiedOnly
            }
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
                "set:dictated text".to_string(),
                "guard".to_string(),
                "paste".to_string(),
                "set-image:2x1:4".to_string(),
            ],
        ),
    ];

    for (name, mut clipboard, expected_content, expected_events) in cases {
        let events = clipboard.events();
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
            ClipboardPolicy::RestorePrevious,
            || {
                events.borrow_mut().push("guard".to_string());
                Ok(true)
            },
        )
        .expect(name);

        assert_eq!(result, PasteOutcome::Pasted, "{name}");
        assert_eq!(clipboard.content, expected_content, "{name}");
        assert_eq!(events.borrow().as_slice(), expected_events, "{name}");
    }
}

#[test]
fn unsupported_previous_clipboard_is_cleared_when_restore_is_impossible() {
    let mut clipboard = MockClipboard::unsupported();
    let events = clipboard.events();
    let result = paste_with_clipboard_swap_guarded(
        &mut clipboard,
        "dictated text",
        || {
            events.borrow_mut().push("paste".to_string());
            Ok(())
        },
        Duration::ZERO,
        Duration::ZERO,
        ClipboardPolicy::RestorePrevious,
        || {
            events.borrow_mut().push("guard".to_string());
            Ok(false)
        },
    )
    .expect("unsupported clipboard should clear staged transcript on guard block");

    assert_eq!(result, PasteOutcome::Blocked);
    assert_eq!(clipboard.text(), Some(""));
    assert_eq!(
        events.borrow().as_slice(),
        ["read", "set:dictated text", "guard", "set:"]
    );
}
