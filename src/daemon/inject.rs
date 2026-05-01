//! Insert text at the cursor position.
//!
//! Batch mode uses the clipboard plus the platform paste shortcut so the final
//! transcript appears as a single insertion. Direct mode uses
//! `enigo::Keyboard::text()`, which:
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
#[cfg(not(target_os = "linux"))]
use enigo::Direction;
#[cfg(not(target_os = "linux"))]
use enigo::Key;
use enigo::{Enigo, Keyboard, Settings};
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::{
    thread,
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
use x11rb::connection::Connection as _;

/// Error label used when paste succeeded but previous clipboard restore failed.
pub(crate) const CLIPBOARD_RESTORE_ERROR: &str = "could not restore previous clipboard text";

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
    #[cfg(target_os = "linux")]
    super::session::ensure_text_insertion_supported()?;

    if insertion_needs_enigo(mode) {
        let _keyboard = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?;
    }
    if mode != PasteMode::Direct {
        let _clipboard = Clipboard::new().context("could not open system clipboard")?;
        platform_paste_preflight()?;
    }
    Ok(())
}

fn insertion_needs_enigo(mode: PasteMode) -> bool {
    mode == PasteMode::Direct || cfg!(not(target_os = "linux"))
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
    enigo: Option<Enigo>,
    clipboard: Option<Clipboard>,
    #[cfg(target_os = "linux")]
    x11_paste: Option<LinuxX11Paste>,
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
    /// Returns an error if the current desktop session is unsupported or if
    /// `enigo` cannot initialize the platform keyboard backend.
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "linux")]
        super::session::ensure_text_insertion_supported()?;

        Ok(Self {
            enigo: None,
            clipboard: None,
            #[cfg(target_os = "linux")]
            x11_paste: None,
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
        self.keyboard()?
            .text(text)
            .map_err(|e| anyhow::anyhow!("enigo type failed: {e:?}"))
            .context("could not type text at cursor")
    }

    #[cfg(target_os = "linux")]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<()> {
        if self.x11_paste.is_none() {
            self.x11_paste = Some(LinuxX11Paste::open()?);
        }

        let result = self
            .x11_paste
            .as_ref()
            .expect("X11 paste backend was just initialized")
            .send_paste_chord(mode);
        if result.is_err() {
            self.x11_paste = None;
        }
        result
    }

    #[cfg(not(target_os = "linux"))]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<()> {
        let modifiers = paste_modifiers(mode);
        let mut failure = None;
        let enigo = self.keyboard()?;
        for key in modifiers {
            if let Err(e) = enigo.key(*key, Direction::Press) {
                failure = Some(anyhow::anyhow!("enigo paste modifier press failed: {e:?}"));
                break;
            }
        }

        if failure.is_none() {
            failure = paste_key_click(enigo)
                .err()
                .map(|e| anyhow::anyhow!("enigo paste key failed: {e:?}"));
        }

        for key in modifiers.iter().rev() {
            if let Err(e) = enigo.key(*key, Direction::Release) {
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

    fn keyboard(&mut self) -> Result<&mut Enigo> {
        if self.enigo.is_none() {
            self.enigo = Some(
                Enigo::new(&Settings::default())
                    .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?,
            );
        }
        Ok(self.enigo.as_mut().expect("enigo was just initialized"))
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
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}"))
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
const CONTROL_L_KEYSYM: u32 = 0xffe3;
#[cfg(target_os = "linux")]
const SHIFT_L_KEYSYM: u32 = 0xffe1;

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct X11KeyStep {
    keysym: u32,
    press: bool,
}

#[cfg(target_os = "linux")]
struct LinuxX11Paste {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
}

#[cfg(target_os = "linux")]
impl LinuxX11Paste {
    fn open() -> Result<Self> {
        use x11rb::rust_connection::RustConnection;

        let (conn, screen_num) =
            RustConnection::connect(None).context("could not connect to X11")?;
        let root = super::x11::root_window(&conn, screen_num)?;
        Ok(Self { conn, root })
    }

    fn send_paste_chord(&self, mode: PasteMode) -> Result<()> {
        for step in linux_paste_chord_steps(mode) {
            let keycode = linux_cached_keycode(step.keysym)?;
            x11_fake_key(&self.conn, self.root, keycode, step.press)?;
        }

        self.conn
            .flush()
            .context("could not flush XTest paste chord")?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn linux_paste_chord_steps(mode: PasteMode) -> Vec<X11KeyStep> {
    let mut steps = vec![X11KeyStep {
        keysym: CONTROL_L_KEYSYM,
        press: true,
    }];
    if mode == PasteMode::Terminal {
        steps.push(X11KeyStep {
            keysym: SHIFT_L_KEYSYM,
            press: true,
        });
    }
    steps.push(X11KeyStep {
        keysym: super::x11::V_KEYSYM,
        press: true,
    });
    steps.push(X11KeyStep {
        keysym: super::x11::V_KEYSYM,
        press: false,
    });
    if mode == PasteMode::Terminal {
        steps.push(X11KeyStep {
            keysym: SHIFT_L_KEYSYM,
            press: false,
        });
    }
    steps.push(X11KeyStep {
        keysym: CONTROL_L_KEYSYM,
        press: false,
    });
    steps
}

#[cfg(target_os = "linux")]
fn linux_cached_keycode(keysym: u32) -> Result<u8> {
    static KEYCODES: OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<u32, Result<u8, String>>>,
    > = OnceLock::new();
    let keycodes =
        KEYCODES.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let mut keycodes = keycodes
        .lock()
        .map_err(|_| anyhow::anyhow!("X11 keycode cache lock poisoned"))?;
    if let Some(result) = keycodes.get(&keysym) {
        return result.clone().map_err(anyhow::Error::msg);
    }

    let result =
        super::x11::keycode_for_keysym_on_default_display(keysym).map_err(|err| format!("{err:#}"));
    keycodes.insert(keysym, result.clone());
    result.map_err(anyhow::Error::msg)
}

#[cfg(target_os = "linux")]
fn x11_fake_key(
    conn: &x11rb::rust_connection::RustConnection,
    root: u32,
    keycode: u8,
    press: bool,
) -> Result<()> {
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;

    let event_type = if press {
        KEY_PRESS_EVENT
    } else {
        KEY_RELEASE_EVENT
    };
    conn.xtest_fake_input(event_type, keycode, 0, root, 0, 0, 0)
        .context("could not send XTest key event")?
        .check()
        .context("X11 rejected XTest key event")?;
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
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt, CreateWindowAux, EventMask, InputFocus, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    let v_keycode = linux_cached_keycode(super::x11::V_KEYSYM)?;
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
}
