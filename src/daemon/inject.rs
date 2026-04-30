//! Insert text at the cursor position.
//!
//! Batch mode uses the clipboard plus the platform paste shortcut so the final
//! transcript appears as a single insertion. Streaming mode still uses
//! `enigo::Keyboard::text()` for partial chunks, which:
//!   - Windows: synthesizes Unicode keystrokes via `SendInput` with
//!     `KEYEVENTF_UNICODE`. Works for any character; no keyboard layout
//!     translation required.
//!   - Linux X11: uses `XTestFakeKeyEvent` plus a temporary keymap remap
//!     for non-keyboard characters. Works for ASCII/Latin reliably; some
//!     emoji or rare scripts may not pass through cleanly.
//!   - Linux Wayland: limited and depends on the compositor. Most do not
//!     allow synthetic key events from a regular client. Use X11.
//!   - macOS: synthesizes via the CGEvent API. Requires the launcher to
//!     be granted "Input Monitoring" + "Accessibility" permissions.

use anyhow::{Context, Result};
use arboard::Clipboard;
use clap::ValueEnum;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::{
    thread,
    time::{Duration, Instant},
};

/// Paste shortcut style for batch transcript insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum PasteMode {
    /// Terminal-friendly paste: `Ctrl+Shift+V` on Linux/Windows, `Cmd+V` on macOS.
    Terminal,
    /// GUI-app paste: `Ctrl+V` on Linux/Windows, `Cmd+V` on macOS.
    Standard,
    /// Type text directly without using the clipboard.
    Direct,
}

impl PasteMode {
    /// Return the short label used in verbose startup output.
    ///
    /// # Returns
    ///
    /// A stable lowercase mode label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Standard => "standard",
            Self::Direct => "direct",
        }
    }
}

/// Check whether the configured insertion path can be initialized.
///
/// # Arguments
///
/// * `mode` - Insertion mode to probe.
///
/// # Returns
///
/// `Ok(())` when the required insertion resources are available.
///
/// # Errors
///
/// Returns an error if the keyboard, clipboard, or platform paste support is
/// unavailable.
pub(crate) fn preflight(mode: PasteMode) -> Result<()> {
    let _keyboard = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?;
    if mode != PasteMode::Direct {
        let _clipboard = Clipboard::new().context("could not open system clipboard")?;
        platform_paste_preflight()?;
    }
    Ok(())
}

/// Exercise the configured insertion backend without inserting into the user's
/// focused application.
///
/// # Arguments
///
/// * `mode` - Insertion mode to validate.
///
/// # Returns
///
/// `Ok(())` when the backend can initialize and the platform smoke test passes.
///
/// # Errors
///
/// Returns an error when the keyboard, clipboard, or platform event backend
/// fails the validation.
pub(crate) fn smoke_test(mode: PasteMode) -> Result<()> {
    preflight(mode)?;
    if mode == PasteMode::Direct {
        return Ok(());
    }
    platform_paste_smoke_test(mode)
}

trait TextClipboard {
    /// Return the current text clipboard contents.
    ///
    /// # Returns
    ///
    /// The current text clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard is unavailable or does not hold text.
    fn get_text(&mut self) -> Result<String>;
    /// Replace the current text clipboard contents.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the new text.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_text(&mut self, text: String) -> Result<()>;
}

impl TextClipboard for Clipboard {
    fn get_text(&mut self) -> Result<String> {
        Clipboard::get_text(self).context("could not read system clipboard")
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        Clipboard::set_text(self, text).context("could not write system clipboard")
    }
}

/// Open a text insertion handle.
pub struct Injector {
    enigo: Enigo,
    clipboard: Option<Clipboard>,
}

