//! Local control socket for an already-running daemon.

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{
    inject::{FocusSnapshot, Injector, PasteMode, PasteOutcome},
    logging::Logger,
    preflight,
    worker::{sanitize_for_paste, PastePlan},
};

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
    use std::os::unix::net::UnixListener;

    let path = preflight::control_socket_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create daemon socket dir {}", parent.display()))?;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("remove stale {}", path.display())),
    }

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind daemon control socket {}", path.display()))?;
    let thread = thread::Builder::new()
        .name("parakit-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => handle_client(stream, &state, paste_mode, &log),
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
    let response = match read_command(&stream).and_then(|command| {
        handle_command(command, Arc::clone(state), paste_mode).map_err(|err| IpcResponse::Err {
            message: format!("{err:#}"),
        })
    }) {
        Ok(response) => response,
        Err(response) => response,
    };

    if let Err(err) = write_response(&mut stream, &response) {
        log.warn(format!("control socket response failed: {err:#}"));
    }

    if matches!(response, IpcResponse::Ok { ref message } if message == "stopping") {
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

fn handle_command(
    command: IpcCommand,
    state: Arc<SharedState>,
    paste_mode: PasteMode,
) -> Result<IpcResponse> {
    match command {
        IpcCommand::Status => Ok(state.status()),
        IpcCommand::Stop => Ok(IpcResponse::Ok {
            message: "stopping".to_string(),
        }),
        IpcCommand::PasteLast => {
            let text = state
                .last_transcript()
                .context("no transcript has been captured in this daemon session")?;
            let result = paste_text(&text, paste_mode)?;
            Ok(IpcResponse::Ok {
                message: match result {
                    IpcPasteResult::Pasted => "pasted last transcript",
                    IpcPasteResult::CopiedOnly => "copied last transcript",
                }
                .to_string(),
            })
        }
        IpcCommand::TestPaste { text } => {
            let result = paste_text(&text, paste_mode)?;
            Ok(IpcResponse::Ok {
                message: match result {
                    IpcPasteResult::Pasted => "test paste sent",
                    IpcPasteResult::CopiedOnly => "test text copied",
                }
                .to_string(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IpcPasteResult {
    Pasted,
    CopiedOnly,
}

fn paste_text(text: &str, paste_mode: PasteMode) -> Result<IpcPasteResult> {
    let mut injector = Injector::new().context("could not initialize insertion backend")?;
    let text = match sanitize_for_paste(text, paste_mode) {
        PastePlan::Paste(text) => text,
        PastePlan::CopyOnly { text, .. } => {
            injector.copy_text(&text)?;
            return Ok(IpcPasteResult::CopiedOnly);
        }
        PastePlan::Skip { reason } => anyhow::bail!("paste text skipped by sanitizer: {reason}"),
    };

    match super::target::inspect_current_target() {
        super::target::TargetDecision::Allow => {}
        super::target::TargetDecision::CopyOnly(_) => {
            injector.copy_text(&text)?;
            return Ok(IpcPasteResult::CopiedOnly);
        }
        super::target::TargetDecision::Block(reason) => {
            anyhow::bail!("target safety blocked paste: {reason}");
        }
    }

    let focus = FocusSnapshot::capture().ok();
    let outcome = injector
        .paste_text_guarded(&text, paste_mode, || match focus.as_ref() {
            Some(snapshot) => snapshot.matches_current(),
            None => Ok(false),
        })
        .context("could not send paste command")?;
    Ok(match outcome {
        PasteOutcome::Pasted => IpcPasteResult::Pasted,
        PasteOutcome::CopiedOnly => IpcPasteResult::CopiedOnly,
    })
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
}
