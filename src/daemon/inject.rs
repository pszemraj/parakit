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
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::{thread, time::Duration};

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
    /// # Returns
    ///
    /// `Ok(())` when the clipboard was populated and the paste shortcut was
    /// accepted by the platform backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened, the transcript
    /// cannot be copied, or the platform backend rejects the paste shortcut.
    pub fn paste_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        let previous = {
            let clipboard = self.text_clipboard()?;
            let previous = clipboard.get_text().ok();
            clipboard
                .set_text(text.to_owned())
                .context("could not copy transcript to clipboard")?;
            previous.filter(|p| p != text)
        };

        self.paste_clipboard()?;

        if let Some(previous) = previous {
            thread::sleep(Duration::from_millis(120));
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

    fn paste_clipboard(&mut self) -> Result<()> {
        let modifier = paste_modifier();
        self.enigo
            .key(modifier, Direction::Press)
            .map_err(|e| anyhow::anyhow!("enigo paste modifier press failed: {e:?}"))?;

        let click = self.enigo.key(Key::Unicode('v'), Direction::Click);
        let release = self.enigo.key(modifier, Direction::Release);

        click
            .map_err(|e| anyhow::anyhow!("enigo paste key failed: {e:?}"))
            .context("could not send paste shortcut")?;
        release
            .map_err(|e| anyhow::anyhow!("enigo paste modifier release failed: {e:?}"))
            .context("could not release paste shortcut modifier")?;

        Ok(())
    }
}

fn paste_modifier() -> Key {
    #[cfg(target_os = "macos")]
    {
        Key::Meta
    }

    #[cfg(not(target_os = "macos"))]
    {
        Key::Control
    }
}
