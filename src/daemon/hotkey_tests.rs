//! Hotkey backend regression tests.

use super::*;

fn base_time() -> Instant {
    Instant::now()
}

#[test]
fn ctrl_space_starts_and_stops() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::ControlLeft, now), (None, false));
    assert_eq!(
        state.press(Key::Space, now + Duration::from_millis(10)),
        (
            Some(HotkeyAction::Start {
                started_at: now + Duration::from_millis(10)
            }),
            true
        )
    );
    assert_eq!(
        state.release(Key::Space, now + Duration::from_millis(300)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: now + Duration::from_millis(300)
            }),
            true
        )
    );
}

#[test]
fn ctrl_release_before_space_stops_without_suppressing_ctrl_release() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, now + Duration::from_millis(10));
    assert_eq!(
        state.release(Key::ControlLeft, now + Duration::from_millis(50)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: now + Duration::from_millis(50)
            }),
            false
        )
    );
    assert!(!state.is_recording());
}

#[test]
fn ctrl_repress_while_space_held_does_not_restart_recording() {
    let now = base_time();
    let mut state = HotkeyState::default();

    state.press(Key::ControlLeft, now);
    state.press(Key::Space, now + Duration::from_millis(10));

    assert_eq!(
        state.release(Key::ControlLeft, now + Duration::from_millis(50)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: now + Duration::from_millis(50),
            }),
            false,
        )
    );

    assert_eq!(
        state.press(Key::ControlLeft, now + Duration::from_millis(60)),
        (None, false)
    );
    assert_eq!(
        state.press(
            Key::Space,
            now + HOTKEY_DEBOUNCE + Duration::from_millis(20)
        ),
        (None, true)
    );

    assert!(!state.is_recording());

    assert_eq!(
        state.release(
            Key::Space,
            now + HOTKEY_DEBOUNCE + Duration::from_millis(30)
        ),
        (None, true)
    );
}

#[test]
fn repeated_space_press_while_held_is_suppressed_without_restart() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    assert_eq!(
        state.press(Key::Space, now + Duration::from_millis(10)),
        (
            Some(HotkeyAction::Start {
                started_at: now + Duration::from_millis(10)
            }),
            true
        )
    );
    assert_eq!(
        state.press(Key::Space, now + Duration::from_millis(20)),
        (None, true)
    );
    assert!(state.is_recording());
}

#[test]
fn registered_hotkey_press_release_starts_and_stops_once() {
    let now = base_time();
    let mut state = RecordingLatch::default();
    assert_eq!(
        state.start(now),
        Some(HotkeyAction::Start { started_at: now })
    );
    assert_eq!(state.start(now + Duration::from_millis(10)), None);
    assert_eq!(
        state.stop(now + Duration::from_millis(300)),
        Some(HotkeyAction::Stop {
            stopped_at: now + Duration::from_millis(300)
        })
    );
    assert_eq!(state.stop(now + Duration::from_millis(310)), None);
}

#[test]
fn hotkey_actions_emit_logical_transitions_only() {
    let now = base_time();
    let (tx, rx) = crossbeam_channel::unbounded();

    send_hotkey_transition(HotkeyAction::Start { started_at: now }, &tx);
    send_hotkey_transition(
        HotkeyAction::Stop {
            stopped_at: now + Duration::from_millis(250),
        },
        &tx,
    );

    assert_eq!(rx.recv().unwrap(), HotkeyTransition::Pressed { at: now });
    assert_eq!(
        rx.recv().unwrap(),
        HotkeyTransition::Released {
            at: now + Duration::from_millis(250)
        }
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn rapid_double_press_is_ignored_and_suppressed() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, now + Duration::from_millis(10));
    state.release(Key::Space, now + Duration::from_millis(20));
    assert_eq!(
        state.press(Key::Space, now + Duration::from_millis(80)),
        (None, true)
    );
    assert_eq!(
        state.release(Key::Space, now + Duration::from_millis(90)),
        (None, true)
    );
    assert!(!state.is_recording());
}

#[test]
fn ctrl_shift_space_does_not_start_or_suppress() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::ShiftLeft, now + Duration::from_millis(5));
    assert_eq!(
        state.press(Key::Space, now + Duration::from_millis(10)),
        (None, false)
    );
    assert!(!state.is_recording());
}

#[test]
fn unrelated_keys_pass_through() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::KeyA, now), (None, false));
    assert_eq!(
        state.release(Key::KeyA, now + Duration::from_millis(10)),
        (None, false)
    );
}

#[test]
fn backend_labels_are_stable() {
    for (backend, label) in [
        (HotkeyBackend::Auto, "auto"),
        (HotkeyBackend::Desktop, "desktop"),
    ] {
        assert_eq!(backend.label(), label);
    }
    #[cfg(target_os = "linux")]
    for (backend, label) in [
        (HotkeyBackend::X11GlobalHotkey, "x11-global-hotkey"),
        (HotkeyBackend::X11Listen, "x11-listen"),
        (
            HotkeyBackend::EvdevProxyExperimental,
            "evdev-proxy-experimental",
        ),
    ] {
        assert_eq!(backend.label(), label);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_backend_aliases_parse_to_stable_variants() {
    fn parse(value: &str) -> HotkeyBackend {
        <HotkeyBackend as clap::ValueEnum>::from_str(value, false).unwrap()
    }

    assert_eq!(parse("x11-global-hotkey"), HotkeyBackend::X11GlobalHotkey);
    assert_eq!(parse("x11-listen"), HotkeyBackend::X11Listen);
    assert_eq!(
        parse("evdev-proxy-experimental"),
        HotkeyBackend::EvdevProxyExperimental
    );
    assert_eq!(parse("evdev-proxy"), HotkeyBackend::EvdevProxyExperimental);
    assert!(<HotkeyBackend as clap::ValueEnum>::from_str("evdev", false).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn x11_keymap_bit_probe_detects_down_keycodes() {
    let mut keys = [0_u8; 32];
    keys[4] = 0b0010_0000;

    assert!(keycode_down(&keys, 37));
    assert!(!keycode_down(&keys, 36));
    assert!(!keycode_down(&keys, 255));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_backend_routing_helpers_classify_backends() {
    for (backend, registered, passive, evdev) in [
        (HotkeyBackend::Auto, true, false, false),
        (HotkeyBackend::Desktop, true, false, false),
        (HotkeyBackend::X11GlobalHotkey, true, false, false),
        (HotkeyBackend::X11Listen, false, true, false),
        (HotkeyBackend::EvdevProxyExperimental, false, false, true),
    ] {
        assert_eq!(backend.uses_registered_x11(), registered);
        assert_eq!(backend.uses_passive_x11_listen(), passive);
        assert_eq!(backend.uses_evdev_proxy(), evdev);
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
#[test]
fn evdev_input_files_are_opened_nonblocking() {
    use std::os::fd::AsRawFd;
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before UNIX epoch")
        .as_nanos();
    let dir = PathBuf::from(format!(
        "target/tmp/parakit-hotkey-test-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create test directory");
    let path = dir.join("event-test");
    std::fs::write(&path, b"").expect("create test input file");

    let file = open_evdev_input(&path).expect("open test input file");
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(flags & libc::O_NONBLOCK, 0);
}
