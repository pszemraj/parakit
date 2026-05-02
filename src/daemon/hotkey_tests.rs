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
                started_at: now + Duration::from_millis(10),
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
                started_at: now + Duration::from_millis(10),
                stopped_at: now + Duration::from_millis(50)
            }),
            false
        )
    );
    assert!(!state.recording);
}

#[test]
fn held_space_after_ctrl_release_does_not_restart_after_debounce() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, now + Duration::from_millis(10));
    state.release(Key::ControlLeft, now + Duration::from_millis(50));

    assert_eq!(
        state.press(
            Key::ControlLeft,
            now + HOTKEY_DEBOUNCE + Duration::from_millis(10)
        ),
        (None, false)
    );
    assert_eq!(
        state.press(
            Key::Space,
            now + HOTKEY_DEBOUNCE + Duration::from_millis(20)
        ),
        (None, true)
    );
    assert_eq!(
        state.release(
            Key::Space,
            now + HOTKEY_DEBOUNCE + Duration::from_millis(30)
        ),
        (None, true)
    );
    assert!(!state.recording);
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
                started_at: now + Duration::from_millis(10),
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
        state.press(Key::Space, now + Duration::from_millis(70)),
        (None, true)
    );

    assert!(!state.recording);

    assert_eq!(
        state.release(Key::Space, now + Duration::from_millis(80)),
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
    assert!(state.recording);
}

#[test]
fn registered_hotkey_press_release_starts_and_stops_once() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();
    assert_eq!(
        state.press(now),
        Some(HotkeyAction::Start { started_at: now })
    );
    assert_eq!(state.press(now + Duration::from_millis(10)), None);
    assert_eq!(
        state.release(now + Duration::from_millis(300)),
        Some(HotkeyAction::Stop {
            started_at: now,
            stopped_at: now + Duration::from_millis(300)
        })
    );
    assert_eq!(state.release(now + Duration::from_millis(310)), None);
}

#[test]
fn hotkey_actions_emit_logical_transitions_only() {
    let now = base_time();
    let (tx, rx) = crossbeam_channel::unbounded();

    send_hotkey_transition(HotkeyAction::Start { started_at: now }, &tx);
    send_hotkey_transition(
        HotkeyAction::Stop {
            started_at: now,
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
    assert!(!state.recording);
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
    assert!(!state.recording);
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
    assert_eq!(HotkeyBackend::Auto.label(), "auto");
    assert_eq!(HotkeyBackend::Desktop.label(), "desktop");
    #[cfg(target_os = "linux")]
    {
        assert_eq!(HotkeyBackend::X11GlobalHotkey.label(), "x11-global-hotkey");
        assert_eq!(HotkeyBackend::X11Listen.label(), "x11-listen");
        assert_eq!(
            HotkeyBackend::EvdevProxyExperimental.label(),
            "evdev-proxy-experimental"
        );
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
    assert_eq!(parse("evdev"), HotkeyBackend::EvdevProxyExperimental);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_backend_routing_helpers_classify_backends() {
    assert!(HotkeyBackend::Auto.uses_registered_x11());
    assert!(HotkeyBackend::Desktop.uses_registered_x11());
    assert!(HotkeyBackend::X11GlobalHotkey.uses_registered_x11());
    assert!(!HotkeyBackend::X11Listen.uses_registered_x11());
    assert!(!HotkeyBackend::EvdevProxyExperimental.uses_registered_x11());

    assert!(HotkeyBackend::X11Listen.uses_passive_x11_listen());
    assert!(!HotkeyBackend::X11GlobalHotkey.uses_passive_x11_listen());

    assert!(HotkeyBackend::EvdevProxyExperimental.uses_evdev_proxy());
    assert!(!HotkeyBackend::X11Listen.uses_evdev_proxy());
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
