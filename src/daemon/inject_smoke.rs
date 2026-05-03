//! Linux X11 insertion smoke test.

use anyhow::{Context, Result};
use std::thread;
use std::time::{Duration, Instant};
use x11rb::rust_connection::RustConnection;

use super::{Injector, PasteMode};

/// Verify that the configured X11 paste chord reaches a temporary focused window.
///
/// # Returns
///
/// `Ok(())` when the smoke window observes the paste key press and release.
///
/// # Errors
///
/// Returns an error if X11 setup, paste synthesis, event observation, or cleanup
/// fails.
pub(super) fn linux_x11_paste_smoke_test(mode: PasteMode) -> Result<()> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt, CreateWindowAux, EventMask, InputFocus, WindowClass,
    };

    let (conn, screen_num) = RustConnection::connect(None).context("could not connect to X11")?;
    let v_keycode = super::super::x11::keycode_for_keysym(&conn, super::super::x11::V_KEYSYM)?;
    let screen = super::super::x11::screen(&conn, screen_num)?;
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

fn linux_wait_for_focus(conn: &RustConnection, window: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        if super::linux_current_input_focus(conn)? == window {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }

    anyhow::bail!("X11 smoke-test window did not receive input focus")
}

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
