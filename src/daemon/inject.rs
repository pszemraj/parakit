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
//!   - Linux Wayland: unsupported. Startup preflight rejects Wayland sessions
//!     because XTest cannot insert into focused native Wayland applications.
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
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt as _;
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

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
    super::session::ensure_x11_session_supported()?;

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

/// Focus owner captured when recording begins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FocusSnapshot {
    #[cfg(target_os = "linux")]
    focus: u32,
}

impl FocusSnapshot {
    /// Capture the current focus owner for later drift checks.
    ///
    /// # Returns
    ///
    /// A focus snapshot that can be compared before insertion.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform focus cannot be read or has no
    /// concrete target window.
    pub(crate) fn capture() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let focus = linux_current_input_focus_on_default_display()?;
            if !linux_focus_is_insertable(focus) {
                anyhow::bail!("X11 input focus is not an insertable application window");
            }
            Ok(Self { focus })
        }

        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self {})
        }
    }

    /// Return whether the current focus still matches this snapshot.
    ///
    /// # Returns
    ///
    /// `Ok(true)` when it is safe to insert into the original target.
    ///
    /// # Errors
    ///
    /// Returns an error when the current focus cannot be read.
    pub(crate) fn matches_current(&self) -> Result<bool> {
        #[cfg(target_os = "linux")]
        {
            Ok(linux_current_input_focus_on_default_display()? == self.focus)
        }

        #[cfg(not(target_os = "linux"))]
        {
            Ok(true)
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_focus_is_insertable(focus: u32) -> bool {
    focus != x11rb::NONE && focus != u32::from(x11rb::protocol::xproto::InputFocus::POINTER_ROOT)
}

#[cfg(target_os = "linux")]
fn linux_current_input_focus_on_default_display() -> Result<u32> {
    let (conn, _) = RustConnection::connect(None).context("could not connect to X11")?;
    linux_current_input_focus(&conn)
}

#[cfg(target_os = "linux")]
fn linux_current_input_focus(conn: &RustConnection) -> Result<u32> {
    Ok(conn
        .get_input_focus()
        .context("could not request current X11 input focus")?
        .reply()
        .context("could not read current X11 input focus")?
        .focus)
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
        super::session::ensure_x11_session_supported()?;

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

    /// Copy text to the clipboard without sending any paste or type event.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard contains `text`.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened or written.
    pub fn copy_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut clipboard = match self.clipboard.take() {
            Some(clipboard) => clipboard,
            None => Clipboard::new().context("could not open system clipboard")?,
        };
        let result = clipboard
            .set_text(text.to_owned())
            .context("could not copy transcript to clipboard");
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
        let enigo = self.keyboard()?;
        let mut sink = EnigoPasteShortcutSink { enigo };
        send_paste_shortcut_with_cleanup(&mut sink, paste_modifiers(mode))
            .context("could not send paste shortcut")
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

#[cfg(not(target_os = "linux"))]
trait PasteShortcutSink {
    /// Send a modifier key press or release.
    /// # Arguments
    /// * `key` - Modifier key to press or release.
    /// * `direction` - Press or release direction to send.
    /// # Returns
    /// `Ok(())` when the backend accepted the synthetic key event.
    /// # Errors
    /// Returns an error when the platform rejects the synthetic key event.
    fn key(&mut self, key: Key, direction: Direction) -> Result<()>;

    /// Click the platform paste key.
    /// # Returns
    /// `Ok(())` when the backend accepted the synthetic paste key event.
    /// # Errors
    /// Returns an error when the platform rejects the synthetic paste key.
    fn paste_key(&mut self) -> Result<()>;
}

#[cfg(not(target_os = "linux"))]
struct EnigoPasteShortcutSink<'a> {
    enigo: &'a mut Enigo,
}

#[cfg(not(target_os = "linux"))]
impl PasteShortcutSink for EnigoPasteShortcutSink<'_> {
    fn key(&mut self, key: Key, direction: Direction) -> Result<()> {
        self.enigo
            .key(key, direction)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    fn paste_key(&mut self) -> Result<()> {
        paste_key_click(self.enigo)
    }
}

#[cfg(not(target_os = "linux"))]
fn send_paste_shortcut_with_cleanup<S: PasteShortcutSink>(
    sink: &mut S,
    modifiers: &[Key],
) -> Result<()> {
    let mut pressed = Vec::with_capacity(modifiers.len());
    let paste_result = (|| -> Result<()> {
        for key in modifiers {
            sink.key(*key, Direction::Press)
                .context("enigo paste modifier press failed")?;
            pressed.push(*key);
        }

        sink.paste_key().context("enigo paste key failed")?;
        Ok(())
    })();

    let mut cleanup_error = None;
    for key in pressed.into_iter().rev() {
        if let Err(err) = sink.key(key, Direction::Release) {
            cleanup_error.get_or_insert_with(|| err.context("enigo paste modifier release failed"));
        }
    }

    match (paste_result, cleanup_error) {
        (Ok(()), None) => Ok(()),
        (Err(err), None) => Err(err),
        (Ok(()), Some(cleanup)) => Err(cleanup).context("paste modifier cleanup failed"),
        (Err(err), Some(cleanup)) => Err(anyhow::anyhow!(
            "{err:#}; paste modifier cleanup also failed: {cleanup:#}"
        )),
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct X11KeyStep {
    keysym: u32,
    press: bool,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResolvedX11KeyStep {
    keycode: u8,
    press: bool,
}

#[cfg(target_os = "linux")]
struct LinuxX11Paste {
    conn: RustConnection,
    root: u32,
}

#[cfg(target_os = "linux")]
impl LinuxX11Paste {
    fn open() -> Result<Self> {
        let (conn, screen_num) =
            RustConnection::connect(None).context("could not connect to X11")?;
        let root = super::x11::root_window(&conn, screen_num)?;
        Ok(Self { conn, root })
    }

    fn send_paste_chord(&self, mode: PasteMode) -> Result<()> {
        let steps = linux_resolved_paste_chord_steps(mode)?;
        let mut sink = X11ConnectionKeySink {
            conn: &self.conn,
            root: self.root,
        };
        send_x11_key_steps(&mut sink, &steps)
    }
}

#[cfg(target_os = "linux")]
fn linux_paste_chord_steps(mode: PasteMode) -> Vec<X11KeyStep> {
    let mut steps = vec![X11KeyStep {
        keysym: super::x11::CONTROL_L_KEYSYM,
        press: true,
    }];
    if mode == PasteMode::Terminal {
        steps.push(X11KeyStep {
            keysym: super::x11::SHIFT_L_KEYSYM,
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
            keysym: super::x11::SHIFT_L_KEYSYM,
            press: false,
        });
    }
    steps.push(X11KeyStep {
        keysym: super::x11::CONTROL_L_KEYSYM,
        press: false,
    });
    steps
}

#[cfg(target_os = "linux")]
fn linux_resolved_paste_chord_steps(mode: PasteMode) -> Result<Vec<ResolvedX11KeyStep>> {
    linux_paste_chord_steps(mode)
        .into_iter()
        .map(|step| {
            Ok(ResolvedX11KeyStep {
                keycode: linux_cached_keycode(step.keysym)?,
                press: step.press,
            })
        })
        .collect()
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
trait X11KeySink {
    /// Send a key press or release event.
    ///
    /// # Arguments
    ///
    /// * `keycode` - X11 keycode to send.
    /// * `press` - `true` for key press, `false` for key release.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the sink accepted the key event.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend rejects the key event.
    fn key(&mut self, keycode: u8, press: bool) -> Result<()>;
    /// Flush queued key events to the X11 server.
    ///
    /// # Returns
    ///
    /// `Ok(())` when pending key events have been submitted.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend cannot flush pending events.
    fn flush(&mut self) -> Result<()>;
}

#[cfg(target_os = "linux")]
struct X11ConnectionKeySink<'a> {
    conn: &'a x11rb::rust_connection::RustConnection,
    root: u32,
}

#[cfg(target_os = "linux")]
impl X11KeySink for X11ConnectionKeySink<'_> {
    fn key(&mut self, keycode: u8, press: bool) -> Result<()> {
        use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
        use x11rb::protocol::xtest::ConnectionExt as XtestConnectionExt;

        let event_type = if press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.conn
            .xtest_fake_input(event_type, keycode, 0, self.root, 0, 0, 0)
            .context("could not send XTest key event")?
            .check()
            .context("X11 rejected XTest key event")?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.conn
            .flush()
            .context("could not flush XTest paste chord")
    }
}

#[cfg(target_os = "linux")]
fn send_x11_key_steps<S: X11KeySink>(sink: &mut S, steps: &[ResolvedX11KeyStep]) -> Result<()> {
    let mut pressed = Vec::new();
    for step in steps {
        if let Err(err) = sink.key(step.keycode, step.press) {
            let cleanup = release_pressed_x11_keys(sink, &mut pressed);
            return combine_primary_cleanup_error(
                err.context("could not send XTest paste chord"),
                cleanup,
            );
        }

        if step.press {
            pressed.push(step.keycode);
        } else if let Some(index) = pressed.iter().rposition(|key| *key == step.keycode) {
            pressed.remove(index);
        }
    }

    if let Err(err) = sink.flush() {
        let cleanup = release_pressed_x11_keys(sink, &mut pressed);
        return combine_primary_cleanup_error(err, cleanup);
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn release_pressed_x11_keys<S: X11KeySink>(sink: &mut S, pressed: &mut Vec<u8>) -> Result<()> {
    let mut cleanup_error = None;
    while let Some(keycode) = pressed.pop() {
        if let Err(err) = sink.key(keycode, false) {
            cleanup_error.get_or_insert_with(|| err.context("could not release XTest key"));
        }
    }

    if cleanup_error.is_none() {
        cleanup_error = sink.flush().err();
    }

    match cleanup_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(target_os = "linux")]
fn combine_primary_cleanup_error(primary: anyhow::Error, cleanup: Result<()>) -> Result<()> {
    match cleanup {
        Ok(()) => Err(primary),
        Err(cleanup_err) => Err(anyhow::anyhow!(
            "{primary:#}; cleanup while releasing pressed XTest keys failed: {cleanup_err:#}"
        )),
    }
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
fn linux_wait_for_focus(conn: &RustConnection, window: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        if linux_current_input_focus(conn)? == window {
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
#[path = "inject_tests.rs"]
mod inject_tests;
