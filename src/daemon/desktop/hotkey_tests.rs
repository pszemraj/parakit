//! Hotkey backend regression tests.

use super::*;

fn base_time() -> Instant {
    Instant::now()
}

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn hotkey_loop_failure_returns_to_daemon_owner_instead_of_exiting() {
    assert!(report_hotkey_loop_failure(Ok(()), "test hotkey", || unreachable!()).is_ok());
    let mut help_shown = false;
    let failed =
        report_hotkey_loop_failure(Err(anyhow::anyhow!("grab lost")), "test hotkey", || {
            help_shown = true;
            "recovery help".to_string()
        });
    assert!(failed.is_err());
    assert!(help_shown);
}

#[test]
fn ctrl_space_starts_and_stops() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::ControlLeft, now), (None, false));
    assert_eq!(
        state.press(Key::Space, at(now, 10)),
        (
            Some(HotkeyAction::Start {
                started_at: at(now, 10)
            }),
            true
        )
    );
    assert_eq!(
        state.release(Key::Space, at(now, 300)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: at(now, 300)
            }),
            true
        )
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_right_control_space_does_not_start() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::ControlRight, now), (None, false));
    assert_eq!(state.press(Key::Space, at(now, 10)), (None, false));
    assert_eq!(state.release(Key::Space, at(now, 20)), (None, false));
    assert!(!state.is_recording());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_left_control_release_stops_even_if_right_control_is_held() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, at(now, 10));
    state.press(Key::ControlRight, at(now, 20));

    assert_eq!(
        state.release(Key::ControlLeft, at(now, 300)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: at(now, 300)
            }),
            false
        )
    );
    assert!(!state.is_recording());
    assert_eq!(state.release(Key::Space, at(now, 310)), (None, true));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_tap_disabled_resets_state_and_allows_next_ptt_cycle() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, at(now, 10));
    assert!(state.is_recording());

    assert_eq!(
        state.reset_after_tap_disabled(at(now, 50)),
        Some(HotkeyAction::Stop {
            stopped_at: at(now, 50)
        })
    );
    assert!(!state.is_recording());
    assert_eq!(state.release(Key::Space, at(now, 60)), (None, false));

    assert_eq!(state.press(Key::ControlLeft, at(now, 200)), (None, false));
    assert_eq!(
        state.press(Key::Space, at(now, 210)),
        (
            Some(HotkeyAction::Start {
                started_at: at(now, 210)
            }),
            true
        )
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_physical_left_control_release_stops_active_recording() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, at(now, 10));
    assert!(state.is_recording());
    assert_eq!(
        state.macos_sync_modifiers(
            ModifierState {
                ctrl_right: true,
                ..ModifierState::default()
            },
            at(now, 20)
        ),
        Some(HotkeyAction::Stop {
            stopped_at: at(now, 20)
        })
    );
    assert!(!state.is_recording());
    assert!(!state.modifiers.ctrl_left);
    assert!(state.modifiers.ctrl_right);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_stale_left_control_state_does_not_start_on_plain_space() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    assert!(state.modifiers.ctrl_left);

    assert_eq!(
        state.macos_sync_modifiers(ModifierState::default(), at(now, 10)),
        None
    );
    assert!(!state.modifiers.ctrl_left);
    assert_eq!(state.press(Key::Space, at(now, 20)), (None, false));
    assert!(!state.is_recording());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_physical_left_control_state_can_start_next_space_press() {
    let now = base_time();
    let mut state = HotkeyState::default();

    assert_eq!(
        state.macos_sync_modifiers(
            ModifierState {
                ctrl_left: true,
                ..ModifierState::default()
            },
            now
        ),
        None
    );
    assert!(state.modifiers.ctrl_left);
    assert_eq!(
        state.press(Key::Space, at(now, 10)),
        (
            Some(HotkeyAction::Start {
                started_at: at(now, 10)
            }),
            true
        )
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_modified_space_tap_does_not_start_or_suppress() {
    let now = base_time();
    let mut state = HotkeyState::default();
    let modifiers = ModifierState {
        ctrl_left: true,
        shift_left: true,
        ..ModifierState::default()
    };

    assert_eq!(
        super::macos::handle_tap_event(
            &mut state,
            super::macos::K_CG_EVENT_KEY_DOWN,
            super::macos::MACOS_KEY_SPACE,
            modifiers,
            now
        ),
        (None, false)
    );
    assert!(!state.is_recording());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_flags_changed_tracks_right_control_as_extra_modifier() {
    let now = base_time();
    let mut state = HotkeyState::default();

    assert_eq!(
        super::macos::handle_tap_event(
            &mut state,
            super::macos::K_CG_EVENT_FLAGS_CHANGED,
            0,
            ModifierState {
                ctrl_left: true,
                ctrl_right: true,
                ..ModifierState::default()
            },
            now
        ),
        (None, false)
    );
    assert!(state.modifiers.ctrl_left);
    assert!(state.modifiers.ctrl_right);
    assert_eq!(
        super::macos::handle_tap_event(
            &mut state,
            super::macos::K_CG_EVENT_KEY_DOWN,
            super::macos::MACOS_KEY_SPACE,
            ModifierState {
                ctrl_left: true,
                ctrl_right: true,
                ..ModifierState::default()
            },
            at(now, 10)
        ),
        (None, false)
    );
    assert!(!state.is_recording());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_right_control_space_starts_and_stops() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::ControlRight, now), (None, false));
    assert_eq!(
        state.press(Key::Space, at(now, 10)),
        (
            Some(HotkeyAction::Start {
                started_at: at(now, 10)
            }),
            true
        )
    );
    assert_eq!(
        state.release(Key::ControlRight, at(now, 300)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: at(now, 300)
            }),
            false
        )
    );
}