impl Injector {
    /// Create an injector backed by the platform's keyboard API.
    ///
    /// # Returns
    ///
    /// A ready-to-use text inserter.
    ///
    /// # Errors
    ///
    /// Returns an error if `enigo` cannot initialize the platform keyboard
    /// backend.
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?;
        Ok(Self {
            enigo,
            clipboard: None,
        })
    }

    /// Paste the given text as one batch insertion at the focused cursor.
    ///
    /// The text clipboard is restored when the previous clipboard contents were
    /// also text. Non-text clipboard contents may be replaced by the transcript.
    ///
    /// # Arguments
    ///
    /// * `text` - Transcript text to insert.
    /// * `mode` - Paste shortcut style to send after updating the clipboard.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard was populated and the paste shortcut was
    /// accepted by the platform backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened, the transcript
    /// cannot be copied, or the platform backend rejects the paste shortcut.
    pub fn paste_text(&mut self, text: &str, mode: PasteMode) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if mode == PasteMode::Direct {
            return self.type_text(text);
        }

        let mut clipboard = match self.clipboard.take() {
            Some(clipboard) => clipboard,
            None => Clipboard::new().context("could not open system clipboard")?,
        };
        let result = paste_with_clipboard_swap(
            &mut clipboard,
            text,
            || self.paste_clipboard(mode),
            clipboard_settle_delay(),
            clipboard_restore_delay(),
        );
        self.clipboard = Some(clipboard);
        result
    }

    /// Type the given text as synthetic keystrokes at the focused cursor.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the text was accepted by the platform backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform backend rejects the synthetic typing
    /// request.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.enigo
            .text(text)
            .map_err(|e| anyhow::anyhow!("enigo type failed: {e:?}"))
            .context("could not type text at cursor")
    }

    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<()> {
        let modifiers = paste_modifiers(mode);
        let mut failure = None;
        for key in modifiers {
            if let Err(e) = self.enigo.key(*key, Direction::Press) {
                failure = Some(anyhow::anyhow!("enigo paste modifier press failed: {e:?}"));
                break;
            }
        }

        if failure.is_none() {
            failure = paste_key_click(&mut self.enigo)
                .err()
                .map(|e| anyhow::anyhow!("enigo paste key failed: {e:?}"));
        }

        for key in modifiers.iter().rev() {
            if let Err(e) = self.enigo.key(*key, Direction::Release) {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!("enigo paste modifier release failed: {e:?}")
                });
            }
        }

        match failure {
            Some(err) => Err(err).context("could not send paste shortcut"),
            None => Ok(()),
        }
    }
}

fn paste_with_clipboard_swap<C, P>(
    clipboard: &mut C,
    text: &str,
    mut paste: P,
    settle_delay: Duration,
    restore_delay: Duration,
) -> Result<()>
where
    C: TextClipboard,
    P: FnMut() -> Result<()>,
{
    if text.is_empty() {
        return Ok(());
    }

    let previous = clipboard
        .get_text()
        .ok()
        .filter(|previous| previous != text);
    clipboard
        .set_text(text.to_owned())
        .context("could not copy transcript to clipboard")?;

    sleep_if_nonzero(settle_delay);
    let paste_result = paste();

    let restore_result = if let Some(previous) = previous {
        sleep_if_nonzero(restore_delay);
        clipboard
            .set_text(previous)
            .map_err(|err| anyhow::anyhow!("could not restore previous clipboard text: {err:#}"))
    } else {
        Ok(())
    };

    match (paste_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(paste_err), Ok(())) => Err(paste_err),
        (Ok(()), Err(restore_err)) => Err(restore_err),
        (Err(paste_err), Err(restore_err)) => Err(paste_err.context(format!("{restore_err:#}"))),
    }
}

fn sleep_if_nonzero(delay: Duration) {
    if !delay.is_zero() {
        thread::sleep(delay);
    }
}

#[cfg(target_os = "macos")]
fn paste_modifiers(mode: PasteMode) -> &'static [Key] {
    match mode {
        PasteMode::Standard | PasteMode::Terminal => &[Key::Meta],
        PasteMode::Direct => &[],
    }
}

