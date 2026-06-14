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
use arboard::{Clipboard, ImageData};
use clap::ValueEnum;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use enigo::Direction;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use enigo::Key;
use enigo::{Enigo, Keyboard, Settings};
use std::{borrow::Cow, path::PathBuf, thread, time::Duration};
#[cfg(target_os = "linux")]
use x11rb::connection::Connection as _;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt as _;
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

#[cfg(test)]
use super::clipboard_restore::ClipboardWriteSnapshot;
use super::clipboard_restore::{
    ClipboardRestoreGate, ClipboardRestorePlan, ClipboardWriteToken, PlatformClipboardRestoreGate,
};

#[cfg(target_os = "linux")]
#[path = "inject_smoke.rs"]
mod inject_smoke;

/// Error label used when paste succeeded but previous clipboard restore failed.
pub(crate) const CLIPBOARD_RESTORE_ERROR: &str = "could not restore previous clipboard contents";

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

/// Result of a guarded paste attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasteOutcome {
    /// The paste chord or direct typing path was sent.
    Pasted,
    /// The transcript was left on the clipboard and no synthetic input was sent.
    CopiedOnly,
    /// No paste chord was sent and clipboard policy was applied.
    Blocked,
}

/// Clipboard retention policy after staging text for paste.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardPolicy {
    /// Restore previous supported clipboard contents after paste or guarded cancellation.
    RestorePrevious,
    /// Leave the transcript on the clipboard after paste or guarded cancellation.
    KeepTranscript,
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
    let mut injector = Injector::new()?;
    injector.prepare_for_mode(mode)?;
    if mode != PasteMode::Direct {
        platform_paste_preflight()?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn insertion_needs_enigo(mode: PasteMode) -> bool {
    mode == PasteMode::Direct
}

#[cfg(not(target_os = "windows"))]
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

/// Minimal clipboard operations used by insertion and smoke-test paths.
pub(super) trait ClipboardStore {
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

    /// Return current HTML clipboard contents.
    ///
    /// # Returns
    ///
    /// The current HTML clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose HTML data.
    fn get_html(&mut self) -> Result<String>;

    /// Replace the clipboard with HTML and an optional plain-text alternative.
    ///
    /// # Arguments
    ///
    /// * `html` - HTML payload to restore.
    /// * `alt_text` - Optional plain-text alternative for targets that prefer text.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the HTML data.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()>;

    /// Return current file-list clipboard contents.
    ///
    /// # Returns
    ///
    /// The current list of copied file paths.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose file-list data.
    fn get_file_list(&mut self) -> Result<Vec<PathBuf>>;

    /// Replace the clipboard with a file-list payload.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the file list.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()>;

    /// Return current image clipboard contents.
    ///
    /// # Returns
    ///
    /// The current image clipboard value.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard does not expose image data.
    fn get_image(&mut self) -> Result<ImageData<'static>>;

    /// Replace the clipboard with image data.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard accepted the image.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be written.
    fn set_image(&mut self, image: ImageData<'static>) -> Result<()>;

    /// Clear all current clipboard contents.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the clipboard was cleared.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be cleared.
    fn clear(&mut self) -> Result<()>;
}

impl ClipboardStore for Clipboard {
    fn get_text(&mut self) -> Result<String> {
        Clipboard::get_text(self).context("could not read system clipboard")
    }

    fn set_text(&mut self, text: String) -> Result<()> {
        Clipboard::set_text(self, text).context("could not write system clipboard")
    }

    fn get_html(&mut self) -> Result<String> {
        self.get()
            .html()
            .context("could not read HTML clipboard contents")
    }

    fn set_html(&mut self, html: String, alt_text: Option<String>) -> Result<()> {
        self.set()
            .html(html, alt_text)
            .context("could not write HTML clipboard contents")
    }

    fn get_file_list(&mut self) -> Result<Vec<PathBuf>> {
        self.get()
            .file_list()
            .context("could not read file-list clipboard contents")
    }

    fn set_file_list(&mut self, files: &[PathBuf]) -> Result<()> {
        self.set()
            .file_list(files)
            .context("could not write file-list clipboard contents")
    }

    fn get_image(&mut self) -> Result<ImageData<'static>> {
        Clipboard::get_image(self).context("could not read image clipboard contents")
    }

    fn set_image(&mut self, image: ImageData<'static>) -> Result<()> {
        Clipboard::set_image(self, image).context("could not write image clipboard contents")
    }

    fn clear(&mut self) -> Result<()> {
        Clipboard::clear(self).context("could not clear system clipboard")
    }
}

/// Focus owner captured when recording begins.
pub(crate) struct FocusSnapshot {
    #[cfg(target_os = "linux")]
    input_focus: Option<u32>,
    #[cfg(target_os = "linux")]
    active_window: Option<u32>,
    #[cfg(target_os = "windows")]
    windows: super::windows_focus::WindowsFocusSnapshot,
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
            let (conn, screen_num) = RustConnection::connect(None)
                .context("could not connect to X11 while capturing recording focus")?;
            let root = super::x11::root_window(&conn, screen_num)
                .context("could not read X11 root window for focus snapshot")?;
            let focus = linux_current_input_focus(&conn)?;
            let input_focus = linux_focus_is_insertable(focus).then_some(focus);
            let active_window = super::x11::active_window(&conn, root)
                .context("could not read X11 active window for focus snapshot")?;
            if input_focus.is_none() && active_window.is_none() {
                anyhow::bail!("X11 focus is not an insertable application window");
            }
            Ok(Self {
                input_focus,
                active_window,
            })
        }

        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                windows: super::windows_focus::WindowsFocusSnapshot::capture()?,
            })
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
            let (conn, screen_num) = RustConnection::connect(None)
                .context("could not reconnect to X11 while checking recording focus")?;
            let root = super::x11::root_window(&conn, screen_num)
                .context("could not read X11 root window for focus check")?;
            if let Some(expected) = self.active_window {
                if let Some(current) = super::x11::active_window(&conn, root)
                    .context("could not query the current X11 active window")?
                {
                    return Ok(current == expected);
                }
            }

            let Some(expected) = self.input_focus else {
                anyhow::bail!(
                    "X11 active window is unavailable and no input focus fallback exists"
                );
            };
            Ok(
                linux_current_input_focus(&conn)
                    .context("could not query the current X11 focus")?
                    == expected,
            )
        }

        #[cfg(target_os = "windows")]
        {
            self.windows.matches_current()
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
    #[cfg(target_os = "windows")]
    clipboard_history: Option<super::windows_clipboard_history::ClipboardHistoryListener>,
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

        #[cfg(target_os = "windows")]
        let clipboard_history =
            match super::windows_clipboard_history::ClipboardHistoryListener::start() {
                Ok(listener) => Some(listener),
                Err(err) => {
                    clipboard_history_debug(format_args!(
                        "Windows clipboard-history listener unavailable; using timed restore fallback: {err:#}"
                    ));
                    None
                }
            };

        Ok(Self {
            enigo: None,
            clipboard: None,
            #[cfg(target_os = "windows")]
            clipboard_history,
            #[cfg(target_os = "linux")]
            x11_paste: None,
        })
    }

    /// Initialize the platform resources needed for `mode`.
    ///
    /// Linux keeps the X11 paste connection and resolved keycodes warm so a
    /// long-running daemon does not have to rediscover the display during the
    /// narrow paste window after transcription.
    ///
    /// # Arguments
    ///
    /// * `mode` - Insertion mode that will be used later.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the required keyboard, clipboard, and paste handles are
    /// ready.
    ///
    /// # Errors
    ///
    /// Returns an error if a required platform handle cannot be opened.
    pub fn prepare_for_mode(&mut self, mode: PasteMode) -> Result<()> {
        if insertion_needs_enigo(mode) {
            let _keyboard = self.keyboard()?;
        }

        if mode != PasteMode::Direct && self.clipboard.is_none() {
            self.clipboard = Some(Clipboard::new().context("could not open system clipboard")?);
        }

        #[cfg(target_os = "linux")]
        if mode != PasteMode::Direct && self.x11_paste.is_none() {
            self.x11_paste =
                Some(LinuxX11Paste::open().context("could not initialize X11 paste connection")?);
        }

        Ok(())
    }

    /// Paste text, but re-run a caller-supplied safety check immediately before
    /// synthetic input is sent.
    ///
    /// By default the previous supported clipboard payload is restored after
    /// the paste consume delay. Callers may opt into leaving the transcript on
    /// the clipboard for workflows that prefer that behavior.
    ///
    /// # Arguments
    ///
    /// * `text` - Transcript text to insert.
    /// * `mode` - Paste shortcut style to send after updating the clipboard.
    ///
    /// # Returns
    ///
    /// [`PasteOutcome::Pasted`] when synthetic input was sent,
    /// [`PasteOutcome::CopiedOnly`] when the guard blocked insertion and the
    /// transcript was intentionally left on the clipboard, or
    /// [`PasteOutcome::Blocked`] when no input was sent and the previous
    /// clipboard was restored.
    ///
    /// # Errors
    ///
    /// Returns an error if clipboard staging, the guard, direct typing, or the
    /// paste shortcut fails.
    pub fn paste_text_guarded(
        &mut self,
        text: &str,
        mode: PasteMode,
        clipboard_policy: ClipboardPolicy,
        mut before_chord: impl FnMut() -> Result<bool>,
    ) -> Result<PasteOutcome> {
        if text.is_empty() {
            return Ok(PasteOutcome::Pasted);
        }
        if mode == PasteMode::Direct {
            if before_chord()? {
                self.type_text(text)?;
                return Ok(PasteOutcome::Pasted);
            }
            anyhow::bail!("direct insertion blocked by safety guard");
        }

        let mut clipboard = self.take_clipboard()?;
        let restore_gate = self.clipboard_restore_gate();
        let restore_plan = ClipboardRestorePlan::new(
            clipboard_restore_delay(),
            restore_gate.paste_consume_delay(),
            &restore_gate,
        );
        let result = paste_with_clipboard_swap_guarded(
            &mut clipboard,
            text,
            || self.paste_clipboard(mode),
            clipboard_settle_delay(),
            restore_plan,
            clipboard_policy,
            before_chord,
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

    /// Stage text without sending any paste or type event, then apply the
    /// configured clipboard retention policy.
    ///
    /// This is used for blocked insertion paths so clipboard history managers
    /// can still observe the transcript while the active clipboard is restored
    /// by default.
    ///
    /// # Arguments
    ///
    /// * `text` - Transcript text to stage.
    /// * `clipboard_policy` - Policy deciding whether the transcript remains
    ///   on the active clipboard.
    ///
    /// # Returns
    ///
    /// [`PasteOutcome::CopiedOnly`] when the transcript remains on the active
    /// clipboard, or [`PasteOutcome::Blocked`] when the previous clipboard was
    /// restored.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard cannot be opened, written, or restored.
    pub fn stage_text_for_history(
        &mut self,
        text: &str,
        clipboard_policy: ClipboardPolicy,
    ) -> Result<PasteOutcome> {
        if text.is_empty() {
            return Ok(PasteOutcome::Blocked);
        }
        let mut clipboard = self.take_clipboard()?;
        let restore_gate = self.clipboard_restore_gate();
        let restore_plan = ClipboardRestorePlan::new(
            clipboard_restore_delay(),
            restore_gate.paste_consume_delay(),
            &restore_gate,
        );
        let result = stage_text_without_paste(&mut clipboard, text, restore_plan, clipboard_policy);
        self.clipboard = Some(clipboard);
        result
    }

    fn take_clipboard(&mut self) -> Result<Clipboard> {
        match self.clipboard.take() {
            Some(clipboard) => Ok(clipboard),
            None => Clipboard::new().context("could not open system clipboard"),
        }
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

    #[cfg(target_os = "windows")]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<()> {
        let use_shift = mode == PasteMode::Terminal;
        super::windows_input::send_paste_chord(use_shift)
            .context("could not send Windows paste shortcut")
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
    fn paste_clipboard(&mut self, mode: PasteMode) -> Result<()> {
        let enigo = self.keyboard()?;
        let mut sink = EnigoPasteShortcutSink { enigo };
        let result = send_paste_shortcut_with_cleanup(&mut sink, paste_modifiers(mode))
            .context("could not send paste shortcut");
        flush_paste_modifiers(&mut sink);
        result
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

    fn clipboard_restore_gate(&self) -> PlatformClipboardRestoreGate {
        #[cfg(target_os = "windows")]
        {
            PlatformClipboardRestoreGate::from_listener(self.clipboard_history.as_ref())
        }
        #[cfg(not(target_os = "windows"))]
        {
            PlatformClipboardRestoreGate::fallback()
        }
    }
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
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

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
struct EnigoPasteShortcutSink<'a> {
    enigo: &'a mut Enigo,
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
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

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
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

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
fn flush_paste_modifiers<S: PasteShortcutSink>(sink: &mut S) {
    for key in [Key::Control, Key::Shift, Key::Alt, Key::Meta] {
        let _ = sink.key(key, Direction::Release);
    }
}

fn paste_with_clipboard_swap_guarded<C, P, G, H>(
    clipboard: &mut C,
    text: &str,
    mut paste: P,
    settle_delay: Duration,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
    mut before_chord: G,
) -> Result<PasteOutcome>
where
    C: ClipboardStore,
    P: FnMut() -> Result<()>,
    G: FnMut() -> Result<bool>,
    H: ClipboardRestoreGate + ?Sized,
{
    if text.is_empty() {
        return Ok(PasteOutcome::Pasted);
    }

    match before_chord() {
        Ok(true) => {}
        Ok(false) => {
            return stage_text_without_paste(clipboard, text, restore_plan, clipboard_policy);
        }
        Err(err) => return Err(err),
    }

    let previous = ClipboardSnapshot::capture(clipboard);
    let write_before = restore_plan.before_transcript_write();
    clipboard
        .set_text(text.to_owned())
        .context("could not copy transcript to clipboard")?;
    let write_token = restore_plan.after_transcript_write(write_before);

    sleep_if_nonzero(settle_delay);
    match before_chord() {
        Ok(true) => {}
        Ok(false) => {
            return finish_blocked_clipboard(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
            );
        }
        Err(err) => {
            let restore_result = restore_after_delay(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
                RestoreWait::BeforeRestore,
            );
            return match restore_result {
                Ok(()) => Err(err),
                Err(restore_err) => Err(err.context(format!("{restore_err:#}"))),
            };
        }
    }

    let paste_result = paste();
    match paste_result {
        Ok(()) => {
            restore_after_delay(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
                RestoreWait::AfterPaste,
            )?;
            Ok(PasteOutcome::Pasted)
        }
        Err(paste_err) => {
            let restore_result = restore_after_delay(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
                RestoreWait::BeforeRestore,
            );
            match restore_result {
                Ok(()) => Err(paste_err),
                Err(restore_err) => Err(paste_err.context(format!("{restore_err:#}"))),
            }
        }
    }
}

fn stage_text_without_paste<C, H>(
    clipboard: &mut C,
    text: &str,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
) -> Result<PasteOutcome>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    if clipboard_policy == ClipboardPolicy::KeepTranscript {
        clipboard
            .set_text(text.to_owned())
            .context("could not copy transcript to clipboard")?;
        return Ok(PasteOutcome::CopiedOnly);
    }

    let previous = ClipboardSnapshot::capture(clipboard);
    let write_before = restore_plan.before_transcript_write();
    clipboard
        .set_text(text.to_owned())
        .context("could not copy transcript to clipboard")?;
    let write_token = restore_plan.after_transcript_write(write_before);
    restore_after_delay(
        clipboard,
        previous,
        write_token,
        restore_plan,
        clipboard_policy,
        RestoreWait::BeforeRestore,
    )?;
    Ok(PasteOutcome::Blocked)
}

fn finish_blocked_clipboard<C, H>(
    clipboard: &mut C,
    previous: ClipboardSnapshot,
    write_token: ClipboardWriteToken,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
) -> Result<PasteOutcome>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    restore_after_delay(
        clipboard,
        previous,
        write_token,
        restore_plan,
        clipboard_policy,
        RestoreWait::BeforeRestore,
    )?;
    Ok(match clipboard_policy {
        ClipboardPolicy::RestorePrevious => PasteOutcome::Blocked,
        ClipboardPolicy::KeepTranscript => PasteOutcome::CopiedOnly,
    })
}

#[derive(Clone, Copy)]
enum RestoreWait {
    BeforeRestore,
    AfterPaste,
}

fn restore_after_delay<C, H>(
    clipboard: &mut C,
    previous: ClipboardSnapshot,
    write_token: ClipboardWriteToken,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
    wait: RestoreWait,
) -> Result<()>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    if clipboard_policy == ClipboardPolicy::RestorePrevious {
        match wait {
            RestoreWait::BeforeRestore => restore_plan.wait_before_restore(write_token),
            RestoreWait::AfterPaste => restore_plan.wait_after_paste_before_restore(write_token),
        }
    }
    restore_or_clear_clipboard(clipboard, previous, clipboard_policy)
}

/// Best-effort snapshot of supported clipboard payloads before staging text.
pub(super) enum ClipboardSnapshot {
    Text(String),
    Html {
        html: String,
        alt_text: Option<String>,
    },
    FileList(Vec<PathBuf>),
    Image(ImageData<'static>),
    Unsupported,
}

impl ClipboardSnapshot {
    /// Capture the current clipboard payload if it is one of the supported kinds.
    ///
    /// # Arguments
    ///
    /// * `clipboard` - Clipboard backend to inspect.
    ///
    /// # Returns
    ///
    /// A supported clipboard snapshot, or [`ClipboardSnapshot::Unsupported`]
    /// when the current payload cannot be restored by Parakit.
    pub(super) fn capture<C: ClipboardStore>(clipboard: &mut C) -> Self {
        if let Ok(files) = clipboard.get_file_list() {
            return Self::FileList(files);
        }

        if let Ok(html) = clipboard.get_html() {
            return Self::Html {
                html,
                alt_text: clipboard.get_text().ok(),
            };
        }

        if let Ok(image) = clipboard.get_image() {
            return Self::Image(owned_image(image));
        }

        match clipboard.get_text().ok() {
            Some(text) => Self::Text(text),
            None => Self::Unsupported,
        }
    }
}

fn owned_image(image: ImageData<'_>) -> ImageData<'static> {
    ImageData {
        width: image.width,
        height: image.height,
        bytes: Cow::Owned(image.bytes.into_owned()),
    }
}

/// Restore a previous clipboard snapshot unless the transcript should be retained.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to update.
/// * `previous` - Snapshot captured before staging transcript text.
/// * `clipboard_policy` - Policy deciding whether restoration should occur.
///
/// # Returns
///
/// `Ok(())` when restoration is skipped or the previous payload is restored.
///
/// # Errors
///
/// Returns an error if the previous supported payload cannot be written back
/// to the clipboard.
pub(super) fn restore_or_clear_clipboard<C: ClipboardStore>(
    clipboard: &mut C,
    previous: ClipboardSnapshot,
    clipboard_policy: ClipboardPolicy,
) -> Result<()> {
    if clipboard_policy == ClipboardPolicy::KeepTranscript {
        return Ok(());
    }
    match previous {
        ClipboardSnapshot::Text(previous) => clipboard
            .set_text(previous)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Html { html, alt_text } => clipboard
            .set_html(html, alt_text)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::FileList(files) => clipboard
            .set_file_list(&files)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Image(image) => clipboard
            .set_image(image)
            .map_err(|err| anyhow::anyhow!("{CLIPBOARD_RESTORE_ERROR}: {err:#}")),
        ClipboardSnapshot::Unsupported => clipboard
            .clear()
            .or_else(|_| clipboard.set_text(String::new()))
            .map_err(|err| {
                anyhow::anyhow!(
                    "{CLIPBOARD_RESTORE_ERROR}: previous clipboard format unsupported and staged transcript could not be cleared: {err:#}"
                )
            }),
    }
}

fn sleep_if_nonzero(delay: Duration) {
    if !delay.is_zero() {
        thread::sleep(delay);
    }
}

#[cfg(target_os = "windows")]
fn clipboard_history_debug(message: impl std::fmt::Display) {
    #[cfg(debug_assertions)]
    eprintln!("parakit: debug: {message}");
    #[cfg(not(debug_assertions))]
    let _ = message;
}

#[cfg(target_os = "macos")]
fn paste_modifiers(mode: PasteMode) -> &'static [Key] {
    match mode {
        PasteMode::Standard | PasteMode::Terminal => &[Key::Meta],
        PasteMode::Direct => &[],
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
    inject_smoke::linux_x11_paste_smoke_test(mode)
}

#[cfg(target_os = "windows")]
fn platform_paste_smoke_test(mode: PasteMode) -> Result<()> {
    super::windows_paste_smoke::windows_paste_smoke_test(mode)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
    #[cfg(target_os = "linux")]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "macos")]
    {
        Duration::from_millis(200)
    }
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(750)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Duration::from_millis(150)
    }
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
    standard_steps: Vec<ResolvedX11KeyStep>,
    terminal_steps: Vec<ResolvedX11KeyStep>,
    modifier_cleanup_keycodes: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl LinuxX11Paste {
    fn open() -> Result<Self> {
        let (conn, screen_num) =
            RustConnection::connect(None).context("could not connect to X11")?;
        let root = super::x11::root_window(&conn, screen_num)?;
        let standard_steps = linux_resolved_paste_chord_steps(&conn, PasteMode::Standard)?;
        let terminal_steps = linux_resolved_paste_chord_steps(&conn, PasteMode::Terminal)?;
        let modifier_cleanup_keycodes = linux_resolved_modifier_cleanup_keycodes(&conn);
        Ok(Self {
            conn,
            root,
            standard_steps,
            terminal_steps,
            modifier_cleanup_keycodes,
        })
    }

    fn send_paste_chord(&self, mode: PasteMode) -> Result<()> {
        let steps = match mode {
            PasteMode::Standard => &self.standard_steps,
            PasteMode::Terminal => &self.terminal_steps,
            PasteMode::Direct => anyhow::bail!("direct mode does not use the X11 paste chord"),
        };
        let mut sink = X11ConnectionKeySink {
            conn: &self.conn,
            root: self.root,
        };
        send_x11_paste_chord_with_modifier_flush(&mut sink, steps, &self.modifier_cleanup_keycodes)
    }
}

#[cfg(target_os = "linux")]
fn linux_modifier_cleanup_keysyms() -> &'static [u32] {
    &[
        super::x11::CONTROL_L_KEYSYM,
        super::x11::CONTROL_R_KEYSYM,
        super::x11::SHIFT_L_KEYSYM,
        super::x11::SHIFT_R_KEYSYM,
        super::x11::ALT_L_KEYSYM,
        super::x11::ALT_R_KEYSYM,
        super::x11::SUPER_L_KEYSYM,
        super::x11::SUPER_R_KEYSYM,
    ]
}

#[cfg(target_os = "linux")]
fn linux_resolved_modifier_cleanup_keycodes(conn: &RustConnection) -> Vec<u8> {
    linux_modifier_cleanup_keysyms()
        .iter()
        .filter_map(|keysym| super::x11::keycode_for_keysym(conn, *keysym).ok())
        .collect()
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
fn linux_resolved_paste_chord_steps(
    conn: &RustConnection,
    mode: PasteMode,
) -> Result<Vec<ResolvedX11KeyStep>> {
    linux_paste_chord_steps(mode)
        .into_iter()
        .map(|step| {
            Ok(ResolvedX11KeyStep {
                keycode: super::x11::keycode_for_keysym(conn, step.keysym)?,
                press: step.press,
            })
        })
        .collect()
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
fn send_x11_paste_chord_with_modifier_flush<S: X11KeySink>(
    sink: &mut S,
    steps: &[ResolvedX11KeyStep],
    modifier_keycodes: &[u8],
) -> Result<()> {
    send_x11_key_steps(sink, steps)?;
    // Best-effort blanket releases protect against focus changes during the
    // paste chord that leave the X server believing a modifier is still held.
    let _ = flush_x11_modifier_releases(sink, modifier_keycodes);
    Ok(())
}

#[cfg(target_os = "linux")]
fn flush_x11_modifier_releases<S: X11KeySink>(
    sink: &mut S,
    modifier_keycodes: &[u8],
) -> Result<()> {
    for keycode in modifier_keycodes {
        sink.key(*keycode, false)
            .context("could not send XTest modifier cleanup release")?;
    }
    sink.flush()
        .context("could not flush XTest modifier cleanup")
}

#[cfg(target_os = "linux")]
fn release_pressed_x11_keys<S: X11KeySink>(sink: &mut S, pressed: &mut Vec<u8>) -> Result<()> {
    let mut release_error = None;
    while let Some(keycode) = pressed.pop() {
        if let Err(err) = sink.key(keycode, false) {
            release_error.get_or_insert_with(|| err.context("could not release XTest key"));
        }
    }

    let flush_error = sink.flush().err();
    match (release_error, flush_error) {
        (None, None) => Ok(()),
        (Some(err), None) | (None, Some(err)) => Err(err),
        (Some(release_err), Some(flush_err)) => Err(anyhow::anyhow!(
            "{release_err:#}; XTest cleanup flush also failed: {flush_err:#}"
        )),
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

#[cfg(test)]
#[path = "inject_tests.rs"]
mod inject_tests;
