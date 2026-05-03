//! Local control socket for an already-running daemon.

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(unix)]
use std::thread;
use std::thread::JoinHandle;
#[cfg(unix)]
use std::time::Duration;

use super::{
    inject::{FocusSnapshot, Injector, PasteMode, PasteOutcome},
    logging::Logger,
    preflight,
    worker::{sanitize_for_paste, PastePlan},
};

#[cfg(unix)]
const IPC_CLIENT_TIMEOUT: Duration = Duration::from_millis(750);

/// Command sent by helper subcommands to the running daemon.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IpcCommand {
    /// Return current daemon state.
    Status,
    /// Stop the daemon process.
    Stop,
    /// Paste the most recent transcript remembered in memory.
    PasteLast,
    /// Run the insertion path with caller-supplied text, without microphone use.
    TestPaste { text: String },
}

/// Response sent by the daemon control socket.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IpcResponse {
    /// Command completed.
    Ok { message: String },
    /// Current daemon state.
    Status {
        phase: String,
        last_transcript_len: Option<usize>,
    },
    /// Command failed.
    Err { message: String },
}

/// Shared in-memory daemon state used by the worker and IPC server.
#[derive(Default)]
pub(crate) struct SharedState {
    inner: Mutex<StateSnapshot>,
    insertion: Mutex<()>,
}

#[derive(Default)]
struct StateSnapshot {
    phase: String,
    last_transcript: Option<String>,
}

impl SharedState {
    /// Create an idle state snapshot.
    ///
    /// # Returns
    ///
    /// Shared state with no remembered transcript.
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(StateSnapshot {
                phase: "starting".to_string(),
                last_transcript: None,
            }),
            insertion: Mutex::new(()),
        }
    }

    /// Update the visible daemon phase.
    pub(crate) fn set_phase(&self, phase: impl Into<String>) {
        self.inner.lock().phase = phase.into();
    }

    /// Remember the latest transcript in memory.
    pub(crate) fn set_last_transcript(&self, text: String) {
        self.inner.lock().last_transcript = Some(text);
    }

    fn status(&self) -> IpcResponse {
        let inner = self.inner.lock();
        IpcResponse::Status {
            phase: inner.phase.clone(),
            last_transcript_len: inner.last_transcript.as_ref().map(String::len),
        }
    }

    fn last_transcript(&self) -> Option<String> {
        self.inner.lock().last_transcript.clone()
    }

    /// Run an insertion transaction while excluding worker and IPC paste paths.
    ///
    /// Clipboard staging plus a synthetic paste chord must be serialized
    /// process-wide. Otherwise `paste-last` or `test-paste` can race the worker
    /// clipboard transaction and paste the wrong text.
    ///
    /// # Returns
    ///
    /// The closure result.
    pub(crate) fn with_insertion_lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.insertion.lock();
        f()
    }
}

