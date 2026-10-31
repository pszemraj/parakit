//! Linux hotkey backend regression tests.

use super::*;

fn base_time() -> Instant {
    Instant::now()
}

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

fn physical(ctrl: bool, space: bool) -> PhysicalHotkeyState {
    PhysicalHotkeyState {
        ctrl,
        space,
        alt_gr: false,
    }
}

#[test]
fn registered_hotkey_physical_poll_keeps_recording_while_chord_is_down() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();

    state.event(
        RegisteredHotKeyState::Pressed,
        DictationMode::Plain,
        physical(true, true),
        now,
    );

    assert_eq!(state.physical_poll(physical(true, true), at(now, 50)), None);
    assert!(state.is_recording());
}

#[test]
fn registered_hotkey_release_is_ignored_while_physical_chord_is_still_down() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();

    state.event(
        RegisteredHotKeyState::Pressed,
        DictationMode::Plain,
        physical(true, true),
        now,
    );

    assert_eq!(
        state.event(
            RegisteredHotKeyState::Released,
            DictationMode::Plain,
            physical(true, true),
            at(now, 50)
        ),
        None
    );
    assert!(state.is_recording());
}

#[test]
fn registered_hotkey_waits_for_space_release_after_ctrl_first_stop() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();

    state.event(
        RegisteredHotKeyState::Pressed,
        DictationMode::Plain,
        physical(true, true),
        now,
    );
    assert_eq!(
        state.physical_poll(physical(false, true), at(now, 50)),
        Some(HotkeyAction::Stop {
            stopped_at: at(now, 50)
        })
    );

    assert!(!state.is_recording());
    assert!(state.needs_physical_poll());
    assert_eq!(
        state.event(
            RegisteredHotKeyState::Pressed,
            DictationMode::Plain,
            physical(true, true),
            at(now, 75)
        ),
        None
    );

    assert_eq!(
        state.physical_poll(physical(false, false), at(now, 100)),
        None
    );
    assert!(!state.needs_physical_poll());
}

#[test]
fn registered_hotkey_physical_state_uses_refreshed_keycodes() {
    let mut keys = [0_u8; 32];
    keys[12] |= 1 << 1; // keycode 97: remapped Control
    keys[8] |= 1 << 1; // keycode 65: Space

    assert_eq!(
        physical_state_from_keycodes(&keys, &[37], &[65], &[108]),
        physical(false, true),
        "the startup Control mapping is stale"
    );
    assert_eq!(
        physical_state_from_keycodes(&keys, &[97], &[65], &[108]),
        physical(true, true),
        "a MappingNotify refresh must use the new Control keycode"
    );
}

#[test]
fn passive_listen_handler_emits_transitions_without_returning_suppression() {
    use std::time::SystemTime;

    fn event(event_type: EventType) -> Event {
        Event {
            time: SystemTime::now(),
            name: None,
            event_type,
        }
    }

    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let (tx, rx) = crossbeam_channel::unbounded();
    handle_listen_event(event(EventType::KeyPress(Key::ControlLeft)), &state, &tx);
    handle_listen_event(event(EventType::KeyPress(Key::Space)), &state, &tx);
    let pressed = rx.recv().unwrap();
    handle_listen_event(event(EventType::KeyRelease(Key::Space)), &state, &tx);
    let released = rx.recv().unwrap();

    assert!(matches!(pressed, HotkeyTransition::Pressed { .. }));
    assert!(matches!(released, HotkeyTransition::Released { .. }));
    assert!(rx.try_recv().is_err());
}

#[test]
fn registered_hotkey_press_uses_ordinary_capture_transitions() {
    let now = base_time();
    let mut latch = RegisteredHotkeyLatch::default();
    let (tx, rx) = crossbeam_channel::unbounded();
    let pressed = latch
        .event(
            RegisteredHotKeyState::Pressed,
            DictationMode::Plain,
            physical(true, true),
            now,
        )
        .unwrap();
    send_hotkey_transition(pressed, &tx);
    let released = latch
        .event(
            RegisteredHotKeyState::Released,
            DictationMode::Plain,
            physical(false, false),
            at(now, 50),
        )
        .unwrap();
    send_hotkey_transition(released, &tx);
    assert_eq!(
        rx.recv().unwrap(),
        HotkeyTransition::Pressed {
            at: now,
            mode: DictationMode::Plain
        }
    );
    assert_eq!(
        rx.recv().unwrap(),
        HotkeyTransition::Released { at: at(now, 50) }
    );
}

#[test]
fn registered_context_chord_does_not_restart_when_alt_changes() {
    let now = base_time();
    let mut latch = RegisteredHotkeyLatch::default();
    assert_eq!(
        latch.event(
            RegisteredHotKeyState::Pressed,
            DictationMode::WithContext,
            physical(true, true),
            now
        ),
        Some(HotkeyAction::Start {
            started_at: now,
            mode: DictationMode::WithContext
        })
    );
    assert_eq!(
        latch.event(
            RegisteredHotKeyState::Released,
            DictationMode::WithContext,
            physical(true, true),
            at(now, 20)
        ),
        None
    );
    assert_eq!(
        latch.event(
            RegisteredHotKeyState::Pressed,
            DictationMode::Plain,
            physical(true, true),
            at(now, 30)
        ),
        None
    );
    assert_eq!(
        latch.physical_poll(physical(true, false), at(now, 40)),
        Some(HotkeyAction::Stop {
            stopped_at: at(now, 40)
        })
    );
}

