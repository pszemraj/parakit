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
/// X11 keysym for right Shift.
pub(crate) const SHIFT_R_KEYSYM: u32 = 0xffe2;
/// X11 keysym for left Alt.
pub(crate) const ALT_L_KEYSYM: u32 = 0xffe9;
/// X11 keysym for right Alt.
pub(crate) const ALT_R_KEYSYM: u32 = 0xffea;
/// X11 keysym for left Super.
pub(crate) const SUPER_L_KEYSYM: u32 = 0xffeb;
/// X11 keysym for right Super.
pub(crate) const SUPER_R_KEYSYM: u32 = 0xffec;
/// X11 keysym commonly emitted by AltGr.
pub(crate) const ISO_LEVEL3_SHIFT_KEYSYM: u32 = 0xfe03;
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
    keycodes_for_keysyms(conn, &[keysym])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("could not map X11 keysym {keysym} to a keycode"))
}

/// Map X11 keysyms to every keycode that emits at least one of them.
///
/// This intentionally returns every match: user remaps can place Control or
/// another modifier on additional physical keys while leaving the original
/// mapping present.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `keysyms` - Modifier or other keysyms to locate.
///
/// # Returns
///
/// Every active keycode that emits at least one requested keysym.
///
/// # Errors
///
/// Returns an error when the active keyboard mapping cannot be requested or
/// read.
pub(crate) fn keycodes_for_keysyms(conn: &RustConnection, keysyms: &[u32]) -> Result<Vec<Keycode>> {
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

    Ok(keycodes_for_mapping(
        min_keycode,
        keysyms_per_keycode,
        &mapping.keysyms,
        keysyms,
    ))
}

fn keycodes_for_mapping(
    min_keycode: Keycode,
    keysyms_per_keycode: usize,
    mapping: &[u32],
    requested: &[u32],
) -> Vec<Keycode> {
    if keysyms_per_keycode == 0 {
        return Vec::new();
    }
    mapping
        .chunks(keysyms_per_keycode)
        .enumerate()
        .filter_map(|(offset, mapped)| {
            mapped
                .iter()
                .any(|keysym| requested.contains(keysym))
                .then_some(min_keycode + offset as u8)
        })
        .collect()
}

/// Whether `keycode` is pressed in an X11 `QueryKeymap` bitmap.
///
/// # Arguments
///
/// * `keys` - The 256-bit `QueryKeymap` state.
/// * `keycode` - X11 keycode to inspect.
///
/// # Returns
///
/// True when the keycode's bit is set.
///
/// # Panics
///
/// Does not panic: an X11 keycode fits the fixed bitmap, and the bit offset is
/// reduced modulo eight before shifting.
pub(crate) fn keycode_down(keys: &[u8; 32], keycode: Keycode) -> bool {
    let index = usize::from(keycode / 8);
    let bit = keycode % 8;
    keys.get(index)
        .is_some_and(|byte| byte & (1_u8 << bit) != 0)
}

/// Return the EWMH active toplevel window when the window manager exposes it.
///
/// # Arguments
///
/// * `conn` - Active X11 connection.
/// * `root` - Root window for the active screen.
///
/// # Returns
///
/// The active toplevel window, or `None` when unavailable.
///
/// # Errors
///
/// Returns an error when X11 rejects the atom or property request.
pub(crate) fn active_window(conn: &RustConnection, root: Window) -> Result<Option<Window>> {
    let atom = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .context("could not request X11 _NET_ACTIVE_WINDOW atom")?
        .reply()
        .context("could not read X11 _NET_ACTIVE_WINDOW atom")?
        .atom;
    let reply = conn
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .context("could not request X11 _NET_ACTIVE_WINDOW")?
        .reply()
        .context("could not read X11 _NET_ACTIVE_WINDOW")?;
    Ok(reply
        .value32()
        .and_then(|mut values| values.find(|window| *window != x11rb::NONE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_returns_every_keycode_for_requested_modifiers() {
        let mapping = [
            SPACE_KEYSYM,
            0,
            CONTROL_L_KEYSYM,
            0,
            CONTROL_L_KEYSYM,
            ISO_LEVEL3_SHIFT_KEYSYM,
            b'a' as u32,
            0,
        ];
        assert_eq!(
            keycodes_for_mapping(8, 2, &mapping, &[CONTROL_L_KEYSYM, ISO_LEVEL3_SHIFT_KEYSYM]),
            vec![9, 10]
        );
    }

    #[test]
    fn keymap_bitmap_handles_bounds_and_pressed_bits() {
        let mut keys = [0_u8; 32];
        keys[4] |= 1 << 5;
        assert!(keycode_down(&keys, 37));
        assert!(!keycode_down(&keys, 36));
    }
}