#[cfg(not(target_os = "macos"))]
fn paste_modifiers(mode: PasteMode) -> &'static [Key] {
    match mode {
        PasteMode::Standard => &[Key::Control],
        PasteMode::Terminal => &[Key::Control, Key::Shift],
        PasteMode::Direct => &[],
    }
}

#[cfg(target_os = "linux")]
fn platform_paste_preflight() -> Result<()> {
    linux_x11_xtest_preflight()
}

#[cfg(not(target_os = "linux"))]
fn platform_paste_preflight() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_paste_smoke_test(mode: PasteMode) -> Result<()> {
    linux_x11_paste_smoke_test(mode)
}

#[cfg(not(target_os = "linux"))]
fn platform_paste_smoke_test(_mode: PasteMode) -> Result<()> {
    Ok(())
}

fn clipboard_settle_delay() -> Duration {
    #[cfg(target_os = "linux")]
    {
        Duration::from_millis(150)
    }
    #[cfg(target_os = "macos")]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(50)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Duration::from_millis(100)
    }
}

fn clipboard_restore_delay() -> Duration {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(100)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Duration::from_millis(150)
    }
}

#[cfg(target_os = "linux")]
fn paste_key_click(_enigo: &mut Enigo) -> Result<()> {
    linux_x11_click_v_key()
}

