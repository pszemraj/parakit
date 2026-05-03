//! Shared X11 helpers used by Linux daemon backends.

use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, Keycode, Screen, Window};
use x11rb::rust_connection::RustConnection;

/// X11 keysym for Space.
pub(crate) const SPACE_KEYSYM: u32 = b' ' as u32;
/// X11 keysym for left Control.
pub(crate) const CONTROL_L_KEYSYM: u32 = 0xffe3;
/// X11 keysym for right Control.
pub(crate) const CONTROL_R_KEYSYM: u32 = 0xffe4;
/// X11 keysym for left Shift.
pub(crate) const SHIFT_L_KEYSYM: u32 = 0xffe1;
/// X11 keysym for lowercase `v`.
pub(crate) const V_KEYSYM: u32 = b'v' as u32;

/// Return the requested X11 screen.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `screen_num` - Screen index returned by `RustConnection::connect`.
///
/// # Returns
///
/// The screen metadata for `screen_num`.
///
/// # Errors
///
/// Returns an error if the display did not expose that screen index.
pub(crate) fn screen(conn: &RustConnection, screen_num: usize) -> Result<&Screen> {
    conn.setup()
        .roots
        .get(screen_num)
        .context("X11 display did not expose the requested screen")
}

/// Return the root window for the requested screen.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `screen_num` - Screen index returned by `RustConnection::connect`.
///
/// # Returns
///
/// The root window for the selected screen.
///
/// # Errors
///
/// Returns an error if the display did not expose that screen index.
pub(crate) fn root_window(conn: &RustConnection, screen_num: usize) -> Result<Window> {
    Ok(screen(conn, screen_num)?.root)
}

/// Map an X11 keysym to the active keyboard keycode.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `keysym` - X11 keysym to resolve.
///
/// # Returns
///
/// The first keycode in the active mapping that emits `keysym`.
///
/// # Errors
///
/// Returns an error if the keyboard mapping cannot be read or does not contain
/// the requested keysym.
pub(crate) fn keycode_for_keysym(conn: &RustConnection, keysym: u32) -> Result<Keycode> {
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