/// Running control socket server.
pub(crate) struct IpcServer {
    path: PathBuf,
    _thread: JoinHandle<()>,
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the daemon control socket.
///
/// # Arguments
///
/// * `state` - Shared daemon status and last-transcript cache.
/// * `paste_mode` - Paste mode used by `paste-last` and `test-paste`.
/// * `log` - Logger for socket errors.
///
/// # Returns
///
/// A server handle that removes the socket path on drop.
///
/// # Errors
///
/// Returns an error when the control socket cannot be bound.
pub(crate) fn spawn_server(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    log: Arc<Logger>,
) -> Result<IpcServer> {
    spawn_server_impl(state, paste_mode, log)
}

/// Run one IPC client command and print a concise response.
///
/// # Arguments
///
/// * `command` - Command to send to the running daemon.
/// * `quiet` - Suppress stdout on success.
///
/// # Returns
///
/// `Ok(())` when the command completed successfully.
///
/// # Errors
///
/// Returns an error when no daemon is listening or the daemon reports failure.
pub(crate) fn run_client(command: IpcCommand, quiet: bool) -> Result<()> {
    let response = send_command(command)?;
    match response {
        IpcResponse::Ok { message } => {
            if !quiet {
                println!("{message}");
            }
            Ok(())
        }
        IpcResponse::Status {
            phase,
            last_transcript_len,
        } => {
            if !quiet {
                println!("parakit: {phase}");
                match last_transcript_len {
                    Some(len) => println!("last transcript: {len} bytes"),
                    None => println!("last transcript: none"),
                }
            }
            Ok(())
        }
        IpcResponse::Err { message } => bail!("{message}"),
    }
}

#[cfg(unix)]
fn spawn_server_impl(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    log: Arc<Logger>,
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
                        let _ = thread::Builder::new()
                            .name("parakit-ipc-client".into())
                            .spawn(move || handle_client(stream, &state, paste_mode, &log));
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

#[cfg(not(unix))]
fn spawn_server_impl(
    _state: Arc<SharedState>,
    _paste_mode: PasteMode,
    _log: Arc<Logger>,
) -> Result<IpcServer> {
    bail!("local daemon IPC is not implemented on this platform")
}

#[cfg(unix)]
fn handle_client(
    mut stream: std::os::unix::net::UnixStream,
    state: &Arc<SharedState>,
    paste_mode: PasteMode,
    log: &Logger,
) {
    let _ = stream.set_read_timeout(Some(IPC_CLIENT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IPC_CLIENT_TIMEOUT));
    let outcome = match read_command(&stream).and_then(|command| {
        handle_command(command, Arc::clone(state), paste_mode).map_err(|err| IpcResponse::Err {
            message: format!("{err:#}"),
        })
    }) {
        Ok(outcome) => outcome,
        Err(response) => CommandOutcome {
            response,
            stop_after_response: false,
        },
    };

    if let Err(err) = write_response(&mut stream, &outcome.response) {
        log.warn(format!("control socket response failed: {err:#}"));
    }

    if outcome.stop_after_response {
        let socket_path = preflight::control_socket_path().ok();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            if let Some(path) = socket_path {
                let _ = std::fs::remove_file(path);
            }
            std::process::exit(0);
        });
    }
}

#[cfg(unix)]
fn read_command(
    stream: &std::os::unix::net::UnixStream,
) -> std::result::Result<IpcCommand, IpcResponse> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| IpcResponse::Err {
            message: format!("read control command failed: {err}"),
        })?;
    serde_json::from_str(&line).map_err(|err| IpcResponse::Err {
        message: format!("invalid control command: {err}"),
    })
}

#[cfg(unix)]
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

struct CommandOutcome {
    response: IpcResponse,
    stop_after_response: bool,
}

fn handle_command(
    command: IpcCommand,
    state: Arc<SharedState>,
    paste_mode: PasteMode,
) -> Result<CommandOutcome> {
    match command {
        IpcCommand::Status => Ok(CommandOutcome {
            response: state.status(),
            stop_after_response: false,
        }),
        IpcCommand::Stop => Ok(CommandOutcome {
            response: IpcResponse::Ok {
                message: "stopping".to_string(),
            },
            stop_after_response: true,
        }),
        IpcCommand::PasteLast => {
            let text = state
                .last_transcript()
                .context("no transcript has been captured in this daemon session")?;
            let result = state.with_insertion_lock(|| paste_text(&text, paste_mode))?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: match result {
                        PasteOutcome::Pasted => "pasted last transcript",
                        PasteOutcome::CopiedOnly => "copied last transcript",
                    }
                    .to_string(),
                },
                stop_after_response: false,
            })
        }
        IpcCommand::TestPaste { text } => {
            let result = state.with_insertion_lock(|| paste_text(&text, paste_mode))?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: match result {
                        PasteOutcome::Pasted => "test paste sent",
                        PasteOutcome::CopiedOnly => "test text copied",
                    }
                    .to_string(),
                },
                stop_after_response: false,
            })
        }
    }
}

