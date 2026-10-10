//! X11 paste synthesis and physical-modifier probes.

use super::{PasteDispatch, PasteMode};
use crate::daemon::desktop::x11;
use anyhow::{Context, Result};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct X11KeyStep {
    keysym: u32,
    press: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResolvedX11KeyStep {
    keycode: u8,
    press: bool,
}

/// X11 connection and complete resolved mapping for paste or direct guards.
pub(super) struct LinuxX11Paste {
    conn: RustConnection,
    root: u32,
    mode: PasteMode,
    standard_steps: Vec<ResolvedX11KeyStep>,
    terminal_steps: Vec<ResolvedX11KeyStep>,
    modifier_keycodes: Vec<u8>,
}

impl LinuxX11Paste {
    /// Open a connection with both batch-paste chords resolved.
    ///
    /// # Returns
    ///
    /// A ready batch-paste backend.
    ///
    /// # Errors
    ///
    /// Returns an error when the display or keyboard mapping is unavailable.
    pub(super) fn open() -> Result<Self> {
        Self::open_for_mode(PasteMode::Standard)
    }

    /// Open the modifier probe and any paste chords needed by `mode`.
    ///
    /// # Arguments
    ///
    /// * `mode` - Direct mode requires modifiers only; batch modes need chords.
    ///
    /// # Returns
    ///
    /// A backend with a complete initial keyboard mapping.
    ///
    /// # Errors
    ///
    /// Returns an error when connecting or resolving keycodes fails.
    pub(super) fn open_for_mode(mode: PasteMode) -> Result<Self> {
        let (conn, screen_num) =
            RustConnection::connect(None).context("could not connect to X11")?;
        let root = x11::root_window(&conn, screen_num)?;
        let mut paste = Self {
            conn,
            root,
            mode,
            standard_steps: Vec::new(),
            terminal_steps: Vec::new(),
            modifier_keycodes: Vec::new(),
        };
        paste.refresh_mapping()?;
        Ok(paste)
    }

    /// Send a paste chord only while physical modifiers are released.
    ///
    /// # Arguments
    ///
    /// * `mode` - Batch shortcut to synthesize.
    ///
    /// # Returns
    ///
    /// Whether the chord was posted or withheld for held modifiers.
    ///
    /// # Errors
    ///
    /// Returns an error for direct mode or a failed X11 query/key event.
    pub(super) fn send_paste_chord(&mut self, mode: PasteMode) -> Result<PasteDispatch> {
        self.refresh_mapping_if_needed()?;
        let steps = match mode {
            PasteMode::Standard => &self.standard_steps,
            PasteMode::Terminal => &self.terminal_steps,
            PasteMode::Direct => anyhow::bail!("direct mode does not use the X11 paste chord"),
        };
        let mut sink = X11ConnectionKeySink {
            conn: &self.conn,
            root: self.root,
        };
        send_x11_paste_chord_with_modifier_flush(
            &mut sink,
            steps,
            &self.modifier_keycodes,
            &self.keymap()?,
        )
    }

    /// Read physical modifiers using the current keyboard mapping.
    ///
    /// # Returns
    ///
    /// Whether any paste-relevant modifier is held.
    ///
    /// # Errors
    ///
    /// Returns an error when mapping refresh or keymap queries fail.
    pub(super) fn modifiers_held(&mut self) -> Result<bool> {
        self.refresh_mapping_if_needed()?;
        Ok(x11_modifier_held(&self.keymap()?, &self.modifier_keycodes))
    }

    fn refresh_mapping_if_needed(&mut self) -> Result<()> {
        if x11::mapping_changed(&self.conn)? {
            self.refresh_mapping()?;
        }
        Ok(())
    }

    fn refresh_mapping(&mut self) -> Result<()> {
        let modifier_keycodes =
            x11::keycodes_for_keysyms(&self.conn, linux_modifier_cleanup_keysyms())?;
        anyhow::ensure!(
            !modifier_keycodes.is_empty(),
            "could not resolve X11 modifier keycodes"
        );
        let (standard_steps, terminal_steps) = if self.mode == PasteMode::Direct {
            (Vec::new(), Vec::new())
        } else {
            (
                linux_resolved_paste_chord_steps(&self.conn, PasteMode::Standard)?,
                linux_resolved_paste_chord_steps(&self.conn, PasteMode::Terminal)?,
            )
        };
        self.modifier_keycodes = modifier_keycodes;
        self.standard_steps = standard_steps;
        self.terminal_steps = terminal_steps;
        Ok(())
    }

    fn keymap(&self) -> Result<[u8; 32]> {
        Ok(self
            .conn
            .query_keymap()
            .context("could not query X11 modifiers before paste")?
            .reply()
            .context("could not read X11 modifiers before paste")?
            .keys)
    }
}

fn x11_modifier_held(keymap: &[u8; 32], modifier_keycodes: &[u8]) -> bool {
    modifier_keycodes
        .iter()
        .any(|keycode| x11::keycode_down(keymap, *keycode))
}

fn linux_modifier_cleanup_keysyms() -> &'static [u32] {
    &[
        x11::CONTROL_L_KEYSYM,
        x11::CONTROL_R_KEYSYM,
        x11::SHIFT_L_KEYSYM,
        x11::SHIFT_R_KEYSYM,
        x11::ALT_L_KEYSYM,
        x11::ALT_R_KEYSYM,
        x11::SUPER_L_KEYSYM,
        x11::SUPER_R_KEYSYM,
        x11::ISO_LEVEL3_SHIFT_KEYSYM,
    ]
}

