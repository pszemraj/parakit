//! Local control socket for an already-running daemon.

#[cfg(any(unix, target_os = "windows"))]
use anyhow::Context;
use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
#[cfg(any(unix, target_os = "windows"))]
use std::cell::Cell;
use std::collections::VecDeque;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Read as _, Write};
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(unix, target_os = "windows"))]
use std::sync::Arc;
#[cfg(any(unix, target_os = "windows"))]
use std::thread;
#[cfg(any(unix, target_os = "windows"))]
use std::thread::JoinHandle;
#[cfg(any(unix, target_os = "windows"))]
use std::time::Duration;
use std::time::Instant;

#[cfg(any(unix, target_os = "windows"))]
use super::preflight;
#[cfg(any(unix, target_os = "windows"))]
use super::{
    inject::{FocusSnapshot, PasteMode},
    logging::Logger,
    notifications::Notifier,
    worker::{insert_text, FocusCheck, InsertOutcome},
};

#[cfg(target_os = "windows")]
mod windows_pipe;

#[cfg(any(unix, target_os = "windows"))]
const IPC_TRANSPORT_TIMEOUT: Duration = Duration::from_millis(750);
/// Response budget for insertion-lock commands. A command can wait behind
/// one complete paste transaction before running its own.
#[cfg(any(unix, target_os = "windows"))]
const IPC_INSERT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time stop finalization waits for an in-flight paste transaction.
/// This covers one normal macOS transaction while preserving a forced-exit
/// path if platform insertion code wedges.
#[cfg(any(unix, target_os = "windows"))]
const STOP_INSERTION_WAIT: Duration = Duration::from_secs(5);
/// Maximum accepted local control-protocol message size.
#[cfg(any(unix, target_os = "windows"))]
const IPC_MAX_MESSAGE_SIZE: usize = 64 * 1024;
/// Largest transcript ring whose complete history response stays below the
/// control-protocol message limit, including worst-case JSON escaping.
pub(crate) const MAX_TRANSCRIPT_HISTORY: usize = 100;
const HISTORY_PREVIEW_MAX_CHARS: usize = 72;
#[cfg(any(unix, target_os = "windows"))]
const STOP_RESPONSE_GRACE: Duration = Duration::from_millis(50);
#[cfg(any(unix, target_os = "windows"))]
const STOP_LOCK_POLL: Duration = Duration::from_millis(10);

/// Marker used by both local IPC transports when no daemon endpoint exists.
///
/// Keeping this distinct from other transport failures lets the CLI render a
/// stable, platform-independent message without hiding permission errors,
/// timeouts, truncated responses, or other actionable diagnostics.
#[cfg(any(unix, target_os = "windows"))]
#[derive(Debug)]
struct DaemonNotRunning;

#[cfg(any(unix, target_os = "windows"))]
impl std::fmt::Display for DaemonNotRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("daemon is not running")
    }
}

#[cfg(any(unix, target_os = "windows"))]
impl std::error::Error for DaemonNotRunning {}

/// Default number of transcripts kept in daemon memory when
/// `daemon.transcript_history` is unset.
pub(crate) const DEFAULT_TRANSCRIPT_HISTORY: usize = 10;

/// Command sent by helper subcommands to the running daemon.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IpcCommand {
    /// Return current daemon state.
    Status,
    /// Stop the daemon process.
    Stop,
    /// Copy a transcript remembered in memory.
    ///
    /// `index` is 0-based on the wire (0 is the most recent). The CLI number
    /// is 1-based and is converted to this 0-based wire index at the CLI boundary.
    CopyLast { index: usize },
    /// List transcripts remembered in memory, newest first.
    History {
        /// Cap the number of entries returned. `None` returns all of them.
        limit: Option<usize>,
    },
    /// Run the insertion path with caller-supplied text, without microphone use.
    TestPaste { text: String },
}

#[cfg(any(unix, target_os = "windows"))]
impl IpcCommand {
    fn response_timeout(&self) -> Duration {
        match self {
            // Both commands acquire SharedState's process-wide insertion
            // lock. CopyLast itself is quick, but it can queue behind a
            // synchronous paste transaction (including macOS modifier and
            // acknowledgement waits), so it needs the same response budget.
            Self::CopyLast { .. } | Self::TestPaste { .. } => IPC_INSERT_RESPONSE_TIMEOUT,
            Self::Status | Self::Stop | Self::History { .. } => IPC_TRANSPORT_TIMEOUT,
        }
    }
}

