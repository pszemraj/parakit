//! Unix control-socket directory, timeout, and message-size regressions.

use super::*;

#[test]
fn socket_dir_is_restricted_to_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = crate::test_support::fixture_root("ipc-private-dir-test", "owner");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
        .expect("test dir permissions should be set");

    ensure_private_socket_dir(&dir).expect("socket dir should be restricted");

    let mode = std::fs::metadata(&dir)
        .expect("test dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);

    std::fs::remove_dir_all(&dir).expect("test dir should be removed");
}

#[test]
fn partial_client_command_times_out() {
    use crate::daemon::logging::{LogLevel, Logger};
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;

    let (mut client, server) = UnixStream::pair().expect("unix stream pair");
    client
        .write_all(b"{")
        .expect("partial command should write");

    let state = Arc::new(SharedState::new());
    let started = Instant::now();
    let handler = thread::spawn(move || {
        let log = Arc::new(Logger::new(LogLevel::Quiet));
        let notifier = Notifier::silent(Arc::clone(&log));
        handle_client(server, &state, PasteMode::Terminal, false, log, notifier);
    });
    handler.join().expect("handler should return after timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn unix_command_larger_than_size_limit_is_rejected() {
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;

    let (mut client, server) = UnixStream::pair().expect("unix stream pair");
    let mut payload = vec![b'x'; IPC_MAX_MESSAGE_SIZE + 1];
    payload.push(b'\n');
    // macOS can fill the socket buffer before a 64 KiB request is written.
    // Run the peer concurrently, as the real IPC transport does.
    let writer = thread::spawn(move || client.write_all(&payload));

    let err = read_command(&server).expect_err("oversized Unix command should be rejected");
    drop(server);
    // Rejection may close the reader before the trailing newline is written.
    let _ = writer.join().expect("writer should finish");

    assert_eq!(
        err.to_string(),
        "Unix daemon control message exceeds 64 KiB"
    );
}
