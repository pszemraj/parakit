//! Windows named-pipe security, timeout, and message-framing regressions.

use super::*;

#[test]
fn current_user_only_sddl_excludes_built_in_admins() {
    let sddl = current_user_only_pipe_sddl("S-1-5-21-1000");

    assert_eq!(sddl, "D:P(A;;GA;;;SY)(A;;GA;;;S-1-5-21-1000)");
    assert!(!sddl.contains(";;;BA"));
}

#[test]
fn server_pipe_mode_rejects_remote_clients() {
    assert_ne!(server_pipe_mode() & PIPE_REJECT_REMOTE_CLIENTS, 0);
}

#[test]
fn remaining_timeout_counts_down_to_none() {
    let started = std::time::Instant::now();

    assert_eq!(
        remaining_timeout_ms(
            started,
            started + Duration::from_millis(250),
            IPC_CLIENT_TIMEOUT_MS,
        ),
        Some(500)
    );
    assert_eq!(
        remaining_timeout_ms(
            started,
            started + Duration::from_millis(749),
            IPC_CLIENT_TIMEOUT_MS,
        ),
        Some(1)
    );
    assert_eq!(
        remaining_timeout_ms(
            started,
            started + Duration::from_millis(750),
            IPC_CLIENT_TIMEOUT_MS,
        ),
        None
    );
}

#[test]
fn retry_sleep_is_capped_by_remaining_timeout() {
    let started = std::time::Instant::now();

    assert_eq!(
        retry_sleep_duration(
            started,
            started + Duration::from_millis(100),
            IPC_CLIENT_TIMEOUT_MS,
            Duration::from_millis(10),
        ),
        Some(Duration::from_millis(10))
    );
    assert_eq!(
        retry_sleep_duration(
            started,
            started + Duration::from_millis(745),
            IPC_CLIENT_TIMEOUT_MS,
            Duration::from_millis(10),
        ),
        Some(Duration::from_millis(5))
    );
}

#[test]
fn retryable_availability_errors_are_limited_to_normal_pipe_races() {
    assert!(is_retryable_pipe_availability_error(ERROR_FILE_NOT_FOUND));
    assert!(is_retryable_pipe_availability_error(ERROR_PIPE_BUSY));
    assert!(!is_retryable_pipe_availability_error(ERROR_SEM_TIMEOUT));
}

#[test]
fn missing_named_pipe_is_classified_as_daemon_not_running() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let pipe_name = encode_wide_null(&format!(
        r"\\.\pipe\parakit-ipc-missing-test-{}-{unique}",
        std::process::id()
    ));

    let Err(err) = connect_client_pipe(&pipe_name) else {
        panic!("a nonexistent named pipe should not connect");
    };

    assert!(err.is::<DaemonNotRunning>(), "unexpected error: {err:#}");
    assert_eq!(err.to_string(), "daemon is not running");
}

#[test]
fn client_message_read_mode_preserves_message_boundaries() -> Result<()> {
    let (server, client) = connected_test_pipe(false)?;

    let payload = b"abcdef";
    write_pipe_message(&server, payload)?;

    let mut first = [0_u8; 3];
    let mut first_read = 0_u32;
    let ok = unsafe {
        ReadFile(
            client.0,
            first.as_mut_ptr().cast(),
            first.len() as u32,
            &mut first_read,
            null_mut(),
        )
    };
    assert_eq!(ok, 0);
    assert_eq!(unsafe { GetLastError() }, ERROR_MORE_DATA);
    assert_eq!(first_read as usize, first.len());

    let mut second = [0_u8; 3];
    let mut second_read = 0_u32;
    let ok = unsafe {
        ReadFile(
            client.0,
            second.as_mut_ptr().cast(),
            second.len() as u32,
            &mut second_read,
            null_mut(),
        )
    };
    assert_ne!(ok, 0);
    assert_eq!(second_read as usize, second.len());

    assert_eq!([first, second].concat(), payload);

    Ok(())
}

#[test]
fn pipe_message_at_size_limit_round_trips() -> Result<()> {
    let (server, client) = connected_test_pipe(true)?;
    let payload: Vec<u8> = (0..IPC_MAX_MESSAGE_SIZE)
        .map(|index| (index % 251) as u8)
        .collect();
    let expected = payload.clone();
    let writer = std::thread::spawn(move || write_pipe_message(&server, &payload));

    let received = read_pipe_message(&client)?;
    writer
        .join()
        .expect("Windows pipe writer thread should not panic")?;

    assert_eq!(received, expected);
    Ok(())
}

#[test]
fn pipe_message_larger_than_size_limit_is_rejected() -> Result<()> {
    let (server, client) = connected_test_pipe(true)?;
    let payload = vec![b'x'; IPC_MAX_MESSAGE_SIZE + 1];
    let writer = std::thread::spawn(move || write_pipe_message(&server, &payload));

    let err = read_pipe_message(&client)
        .expect_err("oversized Windows daemon pipe message should be rejected");
    drop(client);
    let _ = writer
        .join()
        .expect("Windows pipe writer thread should not panic");

    assert_eq!(
        err.to_string(),
        "Windows daemon control message exceeds 64 KiB"
    );
    Ok(())
}

#[test]
fn pipe_message_read_times_out_when_peer_sends_nothing() -> Result<()> {
    let (server, _client) = connected_test_pipe(true)?;
    let started = std::time::Instant::now();

    let err =
        read_pipe_message(&server).expect_err("idle Windows daemon pipe read should time out");

    assert!(started.elapsed() < Duration::from_secs(2));
    let message = format!("{err:#}");
    // The overlapped read receives the total deadline's remaining
    // budget, so setup time can make this 749ms (or slightly less)
    // rather than the configured 750ms.
    assert!(
        message.starts_with("ReadFile Windows daemon control pipe failed: timed out after ")
            && message.ends_with("ms"),
        "unexpected idle-read failure: {message}"
    );
    Ok(())
}

#[test]
fn pipe_response_remains_readable_after_server_handle_closes() -> Result<()> {
    let (server, client) = connected_test_pipe(true)?;

    write_pipe_message(&server, b"ok")?;
    drop(server);

    assert_eq!(read_pipe_message(&client)?, b"ok");
    Ok(())
}

fn connected_test_pipe(overlapped: bool) -> Result<(PipeHandle, PipeHandle)> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let pipe_name = encode_wide_null(&format!(
        r"\\.\pipe\parakit-ipc-test-{}-{unique}",
        std::process::id()
    ));
    let overlapped_flag = if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
    let server_handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(pipe_name.as_ptr()),
            PIPE_ACCESS_DUPLEX | overlapped_flag,
            server_pipe_mode(),
            1,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            IPC_CLIENT_TIMEOUT_MS,
            null_mut(),
        )
    };
    assert!(!is_invalid_handle(server_handle));
    let server = PipeHandle(server_handle);

    let client_handle = unsafe {
        CreateFileW(
            PCWSTR(pipe_name.as_ptr()),
            GENERIC_READ | GENERIC_WRITE,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | overlapped_flag,
            HANDLE::default(),
        )
    };
    assert!(!is_invalid_handle(client_handle));
    let client = PipeHandle(client_handle);

    set_client_message_read_mode(&client)?;
    connect_server_pipe(&server)?;
    Ok((server, client))
}