/// Return whether one Windows pipe read can be appended without exceeding
/// the control protocol's message limit.
///
/// `complete == false` means the pipe reported `ERROR_MORE_DATA`, so reaching
/// the limit already proves that the complete message is oversized.
#[cfg(any(target_os = "windows", test))]
fn pipe_message_fits_limit(buffered: usize, chunk_len: usize, complete: bool) -> bool {
    buffered.checked_add(chunk_len).is_some_and(|total| {
        total < IPC_MAX_MESSAGE_SIZE || (complete && total == IPC_MAX_MESSAGE_SIZE)
    })
}

/// Deserialize one control-socket request line as [`IpcCommand`].
///
/// # Errors
///
/// Returns an error when `raw` is not a current command payload.
#[cfg(any(unix, target_os = "windows"))]
fn parse_command(raw: &str) -> Result<IpcCommand> {
    serde_json::from_str(raw).context("invalid control command")
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
        /// Extended runtime detail for `--verbose status`. `None` covers the
        /// startup window before
        /// [`SharedState::set_info`] has run. Boxed because `StatusDetail` is
        /// much larger than the other `IpcResponse` variants, and `Status` is
        /// otherwise mostly `None` (serde transparently (de)serializes
        /// `Box<T>` as `T`, so the wire format is unaffected).
        detail: Option<Box<StatusDetail>>,
    },
    /// Transcript history listing, newest first.
    History { entries: Vec<HistoryEntry> },
    /// Command failed.
    Err { message: String },
}

/// One remembered transcript, as reported by the `History` command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct HistoryEntry {
    /// Position counting back from the most recent, 1-based to match the
    /// number the user passes to `copy-last`.
    pub(crate) index: usize,
    /// Seconds since the transcript was remembered.
    pub(crate) age_secs: u64,
    /// Transcript length in characters.
    pub(crate) chars: usize,
    /// Single-line, whitespace-collapsed excerpt for display.
    pub(crate) preview: String,
}

/// Extended daemon runtime detail returned by `Status` when
/// [`SharedState::set_info`] has run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct StatusDetail {
    /// Absent during startup until the worker publishes initial residency.
    pub(crate) model_status: Option<super::model_lifecycle::ModelStatus>,
    /// Daemon process id.
    pub(crate) pid: u32,
    /// Seconds since the daemon started serving IPC.
    pub(crate) uptime_secs: u64,
    /// Successful dictations completed this session.
    pub(crate) dictation_count: u64,
    /// Selected microphone summary.
    pub(crate) mic: String,
    /// Model file name.
    pub(crate) model: String,
    /// Model dtype/size label.
    pub(crate) dtype: String,
    /// Resolved runtime compute device summary.
    pub(crate) device: String,
    /// CrispASR backend label.
    pub(crate) backend: String,
    /// Inference thread count.
    pub(crate) threads: usize,
    /// Paste mode label.
    pub(crate) paste_mode: String,
    /// Whether audio cues are enabled.
    pub(crate) sounds: bool,
    /// Cleaning pipeline state label.
    pub(crate) cleaning: String,
    /// Transcription logging target, when enabled.
    pub(crate) log: Option<String>,
    /// Linux hotkey backend label, when applicable.
    pub(crate) hotkey_backend: Option<String>,
    /// Transcript history depth summary (`"3 of 10"`) or `"disabled"` when
    /// `daemon.transcript_history = 0`.
    pub(crate) history: String,
}

/// Daemon runtime info captured once at startup and exposed through `Status`.
///
/// Set once via [`SharedState::set_info`] after the model, microphone, and
/// insertion backend are all ready. Kept separate from [`StatusDetail`]
/// because it holds process-lifetime values (backend labels, thread counts)
/// as-computed at startup, while `StatusDetail` also carries values that
/// change per query (uptime, dictation count).
#[derive(Debug, Clone)]
pub(crate) struct DaemonInfo {
    /// Daemon process id.
    pub(crate) pid: u32,
    /// Model file name.
    pub(crate) model_name: String,
    /// Model dtype/size label.
    pub(crate) dtype: String,
    /// Selected microphone summary.
    pub(crate) mic_summary: String,
    /// CrispASR backend label.
    pub(crate) backend: String,
    /// Resolved runtime compute device summary.
    pub(crate) device: String,
    /// Inference thread count.
    pub(crate) threads: usize,
    /// Paste mode label.
    pub(crate) paste_mode: &'static str,
    /// Cleaning pipeline state label (e.g. `"on (12 rules)"` or `"off"`).
    pub(crate) cleaning_summary: String,
    /// Whether audio cues are enabled.
    pub(crate) sounds_on: bool,
    /// Transcription logging target, when enabled.
    pub(crate) log_summary: Option<String>,
    /// Linux hotkey backend label, when applicable.
    pub(crate) hotkey_backend_label: Option<&'static str>,
}

