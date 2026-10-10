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

#[cfg(target_os = "windows")]
mod windows_pipe {
    use super::*;
    use std::{
        ffi::c_void,
        ptr::{null, null_mut},
    };
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};

    const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
    const IPC_CLIENT_TIMEOUT_MS: u32 = 750;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const PIPE_TYPE_MESSAGE: u32 = 0x0000_0004;
    const PIPE_READMODE_MESSAGE: u32 = 0x0000_0002;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_MORE_DATA: u32 = 234;
    const ERROR_OPERATION_ABORTED: u32 = 995;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_SEM_TIMEOUT: u32 = 121;
    const SDDL_REVISION_1: u32 = 1;
    const TRUE: i32 = 1;
    const FALSE: i32 = 0;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const WAIT_FAILED: u32 = 0xffff_ffff;
    const INFINITE: u32 = 0xffff_ffff;
    const CLIENT_CONNECT_RETRY: Duration = Duration::from_millis(10);

    #[repr(C)]
    struct RawSecurityAttributes {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: PCWSTR,
            string_security_descriptor_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CancelIoEx(file: HANDLE, overlapped: *mut c_void) -> i32;
        fn ConnectNamedPipe(pipe: HANDLE, overlapped: *mut c_void) -> i32;
        fn CreateEventW(
            event_attributes: *mut c_void,
            manual_reset: i32,
            initial_state: i32,
            name: PCWSTR,
        ) -> HANDLE;
        fn CreateFileW(
            file_name: PCWSTR,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const RawSecurityAttributes,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: HANDLE,
        ) -> HANDLE;
        fn CreateNamedPipeW(
            name: PCWSTR,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer_size: u32,
            in_buffer_size: u32,
            default_timeout: u32,
            security_attributes: *mut RawSecurityAttributes,
        ) -> HANDLE;
        fn GetLastError() -> u32;
        fn GetOverlappedResult(
            file: HANDLE,
            overlapped: *mut c_void,
            bytes_transferred: *mut u32,
            wait: i32,
        ) -> i32;
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
        fn ReadFile(
            file: HANDLE,
            buffer: *mut c_void,
            bytes_to_read: u32,
            bytes_read: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn SetNamedPipeHandleState(
            named_pipe: HANDLE,
            mode: *mut u32,
            max_collection_count: *mut u32,
            collect_data_timeout: *mut u32,
        ) -> i32;
        fn WaitForSingleObject(handle: HANDLE, milliseconds: u32) -> u32;
        fn WaitNamedPipeW(name: PCWSTR, timeout: u32) -> i32;
        fn WriteFile(
            file: HANDLE,
            buffer: *const c_void,
            bytes_to_write: u32,
            bytes_written: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    /// Start the Windows named-pipe daemon control server.
    ///
    /// # Arguments
    ///
    /// * `state` - Shared daemon status and transcript history.
    /// * `paste_mode` - Paste mode used by paste-related commands.
    /// * `keep_transcript_clipboard` - Whether command insertion leaves text on
    ///   the clipboard.
    /// * `log` - Logger used for background transport failures.
    ///
    /// # Returns
    ///
    /// A server handle that owns the listener thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the per-user pipe name or listener thread cannot
    /// be initialized.
    pub(super) fn spawn_server_impl(
        state: Arc<SharedState>,
        paste_mode: PasteMode,
        keep_transcript_clipboard: bool,
        log: Arc<Logger>,
        notifier: Notifier,
    ) -> Result<IpcServer> {
        let identity = DaemonPipeIdentity::current()?;
        let thread = thread::Builder::new()
            .name("parakit-ipc".into())
            .spawn(move || loop {
                match create_server_pipe(&identity).and_then(|pipe| {
                    connect_server_pipe(&pipe)?;
                    Ok(pipe)
                }) {
                    Ok(pipe) => {
                        let state = Arc::clone(&state);
                        let log = Arc::clone(&log);
                        let notifier = notifier.clone();
                        let _ = thread::Builder::new()
                            .name("parakit-ipc-client".into())
                            .spawn(move || {
                                handle_client(
                                    pipe,
                                    state,
                                    paste_mode,
                                    keep_transcript_clipboard,
                                    log,
                                    notifier,
                                )
                            });
                    }
                    Err(err) => {
                        log.warn(format!("Windows daemon named pipe failed: {err:#}"));
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            })
            .context("spawn Windows daemon named pipe")?;
        Ok(IpcServer { _thread: thread })
    }

    /// Send one command to the running Windows daemon.
    ///
    /// # Returns
    ///
    /// The daemon response decoded from the named-pipe reply.
    ///
    /// # Errors
    ///
    /// Returns an error when the per-user named pipe is unavailable, transport
    /// I/O fails, or the response cannot be decoded.
    pub(super) fn send_command(command: &IpcCommand) -> Result<IpcResponse> {
        let response_timeout_ms = command
            .response_timeout()
            .as_millis()
            .clamp(1, u128::from(u32::MAX)) as u32;
        let identity = DaemonPipeIdentity::current()?;
        let pipe = connect_client_pipe(&identity.pipe_name)?;
        write_json_message(&pipe, &command).context("write Windows daemon control command")?;
        let response = read_pipe_message_with_timeout(&pipe, response_timeout_ms)
            .context("read Windows daemon control response")?;
        serde_json::from_slice(&response).context("parse Windows daemon control response")
    }

    fn handle_client(
        pipe: PipeHandle,
        state: Arc<SharedState>,
        paste_mode: PasteMode,
        keep_transcript_clipboard: bool,
        log: Arc<Logger>,
        notifier: Notifier,
    ) {
        let outcome = client_command_outcome(
            read_command(&pipe),
            &state,
            paste_mode,
            keep_transcript_clipboard,
            log.as_ref(),
            &notifier,
        );

        if let Err(err) = write_json_message(&pipe, &outcome.response) {
            log.warn(format!("Windows daemon control response failed: {err:#}"));
        }

        if outcome.stop_after_response {
            finish_stop_after_response(&state, None, terminate_daemon);
        }
    }

    fn read_command(pipe: &PipeHandle) -> Result<IpcCommand> {
        let bytes =
            read_pipe_message(pipe).context("read Windows daemon control command failed")?;
        super::parse_command(&String::from_utf8_lossy(&bytes))
    }

    struct DaemonPipeIdentity {
        pipe_name: Vec<u16>,
        user_sid: String,
    }

    impl DaemonPipeIdentity {
        fn current() -> Result<Self> {
            let user_sid = super::super::windows_security::current_user_sid_string()
                .context("read current Windows user SID for daemon pipe")?;
            let pipe_name = encode_wide_null(&format!(r"\\.\pipe\parakit-daemon-{user_sid}"));
            Ok(Self {
                pipe_name,
                user_sid,
            })
        }
    }

    fn create_server_pipe(identity: &DaemonPipeIdentity) -> Result<PipeHandle> {
        let mut security = PipeSecurity::for_user_sid(&identity.user_sid)?;
        let mut attributes = security.attributes();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(identity.pipe_name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                server_pipe_mode(),
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_SIZE,
                PIPE_BUFFER_SIZE,
                IPC_CLIENT_TIMEOUT_MS,
                &mut attributes,
            )
        };
        if is_invalid_handle(handle) {
            return Err(last_error("CreateNamedPipeW failed"));
        }
        Ok(PipeHandle(handle))
    }

    const fn server_pipe_mode() -> u32 {
        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS
    }

    fn connect_server_pipe(pipe: &PipeHandle) -> Result<()> {
        let event = EventHandle::create()?;
        let mut overlapped = RawOverlapped::new(event.0);
        if unsafe { ConnectNamedPipe(pipe.0, overlapped.as_mut_ptr()) } != 0 {
            return Ok(());
        }
        let err = unsafe { GetLastError() };
        match err {
            ERROR_IO_PENDING => {
                wait_for_overlapped(
                    pipe,
                    &mut overlapped,
                    INFINITE,
                    "ConnectNamedPipe Windows daemon control pipe failed",
                    overlapped_result,
                )?;
                Ok(())
            }
            ERROR_PIPE_CONNECTED => Ok(()),
            _ => Err(win32_error("ConnectNamedPipe failed", err)),
        }
    }

    fn connect_client_pipe(pipe_name: &[u16]) -> Result<PipeHandle> {
        let started = std::time::Instant::now();
        loop {
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(pipe_name.as_ptr()),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null_mut(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                    HANDLE::default(),
                )
            };
            if !is_invalid_handle(handle) {
                let pipe = PipeHandle(handle);
                // CreateNamedPipeW makes a message-type server pipe, but a
                // client handle returned by CreateFileW starts in byte-read
                // mode. The reader below relies on ERROR_MORE_DATA and pipe
                // message boundaries, so opt in before the first read.
                set_client_message_read_mode(&pipe)?;
                return Ok(pipe);
            }

            let err = unsafe { GetLastError() };
            match err {
                ERROR_PIPE_BUSY => {
                    let Some(wait_ms) = remaining_timeout_ms(
                        started,
                        std::time::Instant::now(),
                        IPC_CLIENT_TIMEOUT_MS,
                    ) else {
                        return Err(win32_error(
                            "CreateFileW Windows daemon control pipe failed",
                            err,
                        ));
                    };
                    if unsafe { WaitNamedPipeW(PCWSTR(pipe_name.as_ptr()), wait_ms) } == 0 {
                        let wait_err = unsafe { GetLastError() };
                        if wait_err == ERROR_SEM_TIMEOUT {
                            return Err(win32_error(
                                "WaitNamedPipeW Windows daemon control pipe timed out",
                                wait_err,
                            ));
                        }
                        // WaitNamedPipeW is only a readiness hint; the server
                        // can close an instance or another client can win the
                        // race before this client retries CreateFileW.
                        if is_retryable_pipe_availability_error(wait_err) {
                            let Some(sleep) = retry_sleep_duration(
                                started,
                                std::time::Instant::now(),
                                IPC_CLIENT_TIMEOUT_MS,
                                CLIENT_CONNECT_RETRY,
                            ) else {
                                if wait_err == ERROR_FILE_NOT_FOUND {
                                    return Err(DaemonNotRunning.into());
                                }
                                return Err(win32_error(
                                    "WaitNamedPipeW Windows daemon control pipe failed",
                                    wait_err,
                                ));
                            };
                            thread::sleep(sleep);
                            continue;
                        }
                        return Err(win32_error(
                            "WaitNamedPipeW Windows daemon control pipe failed",
                            wait_err,
                        ));
                    }
                }
                ERROR_FILE_NOT_FOUND => {
                    let Some(sleep) = retry_sleep_duration(
                        started,
                        std::time::Instant::now(),
                        IPC_CLIENT_TIMEOUT_MS,
                        CLIENT_CONNECT_RETRY,
                    ) else {
                        return Err(DaemonNotRunning.into());
                    };
                    thread::sleep(sleep);
                }
                _ => {
                    return Err(win32_error(
                        "CreateFileW Windows daemon control pipe failed",
                        err,
                    ));
                }
            }
        }
    }

    fn set_client_message_read_mode(pipe: &PipeHandle) -> Result<()> {
        let mut mode = PIPE_READMODE_MESSAGE | PIPE_WAIT;
        if unsafe { SetNamedPipeHandleState(pipe.0, &mut mode, null_mut(), null_mut()) } == 0 {
            return Err(last_error(
                "SetNamedPipeHandleState Windows daemon control pipe failed",
            ));
        }
        Ok(())
    }

    fn read_pipe_message(pipe: &PipeHandle) -> Result<Vec<u8>> {
        read_pipe_message_with_timeout(pipe, IPC_CLIENT_TIMEOUT_MS)
    }

    fn read_pipe_message_with_timeout(pipe: &PipeHandle, timeout_ms: u32) -> Result<Vec<u8>> {
        let mut chunk = vec![0_u8; PIPE_BUFFER_SIZE as usize];
        let mut message = Vec::new();
        let started = std::time::Instant::now();
        loop {
            let remaining_ms = remaining_timeout_ms(started, std::time::Instant::now(), timeout_ms)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "ReadFile Windows daemon control pipe failed: timed out after \
                             {timeout_ms}ms"
                    )
                })?;
            let (read, complete) = match read_pipe_chunk(pipe, &mut chunk, remaining_ms)? {
                PipeReadChunk::Complete(read) => (read, true),
                PipeReadChunk::MoreData(read) => (read, false),
            };
            if !pipe_message_fits_limit(message.len(), read, complete) {
                bail!("Windows daemon control message exceeds 64 KiB");
            }
            message.extend_from_slice(&chunk[..read]);
            if complete {
                return Ok(message);
            }
        }
    }

    fn write_json_message<T: Serialize>(pipe: &PipeHandle, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).context("serialize Windows daemon control JSON")?;
        write_pipe_message(pipe, &bytes)
    }

    fn write_pipe_message(pipe: &PipeHandle, bytes: &[u8]) -> Result<()> {
        let bytes_to_write =
            u32::try_from(bytes.len()).context("Windows daemon control message exceeds 4 GiB")?;

        // Message-type pipes frame each WriteFile call as a separate message.
        // The control protocol sends exactly one JSON payload per message.
        let event = EventHandle::create()?;
        let mut overlapped = RawOverlapped::new(event.0);
        let mut written = 0_u32;
        let ok = unsafe {
            WriteFile(
                pipe.0,
                bytes.as_ptr().cast(),
                bytes_to_write,
                &mut written,
                overlapped.as_mut_ptr(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                return Err(win32_error(
                    "WriteFile Windows daemon control pipe failed",
                    err,
                ));
            }
            written = wait_for_overlapped(
                pipe,
                &mut overlapped,
                IPC_CLIENT_TIMEOUT_MS,
                "WriteFile Windows daemon control pipe failed",
                overlapped_result,
            )?;
        }
        if written as usize != bytes.len() {
            bail!(
                "short write to Windows daemon control pipe: wrote {} of {} bytes",
                written,
                bytes.len()
            );
        }
        Ok(())
    }

    enum PipeReadChunk {
        Complete(usize),
        MoreData(usize),
    }

    fn read_pipe_chunk(
        pipe: &PipeHandle,
        chunk: &mut [u8],
        timeout_ms: u32,
    ) -> Result<PipeReadChunk> {
        let event = EventHandle::create()?;
        let mut overlapped = RawOverlapped::new(event.0);
        let mut read = 0_u32;
        let ok = unsafe {
            ReadFile(
                pipe.0,
                chunk.as_mut_ptr().cast(),
                PIPE_BUFFER_SIZE,
                &mut read,
                overlapped.as_mut_ptr(),
            )
        };
        if ok != 0 {
            return Ok(PipeReadChunk::Complete(read as usize));
        }

        let err = unsafe { GetLastError() };
        match err {
            ERROR_IO_PENDING => wait_for_overlapped(
                pipe,
                &mut overlapped,
                timeout_ms,
                "ReadFile Windows daemon control pipe failed",
                read_overlapped_result,
            ),
            ERROR_MORE_DATA => Ok(PipeReadChunk::MoreData(read as usize)),
            _ => Err(win32_error(
                "ReadFile Windows daemon control pipe failed",
                err,
            )),
        }
    }

    fn read_overlapped_result(
        pipe: &PipeHandle,
        overlapped: &mut RawOverlapped,
        wait: i32,
    ) -> std::result::Result<PipeReadChunk, u32> {
        let mut transferred = 0_u32;
        if unsafe { GetOverlappedResult(pipe.0, overlapped.as_mut_ptr(), &mut transferred, wait) }
            != 0
        {
            return Ok(PipeReadChunk::Complete(transferred as usize));
        }

        match unsafe { GetLastError() } {
            ERROR_MORE_DATA => Ok(PipeReadChunk::MoreData(transferred as usize)),
            err => Err(err),
        }
    }

    fn wait_for_overlapped<T>(
        pipe: &PipeHandle,
        overlapped: &mut RawOverlapped,
        timeout_ms: u32,
        label: &str,
        completion: fn(&PipeHandle, &mut RawOverlapped, i32) -> std::result::Result<T, u32>,
    ) -> Result<T> {
        match unsafe { WaitForSingleObject(overlapped.h_event, timeout_ms) } {
            WAIT_OBJECT_0 => match completion(pipe, overlapped, FALSE) {
                Ok(transferred) => Ok(transferred),
                Err(err) => Err(win32_error(label, err)),
            },
            WAIT_TIMEOUT => {
                let _ = unsafe { CancelIoEx(pipe.0, overlapped.as_mut_ptr()) };
                match completion(pipe, overlapped, TRUE) {
                    Ok(transferred) => Ok(transferred),
                    Err(ERROR_OPERATION_ABORTED) => {
                        bail!("{label}: timed out after {timeout_ms}ms")
                    }
                    Err(err) => Err(win32_error(label, err)),
                }
            }
            WAIT_FAILED => Err(last_error("WaitForSingleObject failed")),
            other => bail!("WaitForSingleObject returned unexpected status {other}"),
        }
    }

    fn overlapped_result(
        pipe: &PipeHandle,
        overlapped: &mut RawOverlapped,
        wait: i32,
    ) -> std::result::Result<u32, u32> {
        let mut transferred = 0_u32;
        if unsafe { GetOverlappedResult(pipe.0, overlapped.as_mut_ptr(), &mut transferred, wait) }
            != 0
        {
            return Ok(transferred);
        }

        Err(unsafe { GetLastError() })
    }

    #[repr(C)]
    struct RawOverlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: HANDLE,
    }

    impl RawOverlapped {
        fn new(event: HANDLE) -> Self {
            Self {
                internal: 0,
                internal_high: 0,
                offset: 0,
                offset_high: 0,
                h_event: event,
            }
        }

        fn as_mut_ptr(&mut self) -> *mut c_void {
            std::ptr::from_mut(self).cast()
        }
    }

    struct EventHandle(HANDLE);

    impl EventHandle {
        fn create() -> Result<Self> {
            let handle = unsafe { CreateEventW(null_mut(), TRUE, FALSE, PCWSTR(null())) };
            if is_null_handle(handle) {
                return Err(last_error("CreateEventW failed"));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for EventHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct PipeSecurity {
        descriptor: *mut c_void,
    }

    impl PipeSecurity {
        fn for_user_sid(user_sid: &str) -> Result<Self> {
            let sddl = encode_wide_null(&current_user_only_pipe_sddl(user_sid));
            let mut descriptor = null_mut::<c_void>();
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(sddl.as_ptr()),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            } == 0
            {
                return Err(last_error(
                    "ConvertStringSecurityDescriptorToSecurityDescriptorW failed",
                ));
            }
            Ok(Self { descriptor })
        }

        fn attributes(&mut self) -> RawSecurityAttributes {
            RawSecurityAttributes {
                n_length: std::mem::size_of::<RawSecurityAttributes>() as u32,
                lp_security_descriptor: self.descriptor,
                b_inherit_handle: 0,
            }
        }
    }

    fn current_user_only_pipe_sddl(user_sid: &str) -> String {
        format!("D:P(A;;GA;;;SY)(A;;GA;;;{user_sid})")
    }

    impl Drop for PipeSecurity {
        fn drop(&mut self) {
            if !self.descriptor.is_null() {
                unsafe {
                    let _ = LocalFree(self.descriptor);
                }
            }
        }
    }

    struct PipeHandle(HANDLE);

    unsafe impl Send for PipeHandle {}

    impl Drop for PipeHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn encode_wide_null(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn invalid_handle() -> HANDLE {
        HANDLE((-1_isize) as *mut c_void)
    }

    fn is_invalid_handle(handle: HANDLE) -> bool {
        handle.0 == invalid_handle().0
    }

    fn is_null_handle(handle: HANDLE) -> bool {
        handle.0.is_null()
    }

    fn is_retryable_pipe_availability_error(error: u32) -> bool {
        matches!(error, ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
    }

    fn remaining_timeout_ms(
        started: std::time::Instant,
        now: std::time::Instant,
        timeout_ms: u32,
    ) -> Option<u32> {
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= timeout {
            return None;
        }
        let remaining = timeout - elapsed;
        Some(remaining.as_millis().clamp(1, u128::from(u32::MAX)) as u32)
    }

    fn retry_sleep_duration(
        started: std::time::Instant,
        now: std::time::Instant,
        timeout_ms: u32,
        requested: Duration,
    ) -> Option<Duration> {
        let remaining =
            Duration::from_millis(u64::from(remaining_timeout_ms(started, now, timeout_ms)?));
        Some(requested.min(remaining))
    }

    fn last_error(label: &str) -> anyhow::Error {
        win32_error(label, unsafe { GetLastError() })
    }

    fn win32_error(label: &str, code: u32) -> anyhow::Error {
        anyhow::anyhow!(
            "{label}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }

    #[cfg(test)]
    mod tests {
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

            let err = read_pipe_message(&server)
                .expect_err("idle Windows daemon pipe read should time out");

            assert!(started.elapsed() < Duration::from_secs(2));
            let message = format!("{err:#}");
            // The overlapped read receives the total deadline's remaining
            // budget, so setup time can make this 749ms (or slightly less)
            // rather than the configured 750ms.
            assert!(
                message
                    .starts_with("ReadFile Windows daemon control pipe failed: timed out after ")
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
    }
}

#[cfg(test)]
#[path = "ipc_tests.rs"]
mod tests;
