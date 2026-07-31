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
use std::io::{BufRead, BufReader, Write};
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

#[cfg(unix)]
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

/// Default number of transcripts kept in daemon memory when
/// `daemon.transcript_history` is unset.
pub(crate) const DEFAULT_TRANSCRIPT_HISTORY: usize = 10;

/// Command sent by helper subcommands to the running daemon.
///
/// `CopyLast` changed from a unit variant to a struct variant carrying an
/// `index` when transcript history became configurable. This is a
/// deliberate wire-format break: a client built after this change talking
/// to a daemon started by an older build (or vice versa, after an in-place
/// binary upgrade that left the old daemon running) can't exchange this
/// command. History depth is read once at startup, so there is no way to
/// bridge the two wire formats; restarting the daemon is the only fix.
/// Neither direction is left to surface a raw serde error: `parse_command`
/// recognizes the legacy bare-string encoding on the daemon side and replies
/// with the restart hint through the existing `IpcResponse::Err` path (old
/// clients can still render that, since its shape hasn't changed), and
/// `send_command`'s response parsing adds the same hint on the client side
/// for `CopyLast` specifically.
///
/// `PasteLast` was removed: run from a terminal, it pasted into the terminal
/// itself rather than wherever the caller meant to paste, so `copy-last`'s
/// copy-then-paste-manually flow is the one that stays. A CLI built before
/// the removal can still send the legacy bare `"paste_last"` string (see
/// [`LEGACY_PASTE_LAST_WIRE`]); `parse_command` answers that with a
/// "removed, use copy-last" message instead of a raw serde error.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IpcCommand {
    /// Return current daemon state.
    Status,
    /// Stop the daemon process.
    Stop,
    /// Copy a transcript remembered in memory.
    ///
    /// `index` is 0-based on the wire (0 is the most recent); a missing key
    /// defaults to 0. The CLI number is 1-based and is converted to this
    /// 0-based wire index at the CLI boundary.
    CopyLast {
        #[serde(default)]
        index: usize,
    },
    /// List transcripts remembered in memory, newest first.
    History {
        /// Cap the number of entries returned. `None` returns all of them.
        #[serde(default)]
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

/// Bare-string wire encoding used by the removed `paste_last` command and by
/// `copy_last` before it gained an `index` field. A CLI built before the
/// respective change still sends one of these two literals.
#[cfg(any(unix, target_os = "windows"))]
const LEGACY_PASTE_LAST_WIRE: &str = "\"paste_last\"";
#[cfg(any(unix, target_os = "windows"))]
const LEGACY_COPY_LAST_WIRE: &str = "\"copy_last\"";

/// Deserialize one control-socket request line as [`IpcCommand`].
///
/// Detects two legacy bare-string (pre-`index` unit-variant) encodings and
/// replaces the resulting serde error with an actionable one, instead of
/// surfacing raw serde text (e.g. "unknown variant `paste_last`") to
/// whoever reads `IpcResponse::Err`:
/// - `"paste_last"` only comes from a CLI old enough to predate the
///   command's removal; the daemon has no handler for it at all anymore, so
///   the message says it was removed and points at `copy-last`.
/// - `"copy_last"` comes from a CLI built before `copy-last` gained an
///   `index`; the message names the CLI/daemon version mismatch and the
///   restart fix.
///
/// # Errors
///
/// Returns an error naming the removed command or the CLI/daemon version
/// mismatch when `raw` is one of the two legacy literals, otherwise the
/// underlying serde error wrapped with context.
#[cfg(any(unix, target_os = "windows"))]
fn parse_command(raw: &str) -> Result<IpcCommand> {
    serde_json::from_str(raw).or_else(|err| match raw.trim() {
        LEGACY_PASTE_LAST_WIRE => bail!(
            "paste-last was removed from parakit: run from a terminal, it pasted into the \
             terminal itself, so it never did what it looked like it should. Use `parakit \
             copy-last` instead (copies the transcript to the clipboard; paste it yourself)."
        ),
        LEGACY_COPY_LAST_WIRE => bail!(
            "this daemon is newer than the CLI that sent this command: copy-last gained a \
             transcript index and can no longer be sent as a bare command. Reinstall or upgrade \
             the parakit CLI to match this daemon, or restart the daemon to go back to matching \
             an older CLI."
        ),
        _ => Err(err).context("invalid control command"),
    })
}

/// Extra context for a control-socket response parse failure, naming the
/// probable cause when `command` is `CopyLast`: it is the only command with
/// a wire-format break (see the `IpcCommand` doc comment), so a response
/// that fails to parse most likely means the running daemon predates this
/// CLI build. `Status` and `History` are additive-compatible (new fields
/// are `#[serde(default)]`; new variants are never sent by an old daemon)
/// and don't need this hint.
///
/// # Returns
///
/// `Some` context message for `CopyLast`, `None` otherwise.
#[cfg(any(unix, target_os = "windows"))]
fn stale_daemon_response_hint(command: &IpcCommand) -> Option<&'static str> {
    match command {
        IpcCommand::CopyLast { .. } => Some(
            "parse daemon control response: the running daemon predates this CLI's \
             copy-last wire format; run `parakit stop` and start it again",
        ),
        IpcCommand::Status
        | IpcCommand::Stop
        | IpcCommand::History { .. }
        | IpcCommand::TestPaste { .. } => None,
    }
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
        /// Extended runtime detail for `--verbose status`. `#[serde(default)]`
        /// so a reply from an older daemon build without this key still
        /// deserializes; `None` also covers the startup window before
        /// [`SharedState::set_info`] has run. Boxed because `StatusDetail` is
        /// much larger than the other `IpcResponse` variants, and `Status` is
        /// otherwise mostly `None` (serde transparently (de)serializes
        /// `Box<T>` as `T`, so the wire format is unaffected).
        #[serde(default)]
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
    /// `daemon.transcript_history = 0`. `#[serde(default)]` so a reply from
    /// an older daemon build without this key still deserializes.
    #[serde(default)]
    pub(crate) history: Option<String>,
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
            inner: Mutex::new(StateSnapshot {
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
                history: Some(self.history_label(inner.history.len())),
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
    /// # Errors
    ///
    /// Returns an error when `history_limit` is 0, no transcript has been
    /// remembered yet, or `index` is past the end of what is remembered.
    #[cfg(any(unix, target_os = "windows"))]
    fn resolve_transcript(&self, index: usize) -> Result<String> {
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
) -> Result<IpcServer> {
    spawn_server_impl(state, paste_mode, keep_transcript_clipboard, log)
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
/// Returns an error when no daemon is listening or the daemon reports failure.
pub(crate) fn run_client(command: IpcCommand, quiet: bool, verbose: bool) -> Result<()> {
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
            detail,
        } => {
            if !quiet {
                // These two lines must stay byte-identical to the pre-verbose
                // output: existing scripts parse them.
                println!("parakit: {phase}");
                match last_transcript_len {
                    Some(len) => println!("last transcript: {len} bytes"),
                    None => println!("last transcript: none"),
                }
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
///   starting up) or predates this field.
fn print_status_detail(detail: Option<&StatusDetail>) {
    let Some(detail) = detail else {
        println!("  detail unavailable (daemon starting or older version)");
        return;
    };
    println!("  pid:        {}", detail.pid);
    println!("  uptime:     {}", format_uptime(detail.uptime_secs));
    println!("  dictations: {}", detail.dictation_count);
    println!("  mic:        {}", detail.mic);
    println!("  model:      {} ({})", detail.model, detail.dtype);
    println!(
        "  device:     {} ({}, {} threads)",
        detail.device, detail.backend, detail.threads
    );
    println!("  paste mode: {}", detail.paste_mode);
    println!("  sounds:     {}", if detail.sounds { "on" } else { "off" });
    println!("  cleaning:   {}", detail.cleaning);
    println!("  logging:    {}", detail.log.as_deref().unwrap_or("off"));
    if let Some(history) = &detail.history {
        println!("  history:    {history}");
    }
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
    const MAX_CHARS: usize = 72;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CHARS {
        return collapsed;
    }
    let mut truncated: String = collapsed.chars().take(MAX_CHARS).collect();
    truncated.push('…');
    truncated
}

#[cfg(unix)]
fn spawn_server_impl(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
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
                            .spawn(move || {
                                handle_client(
                                    stream,
                                    &state,
                                    paste_mode,
                                    keep_transcript_clipboard,
                                    log,
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
) {
    let notifier = Notifier::new(Arc::clone(&log));
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
        schedule_exit_after_response(preflight::control_socket_path().ok());
    }
}

#[cfg(unix)]
fn read_command(stream: &std::os::unix::net::UnixStream) -> Result<IpcCommand> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("read control command failed")?;
    parse_command(&line)
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
fn schedule_exit_after_response(cleanup_path: Option<std::path::PathBuf>) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        if let Some(path) = cleanup_path {
            let _ = std::fs::remove_file(path);
        }
        std::process::exit(0);
    });
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
        false,
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
fn send_command(command: IpcCommand) -> Result<IpcResponse> {
    use std::os::unix::net::UnixStream;

    let response_timeout = command.response_timeout();
    let path = preflight::control_socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect daemon control socket {}", path.display()))?;
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
    serde_json::from_str(&line).with_context(|| {
        stale_daemon_response_hint(&command).unwrap_or("parse daemon control response")
    })
}

#[cfg(target_os = "windows")]
fn spawn_server_impl(
    state: Arc<SharedState>,
    paste_mode: PasteMode,
    keep_transcript_clipboard: bool,
    log: Arc<Logger>,
) -> Result<IpcServer> {
    windows_pipe::spawn_server_impl(state, paste_mode, keep_transcript_clipboard, log)
}

#[cfg(target_os = "windows")]
fn send_command(command: IpcCommand) -> Result<IpcResponse> {
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
                        let _ = thread::Builder::new()
                            .name("parakit-ipc-client".into())
                            .spawn(move || {
                                handle_client(
                                    pipe,
                                    state,
                                    paste_mode,
                                    keep_transcript_clipboard,
                                    log,
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
    pub(super) fn send_command(command: IpcCommand) -> Result<IpcResponse> {
        let response_timeout_ms = command
            .response_timeout()
            .as_millis()
            .clamp(1, u128::from(u32::MAX)) as u32;
        let identity = DaemonPipeIdentity::current()?;
        let pipe = connect_client_pipe(&identity.pipe_name)?;
        write_json_message(&pipe, &command).context("write Windows daemon control command")?;
        let response = read_pipe_message_with_timeout(&pipe, response_timeout_ms)
            .context("read Windows daemon control response")?;
        serde_json::from_slice(&response).with_context(|| {
            stale_daemon_response_hint(&command).unwrap_or("parse Windows daemon control response")
        })
    }

    fn handle_client(
        pipe: PipeHandle,
        state: Arc<SharedState>,
        paste_mode: PasteMode,
        keep_transcript_clipboard: bool,
        log: Arc<Logger>,
    ) {
        let notifier = Notifier::new(Arc::clone(&log));
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
            schedule_exit_after_response(None);
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
                        return Err(win32_error(
                            "CreateFileW Windows daemon control pipe failed",
                            err,
                        ));
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
            match read_pipe_chunk(pipe, &mut chunk, remaining_ms)? {
                PipeReadChunk::Complete(read) => {
                    message.extend_from_slice(&chunk[..read]);
                    return Ok(message);
                }
                PipeReadChunk::MoreData(read) => {
                    message.extend_from_slice(&chunk[..read]);
                }
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
        fn pipe_message_larger_than_transport_buffer_round_trips() -> Result<()> {
            let (server, client) = connected_test_pipe(true)?;
            let payload: Vec<u8> = (0..PIPE_BUFFER_SIZE as usize + 1024)
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
        fn pipe_message_read_times_out_when_peer_sends_nothing() -> Result<()> {
            let (server, _client) = connected_test_pipe(true)?;
            let started = std::time::Instant::now();

            let err = read_pipe_message(&server)
                .expect_err("idle Windows daemon pipe read should time out");

            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(format!("{err:#}").contains("timed out after 750ms"));
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
mod tests {
    use super::*;

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn ipc_command_response_timeout_and_stale_hint_matrix() {
        /// Expected `IpcCommand::response_timeout()` for `command`.
        ///
        /// Exhaustive with no wildcard arm: a new `IpcCommand` variant fails
        /// compilation here until this function states its response-timeout
        /// budget. This also closes a real coverage gap: `Stop` and
        /// `History` were never asserted before this matrix.
        fn expected_response_timeout(command: &IpcCommand) -> Duration {
            match command {
                IpcCommand::CopyLast { .. } | IpcCommand::TestPaste { .. } => {
                    IPC_INSERT_RESPONSE_TIMEOUT
                }
                IpcCommand::Status | IpcCommand::Stop | IpcCommand::History { .. } => {
                    IPC_TRANSPORT_TIMEOUT
                }
            }
        }

        /// Expected `stale_daemon_response_hint()` substrings for `command`.
        ///
        /// Exhaustive with no wildcard arm, mirroring
        /// `expected_response_timeout`.
        fn expected_stale_hint_substrings(command: &IpcCommand) -> Option<&'static [&'static str]> {
            match command {
                IpcCommand::CopyLast { .. } => Some(&["parakit stop", "predates this CLI"]),
                // Status/History are additive-compatible (see the doc
                // comment on `stale_daemon_response_hint`) and must not get
                // this wording.
                IpcCommand::Status
                | IpcCommand::Stop
                | IpcCommand::History { .. }
                | IpcCommand::TestPaste { .. } => None,
            }
        }

        let commands = [
            IpcCommand::Status,
            IpcCommand::Stop,
            IpcCommand::History { limit: None },
            IpcCommand::CopyLast { index: 0 },
            IpcCommand::TestPaste {
                text: "test".to_string(),
            },
        ];

        let failures: Vec<String> = commands
            .iter()
            .filter_map(|command| {
                let expected_timeout = expected_response_timeout(command);
                let actual_timeout = command.response_timeout();
                if actual_timeout != expected_timeout {
                    return Some(format!(
                        "{command:?} timeout: expected {expected_timeout:?}, got {actual_timeout:?}"
                    ));
                }

                let hint = stale_daemon_response_hint(command);
                match (expected_stale_hint_substrings(command), hint) {
                    (None, None) => {}
                    (None, Some(hint)) => {
                        return Some(format!(
                            "{command:?} hint: expected None, got Some({hint:?})"
                        ));
                    }
                    (Some(_), None) => {
                        return Some(format!("{command:?} hint: expected Some(..), got None"));
                    }
                    (Some(substrings), Some(hint)) => {
                        for substring in substrings {
                            if !hint.contains(substring) {
                                return Some(format!(
                                    "{command:?} hint {hint:?}: missing substring {substring:?}"
                                ));
                            }
                        }
                    }
                }

                None
            })
            .collect();

        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn insertion_response_timeout_covers_queued_macos_paste_transaction() {
        let acknowledgement_budget = std::cmp::max(
            crate::daemon::macos::pasteboard::AX_CONFIRM_DEADLINE,
            crate::daemon::macos::pasteboard::UNVERIFIED_GRACE,
        );
        let transaction_wait_budget = crate::daemon::macos::PASTE_MODIFIER_RELEASE_TIMEOUT
            + crate::daemon::desktop::inject::MACOS_CLIPBOARD_SETTLE_DELAY
            + acknowledgement_budget;
        let queued_then_own_transaction_budget = transaction_wait_budget + transaction_wait_budget;

        assert!(
            IPC_INSERT_RESPONSE_TIMEOUT > queued_then_own_transaction_budget,
            "IPC insertion response timeout must cover one queued macOS paste transaction \
             plus the command's own transaction"
        );
    }

    #[test]
    fn shared_state_reports_phase_and_newest_transcript_byte_length() {
        let state = SharedState::new();
        state.set_phase("recording");
        state.remember_transcript("hi".to_string());
        state.remember_transcript("hello world".to_string());

        assert!(matches!(
            state.status(),
            IpcResponse::Status {
                phase,
                last_transcript_len: Some(11),
                detail: None,
            } if phase == "recording"
        ));
    }

    #[test]
    fn history_snapshot_matrix() {
        struct Case {
            name: &'static str,
            history_limit: usize,
            remembered: &'static [&'static str],
            snapshot_limit: Option<usize>,
            expect: &'static [(usize, &'static str)],
        }

        let cases = [
            Case {
                name: "evicts oldest beyond history limit",
                history_limit: 2,
                remembered: &["first", "second", "third"],
                snapshot_limit: None,
                expect: &[(1, "third"), (2, "second")],
            },
            Case {
                name: "history limit zero remembers nothing",
                history_limit: 0,
                remembered: &["should not be kept"],
                snapshot_limit: None,
                expect: &[],
            },
            Case {
                name: "orders newest first with one-based index",
                history_limit: DEFAULT_TRANSCRIPT_HISTORY,
                remembered: &["oldest", "middle", "newest"],
                snapshot_limit: None,
                expect: &[(1, "newest"), (2, "middle"), (3, "oldest")],
            },
            Case {
                name: "respects snapshot limit",
                history_limit: DEFAULT_TRANSCRIPT_HISTORY,
                remembered: &["one", "two", "three"],
                snapshot_limit: Some(2),
                expect: &[(1, "three"), (2, "two")],
            },
        ];

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let state = SharedState::with_history_limit(case.history_limit);
                for text in case.remembered {
                    state.remember_transcript((*text).to_string());
                }
                let entries = state.history_snapshot(case.snapshot_limit);
                let actual: Vec<(usize, &str)> = entries
                    .iter()
                    .map(|entry| (entry.index, entry.preview.as_str()))
                    .collect();
                (actual.as_slice() != case.expect).then(|| {
                    format!(
                        "{}: expected {:?}, got {:?}",
                        case.name, case.expect, actual
                    )
                })
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn history_ref_label_reproduces_last_transcript_wording_for_index_zero() {
        // Index 0 must keep the pre-history wording byte-identical, since
        // `copy-last` with no `N` argument is the common case and existing
        // docs/scripts quote this exact phrase.
        assert_eq!(history_ref_label(0), "last transcript");
        assert_eq!(history_ref_label(1), "transcript 2");
        assert_eq!(history_ref_label(4), "transcript 5");
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn resolve_transcript_matrix() {
        struct Case {
            name: &'static str,
            /// `None` uses `SharedState::new()`'s default history limit.
            history_limit: Option<usize>,
            remembered: &'static [&'static str],
            index: usize,
            expect: Result<&'static str, &'static str>,
        }

        let cases = [
            Case {
                name: "history disabled reported first",
                history_limit: Some(0),
                remembered: &[],
                index: 0,
                expect: Err("transcript history is disabled (daemon.transcript_history = 0)"),
            },
            Case {
                name: "empty history when enabled but unused",
                history_limit: None,
                remembered: &[],
                index: 0,
                expect: Err("no transcript has been captured in this daemon session"),
            },
            Case {
                name: "out of range index singular",
                history_limit: None,
                remembered: &["only one"],
                index: 1,
                expect: Err("only 1 transcript remembered in this daemon session"),
            },
            Case {
                name: "out of range index plural",
                history_limit: None,
                remembered: &["only one", "second"],
                index: 5,
                expect: Err("only 2 transcripts remembered in this daemon session"),
            },
            Case {
                name: "in range index zero returns newest",
                history_limit: None,
                remembered: &["oldest", "newest"],
                index: 0,
                expect: Ok("newest"),
            },
            Case {
                name: "in range index one returns oldest",
                history_limit: None,
                remembered: &["oldest", "newest"],
                index: 1,
                expect: Ok("oldest"),
            },
        ];

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let state = match case.history_limit {
                    Some(limit) => SharedState::with_history_limit(limit),
                    None => SharedState::new(),
                };
                for text in case.remembered {
                    state.remember_transcript((*text).to_string());
                }
                let actual = state
                    .resolve_transcript(case.index)
                    .map_err(|err| err.to_string());
                let expect_owned: Result<String, String> =
                    case.expect.map(str::to_string).map_err(str::to_string);
                (actual != expect_owned).then(|| {
                    format!(
                        "{}: expected {:?}, got {:?}",
                        case.name, expect_owned, actual
                    )
                })
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn ensure_history_enabled_separates_disabled_from_empty() {
        // `history` must not report a disabled ring as merely empty: the
        // two states need different fixes from the user.
        assert_eq!(
            SharedState::with_history_limit(0)
                .ensure_history_enabled()
                .unwrap_err()
                .to_string(),
            "transcript history is disabled (daemon.transcript_history = 0)"
        );
        assert!(SharedState::new().ensure_history_enabled().is_ok());
    }

    #[test]
    fn preview_text_collapses_whitespace_and_trims() {
        let collapsed = preview_text("  line one\n  line two\t\tline three  ");
        assert_eq!(collapsed, "line one line two line three");
    }

    #[test]
    fn preview_text_truncates_long_text_on_a_char_boundary() {
        // 'e' with combining characters would risk a byte-boundary panic if
        // truncation were byte-based instead of char-based; a plain
        // multi-byte codepoint repeated past the 72-char cap is enough to
        // catch that regression.
        let long = "é".repeat(80);

        let preview = preview_text(&long);

        assert_eq!(preview.chars().count(), 73); // 72 chars + '…'
        assert!(preview.ends_with('…'));
        assert!(preview.starts_with(&"é".repeat(72)));
    }

    fn sample_daemon_info() -> DaemonInfo {
        DaemonInfo {
            pid: 4242,
            model_name: "parakeet-tdt-0.6b-v3-Q8_0.gguf".to_string(),
            dtype: "Q8_0 (745 MB)".to_string(),
            mic_summary: "Test Mic, 48000 Hz mono input -> 16000 Hz mono model, F32".to_string(),
            backend: "CPU".to_string(),
            device: "cpu".to_string(),
            threads: 8,
            paste_mode: "standard",
            cleaning_summary: "on (12 rules)".to_string(),
            sounds_on: true,
            log_summary: Some("jsonl to /home/user/.parakit/logs".to_string()),
            hotkey_backend_label: Some("auto"),
        }
    }

    #[test]
    fn shared_state_status_is_bare_until_info_is_set() {
        let state = SharedState::new();

        let IpcResponse::Status { detail, .. } = state.status() else {
            panic!("expected a Status response");
        };
        assert!(detail.is_none());
    }

    #[test]
    fn shared_state_set_info_and_record_dictation_populate_status_detail() {
        let state = SharedState::new();
        state.set_info(sample_daemon_info());
        state.record_dictation();
        state.record_dictation();

        let IpcResponse::Status { detail, .. } = state.status() else {
            panic!("expected a Status response");
        };
        let detail = detail.expect("detail should be set after set_info");
        assert_eq!(detail.pid, 4242);
        assert_eq!(detail.dictation_count, 2);
        assert_eq!(detail.model, "parakeet-tdt-0.6b-v3-Q8_0.gguf");
        assert_eq!(detail.dtype, "Q8_0 (745 MB)");
        assert_eq!(detail.backend, "CPU");
        assert_eq!(detail.device, "cpu");
        assert_eq!(detail.threads, 8);
        assert_eq!(detail.paste_mode, "standard");
        assert!(detail.sounds);
        assert_eq!(detail.cleaning, "on (12 rules)");
        assert_eq!(
            detail.log.as_deref(),
            Some("jsonl to /home/user/.parakit/logs")
        );
        assert_eq!(detail.hotkey_backend.as_deref(), Some("auto"));
    }

    #[test]
    fn status_detail_serde_round_trips() {
        let detail = StatusDetail {
            pid: 1234,
            uptime_secs: 5025,
            dictation_count: 7,
            mic: "Test Mic".to_string(),
            model: "model.gguf".to_string(),
            dtype: "Q8_0".to_string(),
            device: "cpu".to_string(),
            backend: "CPU".to_string(),
            threads: 4,
            paste_mode: "standard".to_string(),
            sounds: true,
            cleaning: "off".to_string(),
            log: None,
            hotkey_backend: None,
            history: Some("3 of 10".to_string()),
        };
        let response = IpcResponse::Status {
            phase: "idle".to_string(),
            last_transcript_len: Some(12),
            detail: Some(Box::new(detail.clone())),
        };

        let json = serde_json::to_string(&response).expect("status response should serialize");
        let round_tripped: IpcResponse =
            serde_json::from_str(&json).expect("status response should deserialize");

        assert!(matches!(
            round_tripped,
            IpcResponse::Status {
                detail: Some(d),
                ..
            } if *d == detail
        ));
    }

    #[test]
    fn status_response_without_detail_key_deserializes_as_none() {
        // Simulates a reply from a daemon build that predates the `detail`
        // field: the wire payload simply omits the key. `#[serde(default)]`
        // must make this parse instead of failing.
        let json = r#"{"status":{"phase":"idle","last_transcript_len":null}}"#;

        let response: IpcResponse =
            serde_json::from_str(json).expect("older status payload should still deserialize");

        assert!(matches!(
            response,
            IpcResponse::Status {
                phase,
                last_transcript_len: None,
                detail: None,
            } if phase == "idle"
        ));
    }

    #[test]
    fn status_detail_without_history_key_deserializes_as_none() {
        // Simulates a reply from a daemon build that predates transcript
        // history depth reporting: the wire payload omits the `history`
        // key. `#[serde(default)]` must make this parse instead of failing.
        let json = r#"{"pid":1,"uptime_secs":0,"dictation_count":0,"mic":"m","model":"m","dtype":"d","device":"d","backend":"b","threads":1,"paste_mode":"standard","sounds":true,"cleaning":"off","log":null,"hotkey_backend":null}"#;

        let detail: StatusDetail = serde_json::from_str(json)
            .expect("older status detail payload should still deserialize");

        assert!(detail.history.is_none());
    }

    #[test]
    fn ipc_command_copy_last_serde_round_trips_with_index() {
        let command = IpcCommand::CopyLast { index: 1 };
        let json = serde_json::to_string(&command).expect("copy_last should serialize");
        assert_eq!(json, r#"{"copy_last":{"index":1}}"#);
        let round_tripped: IpcCommand =
            serde_json::from_str(&json).expect("copy_last should deserialize");
        assert!(matches!(round_tripped, IpcCommand::CopyLast { index: 1 }));
    }

    #[test]
    fn ipc_command_copy_last_missing_index_defaults_to_zero() {
        let command: IpcCommand = serde_json::from_str(r#"{"copy_last":{}}"#)
            .expect("copy_last with no index should parse");

        assert!(matches!(command, IpcCommand::CopyLast { index: 0 }));
    }

    #[test]
    fn ipc_command_history_serde_round_trips_with_and_without_limit() {
        let command: IpcCommand =
            serde_json::from_str(r#"{"history":{}}"#).expect("history with no limit should parse");
        assert!(matches!(command, IpcCommand::History { limit: None }));

        let command = IpcCommand::History { limit: Some(5) };
        let json = serde_json::to_string(&command).expect("history should serialize");
        let round_tripped: IpcCommand =
            serde_json::from_str(&json).expect("history should deserialize");
        assert!(matches!(
            round_tripped,
            IpcCommand::History { limit: Some(5) }
        ));
    }

    #[test]
    fn ipc_response_history_serde_round_trips() {
        let response = IpcResponse::History {
            entries: vec![HistoryEntry {
                index: 1,
                age_secs: 12,
                chars: 47,
                preview: "the quick brown fox".to_string(),
            }],
        };

        let json = serde_json::to_string(&response).expect("history response should serialize");
        let round_tripped: IpcResponse =
            serde_json::from_str(&json).expect("history response should deserialize");

        assert!(matches!(
            round_tripped,
            IpcResponse::History { entries } if entries.len() == 1 && entries[0].preview == "the quick brown fox"
        ));
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn partial_client_command_times_out() {
        use super::super::logging::{LogLevel, Logger};
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
            handle_client(server, &state, PasteMode::Terminal, false, log);
        });
        handler.join().expect("handler should return after timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn parse_command_classifies_legacy_paste_last_as_removed() {
        let err = parse_command("\"paste_last\"\n").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("paste-last was removed"), "{message}");
        assert!(message.contains("parakit copy-last"), "{message}");
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn parse_command_classifies_legacy_copy_last_as_a_version_mismatch() {
        let err = parse_command("\"copy_last\"").unwrap_err();
        assert!(err.to_string().contains("newer than the CLI"));
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn parse_command_keeps_the_generic_context_for_unrelated_garbage() {
        // Guards against the legacy-encoding detection over-firing: any
        // other unparseable request keeps the original generic message.
        let err = parse_command("not json").unwrap_err();
        assert_eq!(err.to_string(), "invalid control command");
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn parse_command_still_accepts_the_current_wire_format() {
        let command = parse_command(r#"{"copy_last":{"index":2}}"#)
            .expect("current copy_last encoding should still parse");
        assert!(matches!(command, IpcCommand::CopyLast { index: 2 }));
    }
}