/// Shared in-memory daemon state used by the worker and IPC server.
pub(crate) struct SharedState {
    /// Admission gate shared by capture, queued work, and all insertion paths.
    pub(crate) activity: Arc<super::model_lifecycle::ActivityGate>,
    /// Session lifetime handshake shared by startup, worker, and IPC stop.
    pub(crate) shutdown: super::worker_shutdown::WorkerShutdown,
    inner: Mutex<StateSnapshot>,
    insertion: Mutex<()>,
    /// Runtime info set once startup completes; `None` until then.
    info: Mutex<Option<DaemonInfo>>,
    /// Instant the daemon started serving IPC, used to compute uptime.
    started_at: Instant,
    /// Count of successful dictations (transcript produced) this session.
    dictation_count: AtomicU64,
    /// Maximum number of transcripts kept in `inner.history`. Immutable for
    /// the life of the daemon process: history depth is read once at
    /// startup from `daemon.transcript_history`.
    history_limit: usize,
}

struct StateSnapshot {
    model_status: Option<super::model_lifecycle::ModelStatus>,
    phase: String,
    /// Remembered transcripts, newest first. Never persisted to disk.
    history: VecDeque<TranscriptEntry>,
}

/// One transcript remembered in daemon memory.
struct TranscriptEntry {
    text: String,
    at: Instant,
}

impl SharedState {
    /// Create an idle state snapshot with the default transcript history
    /// depth.
    ///
    /// # Returns
    ///
    /// Shared state with no remembered transcripts.
    pub(crate) fn new() -> Self {
        Self::with_history_limit(DEFAULT_TRANSCRIPT_HISTORY)
    }

    /// Create an idle state snapshot that remembers at most `limit`
    /// transcripts.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of transcripts kept in memory. `0`
    ///   disables `copy-last` and `history`.
    ///
    /// # Returns
    ///
    /// Shared state with no remembered transcripts.
    pub(crate) fn with_history_limit(limit: usize) -> Self {
        Self {
            activity: super::model_lifecycle::ActivityGate::new(),
            shutdown: super::worker_shutdown::WorkerShutdown::default(),
            inner: Mutex::new(StateSnapshot {
                model_status: None,
                phase: "starting".to_string(),
                history: VecDeque::new(),
            }),
            insertion: Mutex::new(()),
            info: Mutex::new(None),
            started_at: Instant::now(),
            dictation_count: AtomicU64::new(0),
            history_limit: limit,
        }
    }

    /// Update the visible daemon phase.
    pub(crate) fn set_phase(&self, phase: impl Into<String>) {
        self.inner.lock().phase = phase.into();
    }

    /// Publish session residency without changing history or daemon phase.
    pub(crate) fn set_model_status(&self, status: super::model_lifecycle::ModelStatus) {
        self.inner.lock().model_status = Some(status);
    }

    /// Read residency without extending the idle interval or loading the model.
    ///
    /// # Returns
    ///
    /// The most recent worker snapshot, or none before worker startup.
    pub(crate) fn model_status(&self) -> Option<super::model_lifecycle::ModelStatus> {
        self.inner.lock().model_status.clone()
    }

    /// Refresh device details after a reload (auto mode may select a new device).
    ///
    /// # Arguments
    ///
    /// * `backend` - Model backend reported by the new session.
    /// * `device` - Device summary computed during reload.
    pub(crate) fn update_engine_info(&self, backend: &str, device: String) {
        if let Some(info) = self.info.lock().as_mut() {
            info.backend = backend.to_string();
            info.device = device;
        }
    }

    /// Remember a transcript in memory, evicting the oldest entry once
    /// `history_limit` is exceeded. No-op when `history_limit` is 0.
    pub(crate) fn remember_transcript(&self, text: String) {
        if self.history_limit == 0 {
            return;
        }
        let mut inner = self.inner.lock();
        inner.history.push_front(TranscriptEntry {
            text,
            at: Instant::now(),
        });
        inner.history.truncate(self.history_limit);
    }

