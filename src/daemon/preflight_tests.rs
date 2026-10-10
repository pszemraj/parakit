//! Doctor readiness, hotkey backend selection, and singleton lock regressions.

use super::super::audio::MicInfo;
use super::*;

#[test]
#[cfg(target_os = "linux")]
fn selected_hotkey_backend_controls_readiness() {
    let cases = [
        (LinuxHotkeyRoute::RegisteredX11, true, false, false),
        (LinuxHotkeyRoute::RegisteredX11, false, true, true),
        (LinuxHotkeyRoute::PassiveX11, true, false, false),
        (LinuxHotkeyRoute::PassiveX11, false, true, true),
        (LinuxHotkeyRoute::EvdevProxy, false, true, false),
        (LinuxHotkeyRoute::EvdevProxy, true, false, true),
    ];
    for (route, x11_ready, evdev_ready, expected) in cases {
        assert_eq!(
            linux_hotkey_startup_blocked(route, x11_ready, evdev_ready),
            expected,
            "route={route:?} x11_ready={x11_ready} evdev_ready={evdev_ready}"
        );
    }
}

#[cfg(target_os = "linux")]
fn evdev_report_with_hotkey_keyboards(hotkey_keyboards: usize) -> EvdevReport {
    EvdevReport {
        event_devices: 4,
        readable: 1,
        hotkey_keyboards,
        denied: 3,
        uinput_writable: true,
        uinput_error: None,
        other_errors: Vec::new(),
    }
}

#[test]
#[cfg(target_os = "linux")]
fn evdev_readiness_requires_a_candidate_but_allows_denied_non_candidates() {
    for (name, hotkey_keyboards, ready, status) in [
        ("denied non-candidates are allowed", 1, true, "ready"),
        (
            "hotkey candidate is required",
            0,
            false,
            "no keyboard candidates",
        ),
    ] {
        let report = evdev_report_with_hotkey_keyboards(hotkey_keyboards);

        assert_eq!(report.grab_likely_available(), ready, "{name}");
        assert_eq!(report.status_label(), status, "{name}");
    }
}

#[cfg(target_os = "macos")]
fn macos_permissions(
    accessibility: super::super::macos::PermissionStatus,
    input_monitoring: super::super::macos::PermissionStatus,
) -> super::super::macos::PermissionReport {
    super::super::macos::PermissionReport {
        accessibility,
        microphone: super::super::macos::PermissionStatus::Granted,
        input_monitoring,
    }
}

#[test]
#[cfg(target_os = "macos")]
fn macos_hotkey_readiness_requires_accessibility_and_input_monitoring() {
    use super::super::macos::PermissionStatus;

    let ready = macos_permissions(PermissionStatus::Granted, PermissionStatus::Granted);
    assert!(!macos_hotkey_startup_blocked(&ready));
    assert_eq!(
        macos_hotkey_status(&ready),
        "macOS Accessibility and Input Monitoring ready for Left Control+Space"
    );

    let missing_input = macos_permissions(PermissionStatus::Granted, PermissionStatus::Denied);
    assert!(macos_hotkey_startup_blocked(&missing_input));
    assert_eq!(
        macos_hotkey_status(&missing_input),
        "macOS Input Monitoring permission missing"
    );

    let missing_both = macos_permissions(PermissionStatus::Denied, PermissionStatus::NotDetermined);
    assert!(macos_hotkey_startup_blocked(&missing_both));
    assert_eq!(
        macos_hotkey_status(&missing_both),
        "macOS Accessibility and Input Monitoring permissions missing"
    );
}

#[test]
fn doctor_ready_requires_free_daemon_lock() {
    let report = HotkeyReport {
        blocking: false,
        status: "ready".to_string(),
        summary: String::new(),
        details: String::new(),
    };
    let mic = Ok(MicInfo {
        name: "Test Mic".to_string(),
        input_rate: 16_000,
        channels: 1,
        sample_format: "F32".to_string(),
        source_id: None,
        resampling: false,
        config_note: None,
    });
    let insertion = Ok(());

    let free_lock = Ok(());
    assert!(doctor_ready(&report, &free_lock, &mic, &insertion));

    let held_lock: Result<()> = Err(anyhow::anyhow!("already running"));
    assert!(!doctor_ready(&report, &held_lock, &mic, &insertion));
}

#[test]
fn singleton_lock_blocks_second_holder() {
    let path =
        crate::test_support::fixture_root("parakit-lock-test", "singleton").join("parakit.lock");

    let first =
        acquire_singleton_lock_at(&path, SINGLETON_START_WAIT).expect("first lock should succeed");
    assert!(singleton_lock_held_at(&path).expect("held lock should be observable"));
    let Err(second) = acquire_singleton_lock_at(&path, SINGLETON_START_WAIT) else {
        panic!("a held daemon lock should reject another owner");
    };
    assert!(second.is::<DaemonAlreadyRunning>(), "{second:#}");
    assert_eq!(second.to_string(), "daemon is already running");
    drop(first);
    assert!(!singleton_lock_held_at(&path).expect("released lock should be observable"));
    let third = acquire_singleton_lock_at(&path, SINGLETON_START_WAIT)
        .expect("lock should release after drop");
    drop(third);
}

#[test]
fn singleton_lock_probe_does_not_create_missing_state() {
    let path = crate::test_support::fixture_root("parakit-lock-test", "missing-probe")
        .join("missing")
        .join("parakit.lock");
    assert!(!path.exists());
    assert!(!singleton_lock_held_at(&path).expect("missing lock should be free"));
    assert!(!path.exists());
}

#[test]
fn singleton_start_waits_for_a_transient_shared_probe() {
    let path =
        crate::test_support::fixture_root("parakit-lock-test", "shared-probe").join("parakit.lock");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let probe = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    FileExt::lock_shared(&probe).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        drop(probe);
    });

    let daemon = acquire_singleton_lock_at(&path, Duration::from_secs(5))
        .expect("startup should wait for the short shared probe");
    release.join().unwrap();
    assert!(singleton_lock_held_at(&path).unwrap());
    drop(daemon);
    assert!(!singleton_lock_held_at(&path).unwrap());
}
