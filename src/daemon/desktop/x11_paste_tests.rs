//! Regression tests for X11 paste synthesis and modifier guards.

use super::super::Injector;
use super::*;
use crate::daemon::desktop::x11::wait_for_modifier_release as wait_for_x11_modifier_release;
use std::time::Duration;

#[test]
fn linux_xtest_paste_chord_steps_are_ordered() {
    assert_eq!(
        linux_paste_chord_steps(PasteMode::Standard),
        vec![
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, false),
        ]
    );
    assert_eq!(
        linux_paste_chord_steps(PasteMode::Terminal),
        vec![
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::SHIFT_L_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, true),
            x11_key_step(crate::daemon::x11::V_KEYSYM, false),
            x11_key_step(crate::daemon::x11::SHIFT_L_KEYSYM, false),
            x11_key_step(crate::daemon::x11::CONTROL_L_KEYSYM, false),
        ]
    );
}

fn x11_key_step(keysym: u32, press: bool) -> X11KeyStep {
    X11KeyStep { keysym, press }
}

#[derive(Default)]
struct MockX11KeySink {
    events: Vec<(u8, bool)>,
    fail_on: Option<(u8, bool)>,
    fail_cleanup_on: Option<u8>,
    flushes: usize,
}

impl X11KeySink for MockX11KeySink {
    fn key(&mut self, keycode: u8, press: bool) -> Result<()> {
        self.events.push((keycode, press));
        if self.fail_on == Some((keycode, press)) {
            anyhow::bail!("primary failure {keycode}:{press}");
        }
        if !press && self.fail_cleanup_on == Some(keycode) {
            anyhow::bail!("cleanup failure {keycode}");
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

fn three_pressed_key_steps() -> [ResolvedX11KeyStep; 3] {
    [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: true,
        },
    ]
}

#[test]
fn xtest_cleanup_releases_pressed_keys_after_primary_error() {
    let mut sink = MockX11KeySink {
        fail_on: Some((3, true)),
        ..MockX11KeySink::default()
    };
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps())
        .expect_err("primary failure should be reported");

    assert!(format!("{err:#}").contains("primary failure"));
    assert_eq!(
        sink.events,
        vec![(1, true), (2, true), (3, true), (2, false), (1, false)]
    );
}

#[test]
fn xtest_cleanup_reports_primary_and_cleanup_errors() {
    let mut sink = MockX11KeySink {
        fail_on: Some((3, true)),
        fail_cleanup_on: Some(2),
        ..MockX11KeySink::default()
    };
    let err = send_x11_key_steps(&mut sink, &three_pressed_key_steps())
        .expect_err("primary and cleanup failures should be reported");
    let message = format!("{err:#}");
    assert!(message.contains("primary failure"));
    assert!(message.contains("cleanup while releasing pressed XTest keys failed"));
    assert!(message.contains("cleanup failure"));
}

#[test]
fn xtest_success_releases_only_chord_keys() {
    let mut sink = MockX11KeySink::default();
    let steps = [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 3,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 1,
            press: false,
        },
    ];

    send_x11_key_steps(&mut sink, &steps).expect("paste chord should succeed");

    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (3, true),
            (3, false),
            (2, false),
            (1, false)
        ]
    );
}

#[test]
fn xtest_paste_chord_success_flushes_all_cleanup_modifiers() {
    let mut sink = MockX11KeySink::default();
    let steps = [
        ResolvedX11KeyStep {
            keycode: 1,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: true,
        },
        ResolvedX11KeyStep {
            keycode: 2,
            press: false,
        },
        ResolvedX11KeyStep {
            keycode: 1,
            press: false,
        },
    ];

    assert_eq!(
        send_x11_paste_chord_with_modifier_flush(&mut sink, &steps, &[1, 3, 4], &[0; 32])
            .expect("paste chord should succeed"),
        PasteDispatch::Posted
    );

    assert_eq!(
        sink.events,
        vec![
            (1, true),
            (2, true),
            (2, false),
            (1, false),
            (1, false),
            (3, false),
            (4, false)
        ]
    );
    assert_eq!(sink.flushes, 2);
}

#[test]
fn xtest_paste_with_held_modifiers_does_not_emit_input() {
    let modifier_keycodes = [37, 105, 50, 62, 64, 108, 133, 134];
    for held in modifier_keycodes {
        let mut sink = MockX11KeySink::default();
        let mut keymap = [0; 32];
        keymap[usize::from(held / 8)] |= 1 << (held % 8);
        let dispatch = send_x11_paste_chord_with_modifier_flush(
            &mut sink,
            &three_pressed_key_steps(),
            &modifier_keycodes,
            &keymap,
        )
        .expect("a held modifier should safely withhold the paste chord");
        assert_eq!(dispatch, PasteDispatch::SkippedUnsafeModifiers);
        assert!(sink.events.is_empty(), "held keycode {held}");
        assert_eq!(sink.flushes, 0, "held keycode {held}");
    }
}

#[test]
fn x11_modifier_release_wait_allows_late_ctrl_release_after_ptt_stop() {
    // Space released first stops recording; Ctrl lifts a few polls later.
    let ctrl_l = 37;
    let mut keymap = [0; 32];
    keymap[usize::from(ctrl_l / 8)] |= 1 << (ctrl_l % 8);
    let mut polls = 0;
    let ready = wait_for_x11_modifier_release(Duration::from_secs(2), || {
        polls += 1;
        if polls == 3 {
            keymap = [0; 32];
        }
        Ok(x11_modifier_held(&keymap, &[ctrl_l, 50]))
    })
    .expect("keymap polling should succeed");
    assert!(ready);
    assert_eq!(polls, 3);
}

#[test]
fn x11_modifier_release_wait_times_out_and_propagates_query_errors() {
    let mut polls = 0;
    let held = wait_for_x11_modifier_release(Duration::ZERO, || {
        polls += 1;
        Ok(true)
    })
    .expect("keymap polling should succeed");
    assert!(!held, "an overlapping capture keeps its modifiers held");
    assert_eq!(polls, 1);

    let err = wait_for_x11_modifier_release(Duration::from_secs(2), || {
        anyhow::bail!("X11 connection lost")
    })
    .expect_err("query failures should reach the caller");
    assert!(err.to_string().contains("X11 connection lost"));
}

#[test]
#[ignore = "requires a running X11 display; only queries modifiers and closes its own connection"]
fn linux_direct_injector_reopens_failed_modifier_connection_without_paste_setup() {
    use std::os::fd::AsRawFd;

    let mut injector = Injector::new().unwrap();
    injector.linux_direct_modifiers_held().unwrap();
    let direct = injector.x11_direct.as_ref().unwrap();
    assert!(direct.standard_steps.is_empty());
    assert!(direct.terminal_steps.is_empty());
    assert!(injector.x11_paste.is_none());
    assert!(injector.clipboard.is_none());
    // SAFETY: This descriptor belongs to this test's dedicated query
    // connection. Shutdown neither closes it nor affects other X11 clients.
    let result = unsafe { libc::shutdown(direct.conn.stream().as_raw_fd(), libc::SHUT_RDWR) };
    assert_eq!(result, 0);
    assert!(injector.linux_direct_modifiers_held().is_err());
    assert!(injector.x11_direct.is_none());
    injector.linux_direct_modifiers_held().unwrap();
    assert!(injector.x11_direct.is_some());
}