    /// Store daemon runtime info, making it visible to subsequent `Status`
    /// queries. Called once, after startup finishes resolving the model,
    /// microphone, and insertion backend.
    pub(crate) fn set_info(&self, info: DaemonInfo) {
        *self.info.lock() = Some(info);
    }

    /// Record one successful dictation (a non-empty transcript was
    /// produced). Called once per transcription success, not once per
    /// insertion attempt.
    pub(crate) fn record_dictation(&self) {
        self.dictation_count.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(any(unix, target_os = "windows", test))]
    fn status(&self) -> IpcResponse {
        let inner = self.inner.lock();
        let detail = self
            .info
            .lock()
            .as_ref()
            .map(|info| StatusDetail {
                model_status: inner.model_status.clone(),
                pid: info.pid,
                uptime_secs: self.started_at.elapsed().as_secs(),
                dictation_count: self.dictation_count.load(Ordering::Relaxed),
                mic: info.mic_summary.clone(),
                model: info.model_name.clone(),
                dtype: info.dtype.clone(),
                device: info.device.clone(),
                backend: info.backend.clone(),
                threads: info.threads,
                paste_mode: info.paste_mode.to_string(),
                sounds: info.sounds_on,
                cleaning: info.cleaning_summary.clone(),
                log: info.log_summary.clone(),
                hotkey_backend: info.hotkey_backend_label.map(str::to_string),
                history: self.history_label(inner.history.len()),
            })
            .map(Box::new);
        IpcResponse::Status {
            phase: inner.phase.clone(),
            last_transcript_len: inner.history.front().map(|entry| entry.text.len()),
            detail,
        }
    }

    /// Summarize remembered transcript count against `history_limit` for
    /// `StatusDetail::history`.
    ///
    /// # Returns
    ///
    /// `"disabled"` when `history_limit` is 0, otherwise `"{len} of
    /// {history_limit}"`.
    #[cfg(any(unix, target_os = "windows", test))]
    fn history_label(&self, history_len: usize) -> String {
        if self.history_limit == 0 {
            "disabled".to_string()
        } else {
            format!("{history_len} of {}", self.history_limit)
        }
    }

    /// Fail when transcript history is turned off, so `history` reports the
    /// same reason as `copy-last` instead of looking like an empty session.
    ///
    /// # Errors
    ///
    /// Returns an error when `history_limit` is 0.
    #[cfg(any(unix, target_os = "windows"))]
    fn ensure_history_enabled(&self) -> Result<()> {
        if self.history_limit == 0 {
            bail!("transcript history is disabled (daemon.transcript_history = 0)");
        }
        Ok(())
    }

    /// Resolve a transcript by 0-based wire index, honoring the
    /// history-disabled / empty / out-of-range errors in that priority
    /// order.
    ///
    /// # Returns
    ///
    /// A copy of the selected transcript.
    ///
    /// # Panics
    ///
    /// Does not panic: the index is checked while holding the history lock.
    ///
    /// # Errors
    ///
    /// Returns an error when `history_limit` is 0, no transcript has been
    /// remembered yet, or `index` is past the end of what is remembered.
    #[cfg(any(unix, target_os = "windows"))]
    pub(crate) fn resolve_transcript(&self, index: usize) -> Result<String> {
        self.ensure_history_enabled()?;
        let inner = self.inner.lock();
        let count = inner.history.len();
        if count == 0 {
            bail!("no transcript has been captured in this daemon session");
        }
        if index >= count {
            let noun = if count == 1 {
                "transcript"
            } else {
                "transcripts"
            };
            bail!("only {count} {noun} remembered in this daemon session");
        }
        Ok(inner.history[index].text.clone())
    }

    /// Snapshot remembered transcripts, newest first, for the `History`
    /// command.
    ///
    /// # Arguments
    ///
    /// * `limit` - Cap on the number of entries returned. `None` returns
    ///   every remembered transcript.
    ///
    /// # Returns
    ///
    /// Entries with 1-based `index` counting back from the most recent.
    #[cfg(any(unix, target_os = "windows", test))]
    fn history_snapshot(&self, limit: Option<usize>) -> Vec<HistoryEntry> {
        let inner = self.inner.lock();
        let now = Instant::now();
        inner
            .history
            .iter()
            .enumerate()
            .take(limit.unwrap_or(usize::MAX))
            .map(|(position, entry)| HistoryEntry {
                index: position + 1,
                age_secs: now.saturating_duration_since(entry.at).as_secs(),
                chars: entry.text.chars().count(),
                preview: preview_text(&entry.text),
            })
            .collect()
    }

