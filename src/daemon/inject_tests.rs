//! Unit tests for clipboard, paste, and XTest cleanup helpers.

use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug)]
struct MockClipboard {
    text: Option<String>,
    events: Rc<RefCell<Vec<String>>>,
    fail_next_set: bool,
    fail_on_set: Option<String>,
}

impl MockClipboard {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            events: Rc::new(RefCell::new(Vec::new())),
            fail_next_set: false,
            fail_on_set: None,
        }
    }

    fn empty() -> Self {
        Self {
            text: None,
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
}

impl TextClipboard for MockClipboard {
    fn get_text(&mut self) -> Result<String> {
        self.events.borrow_mut().push("read".to_string());
        self.text
            .clone()
            .ok_or_else(|| anyhow::anyhow!("clipboard is not text"))
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        self.events.borrow_mut().push(format!("set:{text}"));
        if self.fail_next_set {
            self.fail_next_set = false;
            anyhow::bail!("clipboard write failed");
        }
        if self.fail_on_set.as_deref() == Some(text.as_str()) {
            anyhow::bail!("clipboard write failed for {text}");
        }
        self.text = Some(text);
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
            x11_key_step(CONTROL_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(CONTROL_L_KEYSYM, false),
        ]
    );
    assert_eq!(
        linux_paste_chord_steps(PasteMode::Terminal),
        vec![
            x11_key_step(CONTROL_L_KEYSYM, true),
            x11_key_step(SHIFT_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(SHIFT_L_KEYSYM, false),
            x11_key_step(CONTROL_L_KEYSYM, false),
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

#[cfg(not(target_os = "linux"))]
#[test]
fn direct_mode_has_no_paste_modifiers() {
    assert!(paste_modifiers(PasteMode::Direct).is_empty());
}

struct ClipboardCase {
    name: &'static str,
    initial: Option<&'static str>,
    transcript: &'static str,
    paste_error: Option<&'static str>,
    fail_next_set: bool,
    fail_on_set: Option<&'static str>,
    expected_text: Option<&'static str>,
    expected_events: &'static [&'static str],
    error_contains: Option<&'static str>,
}

#[test]
fn clipboard_swap_cases_are_stable() {
    let cases = [
        ClipboardCase {
            name: "failed paste restores previous clipboard",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            paste_error: Some("paste failed"),
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &["read", "set:dictated text", "paste", "set:old clipboard"],
            error_contains: Some("paste failed"),
        },
        ClipboardCase {
            name: "successful paste restores previous clipboard",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &["read", "set:dictated text", "paste", "set:old clipboard"],
            error_contains: None,
        },
        ClipboardCase {
            name: "same clipboard text is not rewritten after paste",
            initial: Some("dictated text"),
            transcript: "dictated text",
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: Some("dictated text"),
            expected_events: &["read", "set:dictated text", "paste"],
            error_contains: None,
        },
        ClipboardCase {
            name: "empty text does not touch clipboard or paste",
            initial: None,
            transcript: "",
            paste_error: None,
            fail_next_set: false,
            fail_on_set: None,
            expected_text: None,
            expected_events: &[],
            error_contains: None,
        },
        ClipboardCase {
            name: "transcript clipboard write failure does not paste or restore",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            paste_error: None,
            fail_next_set: true,
            fail_on_set: None,
            expected_text: Some("old clipboard"),
            expected_events: &["read", "set:dictated text"],
            error_contains: Some("could not copy transcript to clipboard"),
        },
        ClipboardCase {
            name: "restore failure is reported after successful paste",
            initial: Some("old clipboard"),
            transcript: "dictated text",
            paste_error: None,
            fail_next_set: false,
            fail_on_set: Some("old clipboard"),
            expected_text: Some("dictated text"),
            expected_events: &["read", "set:dictated text", "paste", "set:old clipboard"],
            error_contains: Some("could not restore previous clipboard text"),
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
        let result = paste_with_clipboard_swap(
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
        );

        match case.error_contains {
            Some(fragment) => {
                let err = result.expect_err(case.name);
                assert!(format!("{err:#}").contains(fragment), "{}", case.name);
            }
            None => result.expect(case.name),
        }
        assert_eq!(
            clipboard.text.as_deref(),
            case.expected_text,
            "{}",
            case.name
        );
        assert_eq!(
            events.borrow().as_slice(),
            case.expected_events,
            "{}",
            case.name
        );
    }
}
