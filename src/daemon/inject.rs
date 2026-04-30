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
use std::{thread, time::Duration};

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

        let previous = {
            let clipboard = self.text_clipboard()?;
            let previous = clipboard.get_text().ok();
            clipboard
                .set_text(text.to_owned())
                .context("could not copy transcript to clipboard")?;
            previous.filter(|p| p != text)
        };

        thread::sleep(clipboard_settle_delay());
        self.paste_clipboard(mode)?;

        if let Some(previous) = previous {
            thread::sleep(clipboard_restore_delay());
            if let Some(clipboard) = &mut self.clipboard {
                let _ = clipboard.set_text(previous);
            }
        }

        Ok(())
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

    fn text_clipboard(&mut self) -> Result<&mut Clipboard> {
        if self.clipboard.is_none() {
            let clipboard = Clipboard::new().context("could not open system clipboard")?;
            self.clipboard = Some(clipboard);
        }
        Ok(self
            .clipboard
            .as_mut()
            .expect("clipboard was initialized above"))
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

#[cfg(target_os = "macos")]
fn paste_modifiers(_mode: PasteMode) -> &'static [Key] {
    &[Key::Meta]
}

#[cfg(not(target_os = "macos"))]
fn paste_modifiers(mode: PasteMode) -> &'static [Key] {
    match mode {
        PasteMode::Standard => &[Key::Control],
        PasteMode::Terminal => &[Key::Control, Key::Shift],
        PasteMode::Direct => &[],
    }
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
fn paste_key_click(enigo: &mut Enigo) -> Result<()> {
    let keycode = linux_cached_keycode_for_keysym(b'v' as u32)?;
    enigo
        .raw(keycode as u16, Direction::Click)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
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
fn linux_cached_keycode_for_keysym(keysym: u32) -> Result<u8> {
    static V_KEYCODE: OnceLock<Result<u8, String>> = OnceLock::new();
    V_KEYCODE
        .get_or_init(|| linux_keycode_for_keysym(keysym).map_err(|err| format!("{err:#}")))
        .clone()
        .map_err(anyhow::Error::msg)
}

#[cfg(target_os = "linux")]
fn linux_keycode_for_keysym(keysym: u32) -> Result<u8> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;
    use x11rb::rust_connection::RustConnection;

    let (conn, _) = RustConnection::connect(None).context("could not connect to X11")?;
    let setup = conn.setup();
    let min_keycode = setup.min_keycode;
    let max_keycode = setup.max_keycode;
    let count = max_keycode - min_keycode + 1;
    let mapping = conn
        .get_keyboard_mapping(min_keycode, count)
        .context("could not request X11 keyboard mapping")?
        .reply()
        .context("could not read X11 keyboard mapping")?;
    let keysyms_per_keycode = mapping.keysyms_per_keycode as usize;

    for (offset, keysyms) in mapping.keysyms.chunks(keysyms_per_keycode).enumerate() {
        if keysyms.contains(&keysym) {
            return Ok(min_keycode + offset as u8);
        }
    }

    anyhow::bail!("could not map X11 keysym {keysym} to a keycode")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
