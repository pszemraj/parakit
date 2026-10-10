//! Unix-domain-socket transport for the daemon control protocol.

use super::*;
use std::io::{BufRead, BufReader, Read as _, Write};

/// Start the Unix-domain-socket daemon control server.
///
/// # Arguments
///
/// * `state` - Shared daemon status and transcript history.
/// * `paste_mode` - Paste mode used by paste-related commands.
/// * `keep_transcript_clipboard` - Whether command insertion leaves text on
///   the clipboard.
/// * `log` - Logger for socket errors.
/// * `notifier` - Daemon notifier shared with IPC insertion commands.
///
/// # Returns
///
/// A server handle that removes the socket path on drop.
///
/// # Errors
///
/// Returns an error when the socket directory cannot be secured or the control
/// socket cannot be bound.
pub(super) fn spawn_server_impl(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: Arc<Logger>,
    notifier: Notifier,
) -> Result<IpcServer> {
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let path = preflight::control_socket_path()?;
    if let Some(parent) = path.parent() {
        ensure_private_socket_dir(parent)?;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("remove stale {}", path.display())),
    }

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind daemon control socket {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restrict daemon control socket {}", path.display()))?;
    let thread = thread::Builder::new()
        .name("parakit-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let state = Arc::clone(&state);
                        let log = Arc::clone(&log);
                        let notifier = notifier.clone();
                        let _ = thread::Builder::new()
                            .name("parakit-ipc-client".into())
                            .spawn(move || {
                                handle_client(
                                    stream,
                                    &state,
                                    paste_mode,
                                    keep_transcript_clipboard,
                                    log,
                                    notifier,
                                )
                            });
                    }
                    Err(err) => log.warn(format!("control socket failed: {err}")),
                }
            }
        })
        .context("spawn daemon control socket")?;

    Ok(IpcServer {
        path,
        _thread: thread,
    })
}

fn handle_client(
    mut stream: std::os::unix::net::UnixStream,
    state: &Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: Arc<Logger>,
    notifier: Notifier,
) {
    let _ = stream.set_read_timeout(Some(IPC_TRANSPORT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IPC_TRANSPORT_TIMEOUT));
    let outcome = client_command_outcome(
        read_command(&stream),
        state,
        paste_mode,
        keep_transcript_clipboard,
        log.as_ref(),
        &notifier,
    );

    if let Err(err) = write_response(&mut stream, &outcome.response) {
        log.warn(format!("control socket response failed: {err:#}"));
    }

    if outcome.stop_after_response {
        finish_stop_after_response(
            state,
            preflight::control_socket_path().ok(),
            terminate_daemon,
        );
    }
}

fn read_command(stream: &std::os::unix::net::UnixStream) -> Result<IpcCommand> {
    let reader = BufReader::new(stream);
    let mut line = Vec::new();
    reader
        .take(IPC_MAX_MESSAGE_SIZE as u64 + 1)
        .read_until(b'\n', &mut line)
        .context("read control command failed")?;
    let payload_len = if line.ends_with(b"\n") {
        line.len() - 1
    } else {
        line.len()
    };
    if payload_len > IPC_MAX_MESSAGE_SIZE {
        bail!("Unix daemon control message exceeds 64 KiB");
    }
    let line = std::str::from_utf8(&line).context("control command is not valid UTF-8")?;
    parse_command(line)
}

fn ensure_private_socket_dir(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    std::fs::create_dir_all(path)
        .with_context(|| format!("create daemon socket dir {}", path.display()))?;
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect daemon socket dir {}", path.display()))?;
    if !meta.file_type().is_dir() {
        bail!("daemon socket path is not a directory: {}", path.display());
    }
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        bail!(
            "daemon socket dir {} is not owned by the current user",
            path.display()
        );
    }

    if meta.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restrict daemon socket dir {}", path.display()))?;
    }
    let mode = std::fs::symlink_metadata(path)
        .with_context(|| format!("reinspect daemon socket dir {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o700 {
        bail!(
            "daemon socket dir {} must have mode 0700, got {mode:o}",
            path.display()
        );
    }
    Ok(())
}

fn write_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &IpcResponse,
) -> Result<()> {
    serde_json::to_writer(&mut *stream, response).context("serialize control response")?;
    stream.write_all(b"\n").context("write control response")?;
    Ok(())
}

/// Send one command to the running Unix daemon.
///
/// # Returns
///
/// The daemon response decoded from the socket reply.
///
/// # Errors
///
/// Returns an error when the control socket is unavailable, transport I/O
/// fails, or the response cannot be decoded.
pub(super) fn send_command(command: &IpcCommand) -> Result<IpcResponse> {
    use std::os::unix::net::UnixStream;

    let response_timeout = command.response_timeout();
    let path = preflight::control_socket_path()?;
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(DaemonNotRunning.into());
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("connect daemon control socket {}", path.display()));
        }
    };
    stream
        .set_read_timeout(Some(response_timeout))
        .context("set daemon control socket read timeout")?;
    stream
        .set_write_timeout(Some(IPC_TRANSPORT_TIMEOUT))
        .context("set daemon control socket write timeout")?;
    serde_json::to_writer(&mut stream, &command).context("serialize control command")?;
    stream
        .write_all(b"\n")
        .context("write daemon control command")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("read daemon control response")?;
    serde_json::from_str(&line).context("parse daemon control response")
}

#[cfg(test)]
#[path = "unix_socket_tests.rs"]
mod tests;