    /// Run a clipboard/insertion transaction while excluding worker and IPC
    /// paste/copy paths.
    ///
    /// Clipboard staging plus a synthetic paste chord must be serialized
    /// process-wide. Otherwise `copy-last` or `test-paste` can race the
    /// worker clipboard transaction and paste or copy the wrong text.
    ///
    /// # Returns
    ///
    /// The closure result.
    pub(crate) fn with_insertion_lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let _activity = self.activity.begin();
        let _guard = self.insertion.lock();
        f()
    }
}

/// Running control socket server.
#[cfg(any(unix, target_os = "windows"))]
pub(crate) struct IpcServer {
    #[cfg(unix)]
    path: PathBuf,
    _thread: JoinHandle<()>,
}

#[cfg(unix)]
impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the daemon control socket.
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
/// Returns an error when the control socket cannot be bound.
#[cfg(any(unix, target_os = "windows"))]
pub(crate) fn spawn_server(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: Arc<Logger>,
    notifier: Notifier,
) -> Result<IpcServer> {
    spawn_server_impl(state, paste_mode, keep_transcript_clipboard, log, notifier)
}

/// Check whether the daemon control endpoint answers a status request.
///
/// # Returns
///
/// `true` only when a live endpoint returns a status response.
///
/// # Errors
///
/// Returns transport errors other than an absent endpoint.
#[cfg(any(unix, target_os = "windows"))]
pub(crate) fn daemon_responsive() -> Result<bool> {
    match send_command(&IpcCommand::Status) {
        Ok(IpcResponse::Status { .. }) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) if error.is::<DaemonNotRunning>() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Run one IPC client command and print a concise response.
///
/// # Arguments
///
/// * `command` - Command to send to the running daemon.
/// * `quiet` - Suppress stdout on success.
/// * `verbose` - Print an extended detail block for `Status` responses. Has
///   no effect on other commands, so their output is unaffected either way.
///
/// # Returns
///
/// `Ok(())` when the command completed successfully.
///
/// # Errors
///
/// Returns an error when a command that needs live daemon state is used while
/// no daemon is listening, or when the daemon reports failure. An absent
/// daemon is a successful state for `Status` and `Stop`.
pub(crate) fn run_client(command: IpcCommand, quiet: bool, verbose: bool) -> Result<()> {
    let stop_deadline =
        Instant::now() + STOP_INSERTION_WAIT + STOP_RESPONSE_GRACE + IPC_TRANSPORT_TIMEOUT;
    let response = match send_command(&command) {
        Ok(response) => response,
        Err(err) if err.is::<DaemonNotRunning>() => {
            return handle_daemon_not_running(&command, quiet, stop_deadline);
        }
        Err(err) => return Err(err),
    };
    match response {
        IpcResponse::Ok { message } => {
            if matches!(&command, IpcCommand::Stop) {
                wait_for_daemon_stop(stop_deadline, false)?;
            }
            if !quiet {
                println!("{}", completed_command_message(&command, &message));
            }
            Ok(())
        }
        IpcResponse::Status {
            phase,
            last_transcript_len,
            detail,
        } => {
            if !quiet {
                // These two lines must stay byte-identical to the pre-verbose
                // output: existing scripts parse them.
                println!("parakit: {phase}");
                println!(
                    "last transcript: {}",
                    last_transcript_summary(last_transcript_len, detail.as_deref())
                );
                if verbose {
                    print_status_detail(detail.as_deref());
                }
            }
            Ok(())
        }
        IpcResponse::History { entries } => {
            if !quiet {
                print_history(&entries);
            }
            Ok(())
        }
        IpcResponse::Err { message } => bail!("{message}"),
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn completed_command_message<'a>(command: &IpcCommand, server_message: &'a str) -> &'a str {
    if matches!(command, IpcCommand::Stop) {
        "stopped"
    } else {
        server_message
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn wait_for_daemon_stop(mut deadline: Instant, stop_pending: bool) -> Result<()> {
    if stop_pending {
        // An absent endpoint can mean startup, not just teardown. Deliver the
        // pending stop within a bounded discovery window, then allow the
        // daemon its full existing worker/insertion shutdown budget.
        while preflight::singleton_lock_held()? && Instant::now() < deadline {
            match send_command(&IpcCommand::Stop) {
                Ok(IpcResponse::Ok { .. }) => {
                    deadline = Instant::now()
                        + STOP_INSERTION_WAIT
                        + STOP_RESPONSE_GRACE
                        + IPC_TRANSPORT_TIMEOUT;
                    break;
                }
                Ok(IpcResponse::Err { message }) => bail!("{message}"),
                Ok(_) => bail!("unexpected daemon stop response"),
                Err(error) if error.is::<DaemonNotRunning>() => {}
                Err(error) => return Err(error),
            }
            thread::sleep(STOP_LOCK_POLL);
        }
    }
    wait_for_daemon_stop_with_probe(deadline, preflight::singleton_lock_held)
}

#[cfg(any(unix, target_os = "windows"))]
fn wait_for_daemon_stop_with_probe(
    deadline: Instant,
    mut lock_held: impl FnMut() -> Result<bool>,
) -> Result<()> {
    while lock_held().context("check daemon shutdown")? {
        if Instant::now() >= deadline {
            #[cfg(not(target_os = "windows"))]
            bail!("daemon stop timed out; if this persists, identify the daemon with `pgrep -af parakit` and end that process");
            #[cfg(target_os = "windows")]
            bail!("daemon stop timed out; if this persists, identify the daemon with `Get-Process parakit` and end it with `Stop-Process -Id <PID>`");
        }
        thread::sleep(STOP_LOCK_POLL);
    }
    Ok(())
}

#[cfg(any(unix, target_os = "windows"))]
#[derive(Debug, Eq, PartialEq)]
enum DaemonNotRunningDisposition {
    Success(&'static str),
    WaitForStop,
    Error(&'static str),
}

/// Return the user-facing result for a command when no daemon endpoint exists.
///
/// Status queries and stop requests are idempotent state checks once the
/// singleton lock is free. A held lock means the daemon may still be starting
/// or stopping, so reporting a settled "not running" state would be misleading.
#[cfg(any(unix, target_os = "windows"))]
fn daemon_not_running_disposition(
    command: &IpcCommand,
    singleton_held: bool,
) -> DaemonNotRunningDisposition {
    match command {
        IpcCommand::Stop if singleton_held => DaemonNotRunningDisposition::WaitForStop,
        IpcCommand::Status if singleton_held => {
            DaemonNotRunningDisposition::Error(
                "daemon control endpoint is unavailable while the singleton lock is held; it may still be starting or stopping",
            )
        }
        IpcCommand::Status => DaemonNotRunningDisposition::Success("parakit: not running"),
        IpcCommand::Stop => {
            DaemonNotRunningDisposition::Success("parakit: not running; nothing to stop")
        }
        IpcCommand::CopyLast { .. } | IpcCommand::History { .. } | IpcCommand::TestPaste { .. } => {
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            )
        }
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn handle_daemon_not_running(command: &IpcCommand, quiet: bool, deadline: Instant) -> Result<()> {
    let singleton_held = if matches!(command, IpcCommand::Status | IpcCommand::Stop) {
        preflight::singleton_lock_held().context("probe existing daemon singleton lock")?
    } else {
        false
    };
    match daemon_not_running_disposition(command, singleton_held) {
        DaemonNotRunningDisposition::WaitForStop => {
            wait_for_daemon_stop(deadline, true)?;
            if !quiet {
                println!("stopped");
            }
            Ok(())
        }
        DaemonNotRunningDisposition::Success(message) => {
            if !quiet {
                println!("{message}");
            }
            Ok(())
        }
        DaemonNotRunningDisposition::Error(message) => bail!("{message}"),
    }
}

/// Format the stable second status line, distinguishing disabled history from
/// an enabled history that simply has no transcript yet.
fn last_transcript_summary(
    last_transcript_len: Option<usize>,
    detail: Option<&StatusDetail>,
) -> String {
    match last_transcript_len {
        Some(len) => format!("{len} bytes"),
        None if detail.is_some_and(|detail| detail.history == "disabled") => {
            "history disabled".to_string()
        }
        None => "none".to_string(),
    }
}

/// Print the `History` command's transcript listing.
///
/// # Arguments
///
/// * `entries` - Transcripts remembered by the daemon, newest first.
fn print_history(entries: &[HistoryEntry]) {
    if entries.is_empty() {
        println!("no transcripts remembered in this daemon session");
        return;
    }
    for entry in entries {
        println!(
            "{index:<3}{age:<10}{chars:>4} chars  {preview}",
            index = entry.index,
            age = format_age(entry.age_secs),
            chars = entry.chars,
            preview = entry.preview,
        );
    }
}

/// Print the `--verbose status` detail block.
///
/// # Arguments
///
/// * `detail` - Extended runtime detail from the daemon's `Status` response,
///   or `None` when the daemon has not yet called `set_info` (e.g. still
///   starting up).
fn print_status_detail(detail: Option<&StatusDetail>) {
    let Some(detail) = detail else {
        println!("  detail unavailable (daemon starting)");
        return;
    };
    println!("  pid:        {}", detail.pid);
    println!("  uptime:     {}", format_uptime(detail.uptime_secs));
    println!("  dictations: {}", detail.dictation_count);
    println!("  mic:        {}", detail.mic);
    println!("  model:      {} ({})", detail.model, detail.dtype);
    if let Some(model) = &detail.model_status {
        println!("  residency:  {}", model.residency.label());
        if model.idle_minutes == 0 {
            println!("  model idle: disabled");
        } else {
            println!("  model idle: {} minutes", model.idle_minutes);
        }
        if let Some(error) = &model.last_error {
            println!("  model error: {error}");
        }
    }
    println!(
        "  device:     {} ({}, {} threads)",
        detail.device, detail.backend, detail.threads
    );
    println!("  paste mode: {}", detail.paste_mode);
    println!("  sounds:     {}", if detail.sounds { "on" } else { "off" });
    println!("  cleaning:   {}", detail.cleaning);
    println!("  logging:    {}", detail.log.as_deref().unwrap_or("off"));
    println!("  history:    {}", detail.history);
    if let Some(hotkey_backend) = &detail.hotkey_backend {
        println!("  hotkey:     {hotkey_backend}");
    }
}

/// Humanize a duration in seconds as `1h 23m`, `23m 5s`, or `5s`.
///
/// # Returns
///
/// The largest two non-zero units, or `0s` for a zero duration.
fn format_uptime(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Humanize a transcript's age as `format_uptime` output plus `" ago"`.
///
/// # Returns
///
/// e.g. `"12s ago"`, `"3m 5s ago"`, or `"1h 23m ago"`.
fn format_age(age_secs: u64) -> String {
    format!("{} ago", format_uptime(age_secs))
}

/// Collapse a transcript into a single-line preview for `history` output.
///
/// # Returns
///
/// The transcript with every run of whitespace (including newlines)
/// collapsed to a single space and trimmed, truncated on a char boundary
/// to 72 characters with a trailing `…` when truncation occurred.
fn preview_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= HISTORY_PREVIEW_MAX_CHARS {
        return collapsed;
    }
    let mut truncated: String = collapsed.chars().take(HISTORY_PREVIEW_MAX_CHARS).collect();
    truncated.push('…');
    truncated
}

#[cfg(unix)]
fn spawn_server_impl(
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(any(unix, target_os = "windows"))]
struct CommandOutcome {
    response: IpcResponse,
    stop_after_response: bool,
}

/// Human-readable reference to a 0-based wire history index, used in
/// `copy-last` success messages.
///
/// # Returns
///
/// `"last transcript"` for index 0 (byte-identical to the pre-history
/// wording), otherwise `"transcript {index + 1}"` (1-based, matching the
/// CLI's `N` argument).
#[cfg(any(unix, target_os = "windows"))]
fn history_ref_label(index: usize) -> String {
    if index == 0 {
        "last transcript".to_string()
    } else {
        format!("transcript {}", index + 1)
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn handle_command(
    command: IpcCommand,
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: &Logger,
    notifier: &Notifier,
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
        IpcCommand::CopyLast { index } => {
            state.with_insertion_lock(|| {
                let text = state.resolve_transcript(index)?;
                copy_text(&text)
            })?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: format!("copied {}", history_ref_label(index)),
                },
                stop_after_response: false,
            })
        }
        IpcCommand::History { limit } => {
            state.ensure_history_enabled()?;
            Ok(CommandOutcome {
                response: IpcResponse::History {
                    entries: state.history_snapshot(limit),
                },
                stop_after_response: false,
            })
        }
        IpcCommand::TestPaste { text } => {
            let result = state.with_insertion_lock(|| {
                paste_text(&text, paste_mode, keep_transcript_clipboard, log, notifier)
            })?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: match result {
                        InsertOutcome::Pasted => "test paste sent",
                        InsertOutcome::PastedUnverified => {
                            "test paste sent (insertion unconfirmed)"
                        }
                        InsertOutcome::CopiedOnly => "test text copied",
                        InsertOutcome::Blocked => "test paste blocked",
                        InsertOutcome::Skipped => "test paste skipped",
                    }
                    .to_string(),
                },
                stop_after_response: false,
            })
        }
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn client_command_outcome(
    command: Result<IpcCommand>,
    state: &Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: &Logger,
    notifier: &Notifier,
) -> CommandOutcome {
    match command.and_then(|command| {
        handle_command(
            command,
            Arc::clone(state),
            paste_mode,
            keep_transcript_clipboard,
            log,
            notifier,
        )
    }) {
        Ok(outcome) => outcome,
        Err(err) => CommandOutcome {
            response: IpcResponse::Err {
                message: format!("{err:#}"),
            },
            stop_after_response: false,
        },
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn finish_stop_after_response<R>(
    state: &SharedState,
    cleanup_path: Option<std::path::PathBuf>,
    terminate: impl FnOnce(bool) -> R,
) -> R {
    finish_stop_after_response_with_wait(state, cleanup_path, STOP_INSERTION_WAIT, terminate)
}

#[cfg(any(unix, target_os = "windows"))]
fn finish_stop_after_response_with_wait<R>(
    state: &SharedState,
    cleanup_path: Option<std::path::PathBuf>,
    insertion_wait: Duration,
    terminate: impl FnOnce(bool) -> R,
) -> R {
    // A response has already been written, so waiting here does not consume
    // the client's transport timeout. Holding the lock through termination
    // lets an in-flight paste release every synthetic modifier and prevents a
    // new paste from starting during the response grace period. A wedged
    // insertion must not prevent the stop command from terminating the daemon.
    // Wait before taking insertion: the worker may need that lock to finish.
    // In particular, Metal's static device teardown requires all live session
    // buffers to be released, which process::exit alone cannot do on a worker.
    let started = Instant::now();
    let worker_finished = state
        .shutdown
        .request_and_wait(&state.activity, insertion_wait);
    let insertion_guard = state
        .insertion
        .try_lock_for(insertion_wait.saturating_sub(started.elapsed()));
    if insertion_guard.is_some() {
        thread::sleep(STOP_RESPONSE_GRACE);
    }
    if let Some(path) = cleanup_path {
        let _ = std::fs::remove_file(path);
    }
    terminate(worker_finished && insertion_guard.is_some())
}

#[cfg(any(unix, target_os = "windows"))]
fn terminate_daemon(graceful: bool) -> ! {
    super::worker_shutdown::terminate_process(0, graceful)
}

#[cfg(any(unix, target_os = "windows"))]
fn paste_text(
    text: &str,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertOutcome> {
    let focus = FocusSnapshot::capture().ok();
    let mut injector = None;
    let focus_verification = Cell::new("not_applicable");
    let focus_check = FocusCheck {
        snapshot: focus.as_ref(),
        verification: &focus_verification,
    };
    insert_text(
        &mut injector,
        text,
        paste_mode,
        keep_transcript_clipboard,
        focus_check,
        (log, notifier),
    )
    .map(|report| report.outcome)
    .context("could not send paste command")
}

#[cfg(any(unix, target_os = "windows"))]
fn copy_text(text: &str) -> Result<()> {
    let mut injector = super::inject::Injector::new().context("could not initialize clipboard")?;
    injector.copy_text(text)
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
fn send_command(command: &IpcCommand) -> Result<IpcResponse> {
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

#[cfg(target_os = "windows")]
fn spawn_server_impl(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: Arc<Logger>,
    notifier: Notifier,
) -> Result<IpcServer> {
    windows_pipe::spawn_server_impl(state, paste_mode, keep_transcript_clipboard, log, notifier)
}

#[cfg(target_os = "windows")]
fn send_command(command: &IpcCommand) -> Result<IpcResponse> {
    windows_pipe::send_command(command)
}

#[cfg(test)]
#[path = "ipc_tests.rs"]
mod tests;