#[test]
fn ctrl_repress_while_space_held_does_not_restart_recording() {
    let now = base_time();
    let mut state = HotkeyState::default();

    state.press(Key::ControlLeft, now);
    state.press(Key::Space, at(now, 10));

    assert_eq!(
        state.release(Key::ControlLeft, at(now, 50)),
        (
            Some(HotkeyAction::Stop {
                stopped_at: at(now, 50),
            }),
            false,
        )
    );

    assert_eq!(state.press(Key::ControlLeft, at(now, 60)), (None, false));
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
        state.press(Key::Space, at(now, 10)),
        (
            Some(HotkeyAction::Start {
                started_at: at(now, 10)
            }),
            true
        )
    );
    assert_eq!(state.press(Key::Space, at(now, 20)), (None, true));
    assert!(state.is_recording());
}

#[test]
fn standalone_space_auto_repeat_passes_through() {
    let now = base_time();
    let mut state = HotkeyState::default();

    assert_eq!(state.press(Key::Space, now), (None, false));
    assert_eq!(state.press(Key::Space, at(now, 20)), (None, false));
    assert_eq!(state.release(Key::Space, at(now, 40)), (None, false));
    assert!(!state.is_recording());
}

#[test]
fn space_held_before_ctrl_does_not_start_or_suppress_repeat() {
    let now = base_time();
    let mut state = HotkeyState::default();

    assert_eq!(state.press(Key::Space, now), (None, false));
    assert_eq!(state.press(Key::ControlLeft, at(now, 10)), (None, false));
    assert_eq!(state.press(Key::Space, at(now, 20)), (None, false));
    assert!(!state.is_recording());
}

#[test]
fn registered_hotkey_press_release_starts_and_stops_once() {
    let now = base_time();
    let mut state = RecordingLatch::default();
    assert_eq!(
        state.start(now),
        Some(HotkeyAction::Start { started_at: now })
    );
    assert_eq!(state.start(at(now, 10)), None);
    assert_eq!(
        state.stop(at(now, 300)),
        Some(HotkeyAction::Stop {
            stopped_at: at(now, 300)
        })
    );
    assert_eq!(state.stop(at(now, 310)), None);
}

#[test]
fn hotkey_actions_emit_logical_transitions_only() {
    let now = base_time();
    let (tx, rx) = crossbeam_channel::unbounded();

    send_hotkey_transition(HotkeyAction::Start { started_at: now }, &tx);
    send_hotkey_transition(
        HotkeyAction::Stop {
            stopped_at: at(now, 250),
        },
        &tx,
    );

    assert_eq!(rx.recv().unwrap(), HotkeyTransition::Pressed { at: now });
    assert_eq!(
        rx.recv().unwrap(),
        HotkeyTransition::Released { at: at(now, 250) }
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn rapid_double_press_is_ignored_and_suppressed() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::Space, at(now, 10));
    state.release(Key::Space, at(now, 20));
    assert_eq!(state.press(Key::Space, at(now, 80)), (None, true));
    assert_eq!(state.release(Key::Space, at(now, 90)), (None, true));
    assert!(!state.is_recording());
}

#[test]
fn ctrl_shift_space_does_not_start_or_suppress() {
    let now = base_time();
    let mut state = HotkeyState::default();
    state.press(Key::ControlLeft, now);
    state.press(Key::ShiftLeft, at(now, 5));
    assert_eq!(state.press(Key::Space, at(now, 10)), (None, false));
    assert!(!state.is_recording());
}

#[test]
fn unrelated_keys_pass_through() {
    let now = base_time();
    let mut state = HotkeyState::default();
    assert_eq!(state.press(Key::KeyA, now), (None, false));
    assert_eq!(state.release(Key::KeyA, at(now, 10)), (None, false));
}