fn linux_paste_chord_steps(mode: PasteMode) -> Vec<X11KeyStep> {
    let mut steps = vec![X11KeyStep {
        keysym: x11::CONTROL_L_KEYSYM,
        press: true,
    }];
    if mode == PasteMode::Terminal {
        steps.push(X11KeyStep {
            keysym: x11::SHIFT_L_KEYSYM,
            press: true,
        });
    }
    steps.push(X11KeyStep {
        keysym: x11::V_KEYSYM,
        press: true,
    });
    steps.push(X11KeyStep {
        keysym: x11::V_KEYSYM,
        press: false,
    });
    if mode == PasteMode::Terminal {
        steps.push(X11KeyStep {
            keysym: x11::SHIFT_L_KEYSYM,
            press: false,
        });
    }
    steps.push(X11KeyStep {
        keysym: x11::CONTROL_L_KEYSYM,
        press: false,
    });
    steps
}

fn linux_resolved_paste_chord_steps(
    conn: &RustConnection,
    mode: PasteMode,
) -> Result<Vec<ResolvedX11KeyStep>> {
    linux_paste_chord_steps(mode)
        .into_iter()
        .map(|step| {
            Ok(ResolvedX11KeyStep {
                keycode: x11::keycode_for_keysym(conn, step.keysym)?,
                press: step.press,
            })
        })
        .collect()
}

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

struct X11ConnectionKeySink<'a> {
    conn: &'a x11rb::rust_connection::RustConnection,
    root: u32,
}

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

fn send_x11_paste_chord_with_modifier_flush<S: X11KeySink>(
    sink: &mut S,
    steps: &[ResolvedX11KeyStep],
    modifier_keycodes: &[u8],
    keymap: &[u8; 32],
) -> Result<PasteDispatch> {
    // The chord and cleanup release modifiers, which would stop another held
    // push-to-talk capture or interfere with the user's current shortcut.
    // This instant check guards changes after the bounded readiness wait.
    // QueryKeymap cannot distinguish a lost release from a genuinely held key;
    // clearing an apparent "stuck" modifier here would release the latter too.
    if x11_modifier_held(keymap, modifier_keycodes) {
        return Ok(PasteDispatch::SkippedUnsafeModifiers);
    }
    send_x11_key_steps(sink, steps)?;
    // Best-effort blanket releases protect against focus changes during the
    // paste chord that leave the X server believing a modifier is still held.
    let _ = flush_x11_modifier_releases(sink, modifier_keycodes);
    Ok(PasteDispatch::Posted)
}

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

fn combine_primary_cleanup_error(primary: anyhow::Error, cleanup: Result<()>) -> Result<()> {
    match cleanup {
        Ok(()) => Err(primary),
        Err(cleanup_err) => Err(anyhow::anyhow!(
            "{primary:#}; cleanup while releasing pressed XTest keys failed: {cleanup_err:#}"
        )),
    }
}

/// Check that the display exposes the XTest extension used for insertion.
///
/// # Returns
///
/// `Ok(())` when XTest version negotiation succeeds.
///
/// # Errors
///
/// Returns an error when connecting or querying XTest fails.
pub(super) fn linux_x11_xtest_preflight() -> Result<()> {
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
#[path = "x11_paste_tests.rs"]
mod tests;
