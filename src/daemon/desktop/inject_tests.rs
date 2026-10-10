//! Shared clipboard and restore-gate fakes for the clipboard, paste, and XTest cleanup tests.

use super::*;
use crate::daemon::desktop::clipboard_restore::{
    default_paste_confirmation, ClipboardRestoreGate, ClipboardWriteSnapshot, ClipboardWriteToken,
    PasteConfirmation, PasteConfirmationContext, PasteTargetValue,
};
use arboard::ImageData;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

#[path = "inject_acknowledgement_tests.rs"]
mod acknowledgement;
#[path = "inject_restage_tests.rs"]
mod restage;
#[path = "inject_sequence_tests.rs"]
mod sequence;
#[path = "inject_snapshot_tests.rs"]
mod snapshot;

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
    fail_next_set: Rc<Cell<bool>>,
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
            fail_next_set: Rc::new(Cell::new(false)),
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

    fn fail_next_set(self) -> Self {
        self.fail_next_set.set(true);
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
        if self.fail_next_set.replace(false) {
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

#[derive(Clone)]
struct MockRestoreGate {
    events: Rc<RefCell<Vec<String>>>,
    after_sequence: u32,
    timeout: bool,
    confirmation_override: Option<PasteConfirmation>,
    record_baseline: bool,
    on_baseline: Option<Rc<dyn Fn()>>,
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
            on_baseline: None,
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

    fn on_baseline(mut self, hook: impl Fn() + 'static) -> Self {
        self.on_baseline = Some(Rc::new(hook));
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
        if let Some(hook) = &self.on_baseline {
            hook();
        }
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

fn competing_clipboard_payloads() -> Vec<MockClipboardContent> {
    let mut payloads = vec![
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
    ];
    if !cfg!(target_os = "linux") {
        // macOS/Windows generations track writes: an identical-text republish
        // competes with our write, while X11 manager handoffs are tested below.
        payloads.push(MockClipboardContent::Text("dictated text".to_string()));
    }
    payloads
}