#[test]
fn backend_labels_are_stable() {
    let expected = [
        (HotkeyBackend::Auto, "auto"),
        (HotkeyBackend::Desktop, "desktop"),
        #[cfg(target_os = "linux")]
        (HotkeyBackend::Ibus, "ibus"),
        #[cfg(target_os = "linux")]
        (HotkeyBackend::X11GlobalHotkey, "x11-global-hotkey"),
        #[cfg(target_os = "linux")]
        (HotkeyBackend::X11Listen, "x11-listen"),
        #[cfg(target_os = "linux")]
        (
            HotkeyBackend::EvdevProxyExperimental,
            "evdev-proxy-experimental",
        ),
    ];
    let actual = <HotkeyBackend as clap::ValueEnum>::value_variants()
        .iter()
        .map(|&backend| (backend, backend.label()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_backend_names_parse_to_stable_variants() {
    fn parse(value: &str) -> HotkeyBackend {
        <HotkeyBackend as clap::ValueEnum>::from_str(value, false).unwrap()
    }

    assert_eq!(parse("ibus"), HotkeyBackend::Ibus);
    assert_eq!(
        serde_json::from_str::<HotkeyBackend>(r#""ibus""#).unwrap(),
        HotkeyBackend::Ibus
    );
    assert_eq!(parse("x11-global-hotkey"), HotkeyBackend::X11GlobalHotkey);
    assert_eq!(parse("x11-listen"), HotkeyBackend::X11Listen);
    assert_eq!(
        parse("evdev-proxy-experimental"),
        HotkeyBackend::EvdevProxyExperimental
    );
    assert!(<HotkeyBackend as clap::ValueEnum>::from_str("evdev-proxy", false).is_err());
    assert!(serde_json::from_str::<HotkeyBackend>(r#""evdev-proxy""#).is_err());
    assert!(<HotkeyBackend as clap::ValueEnum>::from_str("evdev", false).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn x11_keymap_bit_probe_detects_down_keycodes() {
    let mut keys = [0_u8; 32];
    keys[4] = 0b0010_0000;

    assert!(super::super::x11::keycode_down(&keys, 37));
    assert!(!super::super::x11::keycode_down(&keys, 36));
    assert!(!super::super::x11::keycode_down(&keys, 255));
}

#[test]
fn x11_hotkey_mapping_refresh_retries_failure_and_reports_once_per_mapping_event() {
    let mut mapping = X11HotkeyMapping {
        space: vec![65],
        control: vec![37, 105],
        needs_refresh: false,
    };

    assert!(mapping
        .refresh_with(true, || anyhow::bail!("Control keycodes unavailable"))
        .is_some());
    assert_eq!(mapping.space, [65]);
    assert_eq!(mapping.control, [37, 105]);
    assert!(mapping.needs_refresh);

    // A poll without a new MappingNotify retries but does not report again.
    let mut retried = false;
    assert!(mapping
        .refresh_with(false, || {
            retried = true;
            anyhow::bail!("Control keycodes still unavailable")
        })
        .is_none());
    assert!(retried);
    assert_eq!(mapping.space, [65]);
    assert_eq!(mapping.control, [37, 105]);
    assert!(mapping.needs_refresh);

    assert!(mapping
        .refresh_with(true, || anyhow::bail!("Control keycodes unavailable again"))
        .is_some());
    assert!(mapping.needs_refresh);

    assert!(mapping
        .refresh_with(false, || {
            Ok(X11HotkeyMapping {
                space: vec![66],
                control: vec![38, 106],
                needs_refresh: false,
            })
        })
        .is_none());
    assert_eq!(mapping.space, [66]);
    assert_eq!(mapping.control, [38, 106]);
    assert!(!mapping.needs_refresh);

    assert!(mapping
        .refresh_with(false, || {
            panic!("successful refresh must clear the pending retry")
        })
        .is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_backend_aliases_resolve_to_one_route() {
    for (backend, expected) in [
        (HotkeyBackend::Auto, LinuxHotkeyRoute::RegisteredX11),
        (HotkeyBackend::Desktop, LinuxHotkeyRoute::RegisteredX11),
        (
            HotkeyBackend::X11GlobalHotkey,
            LinuxHotkeyRoute::RegisteredX11,
        ),
        (HotkeyBackend::X11Listen, LinuxHotkeyRoute::PassiveX11),
        (
            HotkeyBackend::EvdevProxyExperimental,
            LinuxHotkeyRoute::EvdevProxy,
        ),
    ] {
        assert_eq!(backend.linux_route_for_session(false), expected);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn wayland_desktop_routes_use_focused_ibus_and_preserve_explicit_choices() {
    for backend in [
        HotkeyBackend::Auto,
        HotkeyBackend::Desktop,
        HotkeyBackend::Ibus,
    ] {
        assert_eq!(
            backend.linux_route_for_session(true),
            LinuxHotkeyRoute::Ibus
        );
    }
    assert_eq!(
        HotkeyBackend::Ibus.linux_route_for_session(false),
        LinuxHotkeyRoute::Ibus
    );
    assert_eq!(
        HotkeyBackend::X11GlobalHotkey.linux_route_for_session(true),
        LinuxHotkeyRoute::RegisteredX11
    );
    assert_eq!(
        HotkeyBackend::X11Listen.linux_route_for_session(true),
        LinuxHotkeyRoute::PassiveX11
    );
}