fn paste_text(text: &str, paste_mode: PasteMode) -> Result<PasteOutcome> {
    let mut injector = Injector::new().context("could not initialize insertion backend")?;
    let text = match sanitize_for_paste(text, paste_mode) {
        PastePlan::Paste(text) => text,
        PastePlan::CopyOnly { text, .. } => {
            if paste_mode == PasteMode::Direct {
                anyhow::bail!("direct insertion blocked by sanitizer");
            }
            injector.copy_text(&text)?;
            return Ok(PasteOutcome::CopiedOnly);
        }
        PastePlan::Skip { reason } => anyhow::bail!("paste text skipped by sanitizer: {reason}"),
    };

    let focus = FocusSnapshot::capture().ok();
    let target = super::target::TargetSnapshot::capture(focus.as_ref());

    match super::target::inspect_recording_target(Some(&target)) {
        super::target::TargetDecision::Allow => {}
        super::target::TargetDecision::CopyOnly(_) => {
            if paste_mode == PasteMode::Direct {
                anyhow::bail!("direct insertion blocked by target safety");
            }
            injector.copy_text(&text)?;
            return Ok(PasteOutcome::CopiedOnly);
        }
        super::target::TargetDecision::Block(reason) => {
            anyhow::bail!("target safety blocked paste: {reason}");
        }
    }

    injector
        .prepare_for_mode(paste_mode)
        .context("could not prepare insertion backend")?;
    let outcome = injector
        .paste_text_guarded(&text, paste_mode, || match focus.as_ref() {
            Some(snapshot) => {
                if !snapshot.matches_current()? {
                    return Ok(false);
                }
                match super::target::inspect_recording_target(Some(&target)) {
                    super::target::TargetDecision::Allow => Ok(true),
                    super::target::TargetDecision::CopyOnly(_) => Ok(false),
                    super::target::TargetDecision::Block(reason) => {
                        anyhow::bail!("target safety blocked paste: {reason}")
                    }
                }
            }
            None => Ok(false),
        })
        .context("could not send paste command")?;
    Ok(outcome)
}

#[cfg(unix)]
fn write_response(
    stream: &mut std::os::unix::net::UnixStream,
    response: &IpcResponse,
) -> Result<()> {
    serde_json::to_writer(&mut *stream, response).context("serialize control response")?;
    stream.write_all(b"\n").context("write control response")?;
    Ok(())
}

#[cfg(unix)]
fn send_command(command: IpcCommand) -> Result<IpcResponse> {
    use std::os::unix::net::UnixStream;

    let path = preflight::control_socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect daemon control socket {}", path.display()))?;
    stream
        .set_read_timeout(Some(IPC_CLIENT_TIMEOUT))
        .context("set daemon control socket read timeout")?;
    stream
        .set_write_timeout(Some(IPC_CLIENT_TIMEOUT))
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

#[cfg(not(unix))]
fn send_command(_command: IpcCommand) -> Result<IpcResponse> {
    bail!("local daemon IPC is not implemented on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_state_reports_phase_and_last_transcript_length() {
        let state = SharedState::new();
        state.set_phase("recording");
        state.set_last_transcript("hello".to_string());

        assert!(matches!(
            state.status(),
            IpcResponse::Status {
                phase,
                last_transcript_len: Some(5),
            } if phase == "recording"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn socket_dir_is_restricted_to_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = PathBuf::from("target/tmp/ipc-private-dir-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir should be created");
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

    #[cfg(unix)]
    #[test]
    fn partial_client_command_times_out() {
        use super::super::logging::{LogLevel, Logger};
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        let (mut client, server) = UnixStream::pair().expect("unix stream pair");
        client
            .write_all(b"{")
            .expect("partial command should write");

        let state = Arc::new(SharedState::new());
        let started = Instant::now();
        let handler = thread::spawn(move || {
            let log = Logger::new(LogLevel::Quiet);
            handle_client(server, &state, PasteMode::Terminal, &log);
        });
        handler.join().expect("handler should return after timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
