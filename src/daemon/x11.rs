//! Shared X11 helpers used by Linux daemon backends.

use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Keycode, Screen, Window};
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

/// Return the `WM_CLASS` class half for `window` or one of its parents.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `root` - Root window for the active screen.
/// * `window` - Focused window to inspect.
///
/// # Returns
///
/// The class half of `WM_CLASS`, or `None` when no inspected window exposes it.
///
/// # Errors
///
/// Returns an error when X11 rejects the atom, property, or tree request.
pub(crate) fn wm_class_for_window(
    conn: &RustConnection,
    root: Window,
    mut window: Window,
) -> Result<Option<String>> {
    let wm_class = conn
        .intern_atom(false, b"WM_CLASS")
        .context("could not request X11 WM_CLASS atom")?
        .reply()
        .context("could not read X11 WM_CLASS atom")?
        .atom;

    for _ in 0..32 {
        let reply = conn
            .get_property(false, window, wm_class, AtomEnum::STRING, 0, 1024)
            .context("could not request X11 WM_CLASS")?
            .reply()
            .context("could not read X11 WM_CLASS")?;
        if let Some(class) = parse_wm_class(&reply.value) {
            return Ok(Some(class));
        }
        if window == root {
            break;
        }
        let tree = conn
            .query_tree(window)
            .context("could not request X11 window tree")?
            .reply()
            .context("could not read X11 window tree")?;
        if tree.parent == x11rb::NONE || tree.parent == window {
            break;
        }
        window = tree.parent;
    }

    Ok(None)
}

/// Parse the class half of an X11 `WM_CLASS` property.
///
/// # Arguments
///
/// * `value` - NUL-separated instance/class bytes.
///
/// # Returns
///
/// The class half when it is valid UTF-8.
pub(crate) fn parse_wm_class(value: &[u8]) -> Option<String> {
    let mut parts = value
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok());
    parts.next_back().map(str::to_string)
}