#[cfg(target_os = "windows")]
fn paste_key_click(enigo: &mut Enigo) -> Result<()> {
    enigo
        .key(Key::V, Direction::Click)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

#[cfg(target_os = "macos")]
fn paste_key_click(enigo: &mut Enigo) -> Result<()> {
    const MACOS_V_KEYCODE: u16 = 9;
    enigo
        .raw(MACOS_V_KEYCODE, Direction::Click)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn paste_key_click(enigo: &mut Enigo) -> Result<()> {
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
}

#[cfg(target_os = "linux")]
fn linux_cached_v_keycode() -> Result<u8> {
    static V_KEYCODE: OnceLock<Result<u8, String>> = OnceLock::new();
    V_KEYCODE
        .get_or_init(|| {
            super::x11::keycode_for_keysym_on_default_display(super::x11::V_KEYSYM)
                .map_err(|err| format!("{err:#}"))
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

#[cfg(target_os = "linux")]
fn linux_x11_click_v_key() -> Result<()> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;
    use x11rb::rust_connection::RustConnection;

    let keycode = linux_cached_v_keycode()?;
    let (conn, screen_num) = RustConnection::connect(None).context("could not connect to X11")?;
    let root = super::x11::root_window(&conn, screen_num)?;
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, root, 0, 0, 0)
        .context("could not send XTest key press")?
        .check()
        .context("X11 rejected XTest key press")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, root, 0, 0, 0)
        .context("could not send XTest key release")?
        .check()
        .context("X11 rejected XTest key release")?;
    conn.flush().context("could not flush XTest paste key")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_x11_xtest_preflight() -> Result<()> {
    use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).context("could not connect to X11")?;
    conn.xtest_get_version(2, 2)
        .context("could not request XTest version")?
        .reply()
        .context("XTest extension is unavailable")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_x11_paste_smoke_test(mode: PasteMode) -> Result<()> {
    const XI_ALL_MASTER_DEVICES: u16 = 1;

    use x11rb::connection::Connection;
    use x11rb::protocol::xinput::{
        ConnectionExt as XinputConnectionExt, EventMask as XiEventMask, XIEventMask,
    };
    use x11rb::protocol::xproto::{
        ConnectionExt, CreateWindowAux, EventMask, InputFocus, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    let v_keycode = linux_cached_v_keycode()?;
    let (conn, screen_num) = RustConnection::connect(None).context("could not connect to X11")?;
    let screen = super::x11::screen(&conn, screen_num)?;
    let previous_focus = conn
        .get_input_focus()
        .context("could not request current X11 input focus")?
        .reply()
        .context("could not read current X11 input focus")?;
    let window = conn
        .generate_id()
        .context("could not allocate X11 smoke-test window id")?;
    let window_aux = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .override_redirect(1)
        .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
    let raw_key_mask = [XiEventMask {
        deviceid: XI_ALL_MASTER_DEVICES,
        mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE],
    }];

    conn.create_window(
        screen.root_depth,
        window,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &window_aux,
    )
    .context("could not create X11 smoke-test window")?
    .check()
    .context("X11 rejected smoke-test window creation")?;
    conn.map_window(window)
        .context("could not map X11 smoke-test window")?
        .check()
        .context("X11 rejected smoke-test window mapping")?;
    conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
        .context("could not focus X11 smoke-test window")?
        .check()
        .context("X11 rejected smoke-test focus change")?;
    conn.xinput_xi_select_events(screen.root, &raw_key_mask)
        .context("could not subscribe to XInput raw key events")?
        .check()
        .context("X11 rejected XInput raw key event subscription")?;
    conn.flush()
        .context("could not flush X11 smoke-test setup")?;
    linux_wait_for_focus(&conn, window)?;
    while conn
        .poll_for_event()
        .context("could not drain X11 smoke-test setup events")?
        .is_some()
    {}

    let smoke_result = (|| {
        let mut injector = Injector::new()?;
        injector
            .paste_clipboard(mode)
            .context("configured paste shortcut failed during smoke test")?;
        linux_wait_for_v_key_events(&conn, window, v_keycode)
    })();

    let cleanup_result = (|| {
        conn.set_input_focus(
            previous_focus.revert_to,
            previous_focus.focus,
            x11rb::CURRENT_TIME,
        )
        .context("could not restore previous X11 input focus")?
        .check()
        .context("X11 rejected previous focus restore")?;
        conn.destroy_window(window)
            .context("could not destroy X11 smoke-test window")?
            .check()
            .context("X11 rejected smoke-test window cleanup")?;
        conn.flush()
            .context("could not flush X11 smoke-test cleanup")
    })();

    smoke_result.and(cleanup_result)
}

#[cfg(target_os = "linux")]
fn linux_wait_for_focus(conn: &x11rb::rust_connection::RustConnection, window: u32) -> Result<()> {
    use x11rb::protocol::xproto::ConnectionExt;

    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        let focus = conn
            .get_input_focus()
            .context("could not request X11 smoke-test focus")?
            .reply()
            .context("could not read X11 smoke-test focus")?;
        if focus.focus == window {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    anyhow::bail!("X11 smoke-test window did not receive input focus")
}

#[cfg(target_os = "linux")]
fn linux_wait_for_v_key_events(
    conn: &x11rb::rust_connection::RustConnection,
    window: u32,
    v_keycode: u8,
) -> Result<()> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;

    let deadline = Instant::now() + Duration::from_millis(750);
    let mut saw_press = false;
    let mut saw_release = false;
    let mut observed = Vec::new();

    while Instant::now() < deadline {
        while let Some(event) = conn
            .poll_for_event()
            .context("could not poll X11 smoke-test events")?
        {
            match event {
                Event::KeyPress(event) => {
                    observed.push(format!(
                        "press:event={},detail={},state={:?}",
                        event.event, event.detail, event.state
                    ));
                    if event.event == window && event.detail == v_keycode {
                        saw_press = true;
                    }
                }
                Event::KeyRelease(event) => {
                    observed.push(format!(
                        "release:event={},detail={},state={:?}",
                        event.event, event.detail, event.state
                    ));
                    if event.event == window && event.detail == v_keycode {
                        saw_release = true;
                    }
                }
                Event::XinputRawKeyPress(event) => {
                    observed.push(format!(
                        "raw-press:detail={},device={},source={}",
                        event.detail, event.deviceid, event.sourceid
                    ));
                    if event.detail == u32::from(v_keycode) {
                        saw_press = true;
                    }
                }
                Event::XinputRawKeyRelease(event) => {
                    observed.push(format!(
                        "raw-release:detail={},device={},source={}",
                        event.detail, event.deviceid, event.sourceid
                    ));
                    if event.detail == u32::from(v_keycode) {
                        saw_release = true;
                    }
                }
                _ => {}
            }
        }
        if saw_press && saw_release {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    anyhow::bail!(
        "X11 smoke test did not observe the paste key event (target_window={window}, target_keycode={v_keycode}, press={saw_press}, release={saw_release}, observed=[{}])",
        observed.join(", ")
    )
}

#[cfg(test)]
mod tests {
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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn direct_mode_has_no_paste_modifiers() {
        assert!(paste_modifiers(PasteMode::Direct).is_empty());
    }

    #[test]
    fn failed_paste_restores_previous_clipboard() {
        let mut clipboard = MockClipboard::new("old clipboard");
        let events = clipboard.events();
        let result = paste_with_clipboard_swap(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Err(anyhow::anyhow!("paste failed"))
            },
            Duration::ZERO,
            Duration::ZERO,
        );

        assert!(result.is_err());
        assert_eq!(clipboard.text.as_deref(), Some("old clipboard"));
        assert_eq!(
            events.borrow().as_slice(),
            ["read", "set:dictated text", "paste", "set:old clipboard"]
        );
    }

    #[test]
    fn successful_paste_restores_previous_clipboard() {
        let mut clipboard = MockClipboard::new("old clipboard");
        let events = clipboard.events();
        paste_with_clipboard_swap(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("paste should succeed");

        assert_eq!(clipboard.text.as_deref(), Some("old clipboard"));
        assert_eq!(
            events.borrow().as_slice(),
            ["read", "set:dictated text", "paste", "set:old clipboard"]
        );
    }

    #[test]
    fn same_clipboard_text_is_not_rewritten_after_paste() {
        let mut clipboard = MockClipboard::new("dictated text");
        let events = clipboard.events();
        paste_with_clipboard_swap(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("paste should succeed");

        assert_eq!(clipboard.text.as_deref(), Some("dictated text"));
        assert_eq!(
            events.borrow().as_slice(),
            ["read", "set:dictated text", "paste"]
        );
    }

    #[test]
    fn empty_text_does_not_touch_clipboard_or_paste() {
        let mut clipboard = MockClipboard::empty();
        let events = clipboard.events();
        let mut pasted = false;
        paste_with_clipboard_swap(
            &mut clipboard,
            "",
            || {
                pasted = true;
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("empty paste should be a no-op");

        assert!(!pasted);
        assert!(clipboard.text.is_none());
        assert!(events.borrow().is_empty());
    }

    #[test]
    fn transcript_clipboard_write_failure_does_not_paste_or_restore() {
        let mut clipboard = MockClipboard::new("old clipboard").fail_next_set();
        let events = clipboard.events();
        let mut pasted = false;
        let result = paste_with_clipboard_swap(
            &mut clipboard,
            "dictated text",
            || {
                pasted = true;
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
        );

        assert!(result.is_err());
        assert!(!pasted);
        assert_eq!(clipboard.text.as_deref(), Some("old clipboard"));
        assert_eq!(events.borrow().as_slice(), ["read", "set:dictated text"]);
    }

    #[test]
    fn restore_failure_is_reported_after_successful_paste() {
        let mut clipboard = MockClipboard::new("old clipboard").fail_on_set("old clipboard");
        let events = clipboard.events();
        let result = paste_with_clipboard_swap(
            &mut clipboard,
            "dictated text",
            || {
                events.borrow_mut().push("paste".to_string());
                Ok(())
            },
            Duration::ZERO,
            Duration::ZERO,
        );

        let err = result.expect_err("restore failure should be reported");
        assert!(format!("{err:#}").contains("could not restore previous clipboard text"));
        assert_eq!(clipboard.text.as_deref(), Some("dictated text"));
        assert_eq!(
            events.borrow().as_slice(),
            ["read", "set:dictated text", "paste", "set:old clipboard"]
        );
    }
}
