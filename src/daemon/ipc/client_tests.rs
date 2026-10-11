//! Daemon-absent disposition, stop-wait, and completion-message regressions for the control client.

use super::*;

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn daemon_not_running_disposition_covers_every_control_command() {
    let cases = [
        (
            IpcCommand::Status,
            DaemonNotRunningDisposition::Success("parakit: not running"),
        ),
        (
            IpcCommand::Stop,
            DaemonNotRunningDisposition::Success("parakit: not running; nothing to stop"),
        ),
        (
            IpcCommand::CopyLast { index: 0 },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
        (
            IpcCommand::History { limit: None },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
        (
            IpcCommand::TestPaste {
                text: "test".to_string(),
            },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
    ];

    for (command, expected) in cases {
        assert_eq!(daemon_not_running_disposition(&command, false), expected);
    }
    let held = DaemonNotRunningDisposition::Error(
        "daemon control endpoint is unavailable while the singleton lock is held; it may still be starting or stopping",
    );
    assert_eq!(
        daemon_not_running_disposition(&IpcCommand::Status, true),
        held
    );
    assert_eq!(
        daemon_not_running_disposition(&IpcCommand::Stop, true),
        DaemonNotRunningDisposition::WaitForStop
    );
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_waits_for_a_held_lock_and_reports_a_persistent_holder() {
    let mut probes = 0;
    wait_for_daemon_stop_with_probe(Instant::now() + Duration::from_secs(1), || {
        probes += 1;
        Ok(probes < 3)
    })
    .expect("stop should wait until the lock is released");
    assert_eq!(probes, 3);

    let error = wait_for_daemon_stop_with_probe(Instant::now(), || Ok(true))
        .expect_err("a persistent lock holder should time out");
    assert!(error.to_string().contains("if this persists"));
    #[cfg(not(target_os = "windows"))]
    assert!(error.to_string().contains("pgrep -af parakit"));
    #[cfg(target_os = "windows")]
    assert!(error.to_string().contains("Get-Process parakit"));
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_success_message_describes_the_completed_state() {
    assert_eq!(
        completed_command_message(&IpcCommand::Stop, "stopping"),
        "stopped"
    );
    assert_eq!(
        completed_command_message(&IpcCommand::CopyLast { index: 0 }, "copied"),
        "copied"
    );
}
