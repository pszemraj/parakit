//! Type text at the cursor position without using the clipboard.
//!
//! Uses `enigo::Keyboard::text()` which:
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
use enigo::{Enigo, Keyboard, Settings};
/// Open a typing handle.
pub struct Injector {
    enigo: Enigo,
}

impl Injector {
    /// Create an injector backed by the platform's synthetic keyboard API.
    ///
    /// # Returns
    ///
    /// A ready-to-use text injector.
    ///
    /// # Errors
    ///
    /// Returns an error if `enigo` cannot initialize the platform keyboard
    /// backend.
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("failed to init enigo: {e:?}"))?;
        Ok(Self { enigo })
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
}
