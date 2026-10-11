//! Linux hotkey backend regression tests.

use super::*;

fn base_time() -> Instant {
    Instant::now()
}

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

fn physical(ctrl: bool, space: bool) -> PhysicalHotkeyState {
    PhysicalHotkeyState { ctrl, space }
}

#[test]
fn registered_hotkey_physical_poll_keeps_recording_while_chord_is_down() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();

    state.event(RegisteredHotKeyState::Pressed, physical(true, true), now);

    assert_eq!(state.physical_poll(physical(true, true), at(now, 50)), None);
    assert!(state.is_recording());
}

#[test]
fn registered_hotkey_release_is_ignored_while_physical_chord_is_still_down() {
    let now = base_time();
    let mut state = RegisteredHotkeyLatch::default();

    state.event(RegisteredHotKeyState::Pressed, physical(true, true), now);

    assert_eq!(
        state.event(
            RegisteredHotKeyState::Released,
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

    state.event(RegisteredHotKeyState::Pressed, physical(true, true), now);
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
        physical_state_from_keycodes(&keys, &[37], &[65]),
        physical(false, true),
        "the startup Control mapping is stale"
    );
    assert_eq!(
        physical_state_from_keycodes(&keys, &[97], &[65]),
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