#[test]
fn registered_context_chord_rejects_alt_gr() {
    let mut latch = RegisteredHotkeyLatch::default();
    assert_eq!(
        latch.event(
            RegisteredHotKeyState::Pressed,
            DictationMode::WithContext,
            PhysicalHotkeyState {
                ctrl: true,
                space: true,
                alt_gr: true
            },
            base_time()
        ),
        None
    );
    assert!(!latch.is_recording());
}

#[test]
#[ignore = "requires an isolated X11 server; synthesizes Ctrl/Alt/Space"]
fn registered_x11_context_chords_and_optional_conflict() -> anyhow::Result<()> {
    use crate::daemon::desktop::x11::{
        keycode_for_keysym, root_window, ALT_L_KEYSYM, CONTROL_L_KEYSYM, SPACE_KEYSYM,
    };
    use crate::daemon::logging::LogLevel;
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;

    let busy = std::env::var_os("PARAKIT_TEST_CONTEXT_BUSY").is_some();
    let enabled = std::env::var_os("PARAKIT_TEST_CONTEXT_DISABLED").is_none();
    let manager = GlobalHotKeyManager::new().map_err(|err| anyhow::anyhow!("{err}"))?;
    if busy {
        manager
            .register(ctrl_alt_space_hotkey())
            .map_err(|err| anyhow::anyhow!("{err}"))?;
    }
    let (sender, receiver) = crossbeam_channel::unbounded();
    // The isolated test process owns this listener and its server connection.
    std::thread::spawn(move || {
        run_linux_registered_hotkey_loop(sender, enabled, &Logger::new(LogLevel::Normal))
            .expect("registered hotkey loop");
    });
    std::thread::sleep(Duration::from_millis(250));
    let (connection, screen) = x11rb::rust_connection::RustConnection::connect(None)?;
    let root = root_window(&connection, screen)?;
    let control = keycode_for_keysym(&connection, CONTROL_L_KEYSYM)?;
    let alt = keycode_for_keysym(&connection, ALT_L_KEYSYM)?;
    let space = keycode_for_keysym(&connection, SPACE_KEYSYM)?;
    let key = |code, pressed| -> anyhow::Result<()> {
        connection
            .xtest_fake_input(
                if pressed {
                    KEY_PRESS_EVENT
                } else {
                    KEY_RELEASE_EVENT
                },
                code,
                0,
                root,
                0,
                0,
                0,
            )?
            .check()?;
        Ok(())
    };
    let expect_press = |expected| -> anyhow::Result<()> {
        let event = receiver.recv_timeout(Duration::from_secs(2))?;
        anyhow::ensure!(
            matches!(event, HotkeyTransition::Pressed { mode, .. } if mode == expected),
            "unexpected press {event:?}"
        );
        Ok(())
    };
    let expect_release = || -> anyhow::Result<()> {
        let event = receiver.recv_timeout(Duration::from_secs(2))?;
        anyhow::ensure!(
            matches!(event, HotkeyTransition::Released { .. }),
            "unexpected release {event:?}"
        );
        Ok(())
    };

    let result = (|| -> anyhow::Result<()> {
        key(control, true)?;
        key(space, true)?;
        expect_press(DictationMode::Plain)?;
        key(control, false)?;
        expect_release()?;
        key(control, true)?;
        anyhow::ensure!(
            receiver.recv_timeout(Duration::from_millis(120)).is_err(),
            "held Space rearmed capture"
        );
        key(space, false)?;
        key(control, false)?;
        std::thread::sleep(Duration::from_millis(50));

        key(control, true)?;
        key(alt, true)?;
        key(space, true)?;
        if busy || !enabled {
            anyhow::ensure!(
                receiver.recv_timeout(Duration::from_millis(120)).is_err(),
                "unavailable context chord started capture"
            );
            key(space, false)?;
            key(alt, false)?;
            key(control, false)?;
        } else {
            expect_press(DictationMode::WithContext)?;
            key(alt, false)?;
            anyhow::ensure!(
                receiver.recv_timeout(Duration::from_millis(120)).is_err(),
                "Alt release changed capture"
            );
            key(space, false)?;
            expect_release()?;
            key(control, false)?;
        }
        anyhow::ensure!(
            receiver.recv_timeout(Duration::from_millis(120)).is_err(),
            "extra hotkey transition"
        );
        Ok(())
    })();
    let release_space = key(space, false);
    let release_alt = key(alt, false);
    let release_control = key(control, false);
    result?;
    release_space?;
    release_alt?;
    release_control?;
    Ok(())
}

#[test]
fn evdev_input_files_are_opened_nonblocking() {
    use std::os::fd::AsRawFd;

    let dir = crate::test_support::fixture_root("parakit-hotkey-test", "evdev-input");
    let path = dir.join("event-test");
    std::fs::write(&path, b"").expect("create test input file");

    let file = open_evdev_input(&path).expect("open test input file");
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(flags & libc::O_NONBLOCK, 0);
}
