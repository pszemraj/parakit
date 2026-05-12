//! Local control socket for an already-running daemon.

#[cfg(any(unix, target_os = "windows"))]
use anyhow::Context;
use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(any(unix, target_os = "windows"))]
use std::sync::Arc;
#[cfg(any(unix, target_os = "windows"))]
use std::thread;
#[cfg(any(unix, target_os = "windows"))]
use std::thread::JoinHandle;
#[cfg(any(unix, target_os = "windows"))]
use std::time::Duration;

#[cfg(unix)]
use super::preflight;
#[cfg(any(unix, target_os = "windows"))]
use super::{
    inject::{FocusSnapshot, PasteMode},
    logging::Logger,
    notifications::Notifier,
    worker::{insert_text, InsertOutcome},
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
    /// Copy the most recent transcript remembered in memory.
    CopyLast,
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

    #[cfg(any(unix, target_os = "windows", test))]
    fn status(&self) -> IpcResponse {
        let inner = self.inner.lock();
        IpcResponse::Status {
            phase: inner.phase.clone(),
            last_transcript_len: inner.last_transcript.as_ref().map(String::len),
        }
    }

    #[cfg(any(unix, target_os = "windows"))]
    fn last_transcript(&self) -> Option<String> {
        self.inner.lock().last_transcript.clone()
    }

    /// Run a clipboard/insertion transaction while excluding worker and IPC
    /// paste/copy paths.
    ///
    /// Clipboard staging plus a synthetic paste chord must be serialized
    /// process-wide. Otherwise `paste-last`, `copy-last`, or `test-paste` can
    /// race the worker clipboard transaction and paste or copy the wrong text.
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
/// * `state` - Shared daemon status and last-transcript cache.
/// * `paste_mode` - Paste mode used by paste-related commands.
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
    let _ = stream.set_read_timeout(Some(IPC_CLIENT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IPC_CLIENT_TIMEOUT));
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
    serde_json::from_str(&line).context("invalid control command")
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
        IpcCommand::PasteLast => {
            let result = state.with_insertion_lock(|| {
                let text = state
                    .last_transcript()
                    .context("no transcript has been captured in this daemon session")?;
                paste_text(&text, paste_mode, keep_transcript_clipboard, log, notifier)
            })?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: match result {
                        InsertOutcome::Pasted => "pasted last transcript",
                        InsertOutcome::CopiedOnly => "copied last transcript",
                        InsertOutcome::Blocked => "paste blocked",
                        InsertOutcome::Skipped => "paste skipped",
                    }
                    .to_string(),
                },
                stop_after_response: false,
            })
        }
        IpcCommand::CopyLast => {
            state.with_insertion_lock(|| {
                let text = state
                    .last_transcript()
                    .context("no transcript has been captured in this daemon session")?;
                copy_text(&text)
            })?;
            Ok(CommandOutcome {
                response: IpcResponse::Ok {
                    message: "copied last transcript".to_string(),
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
    insert_text(
        &mut injector,
        text,
        paste_mode,
        keep_transcript_clipboard,
        focus.as_ref(),
        (log, notifier),
        false,
    )
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
    use std::{ffi::c_void, ptr::null_mut};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};

    const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
    const IPC_CLIENT_TIMEOUT_MS: u32 = 750;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const PIPE_TYPE_MESSAGE: u32 = 0x0000_0004;
    const PIPE_READMODE_MESSAGE: u32 = 0x0000_0002;
    const PIPE_WAIT: u32 = 0x0000_0000;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_MORE_DATA: u32 = 234;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_SEM_TIMEOUT: u32 = 121;
    const SDDL_REVISION_1: u32 = 1;
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
        fn ConnectNamedPipe(pipe: HANDLE, overlapped: *mut c_void) -> i32;
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
        fn DisconnectNamedPipe(pipe: HANDLE) -> i32;
        fn FlushFileBuffers(file: HANDLE) -> i32;
        fn GetLastError() -> u32;
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
        fn ReadFile(
            file: HANDLE,
            buffer: *mut c_void,
            bytes_to_read: u32,
            bytes_read: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
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
    /// * `state` - Shared daemon status and last-transcript cache.
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
        let pipe_name = daemon_pipe_name()?;
        let thread = thread::Builder::new()
            .name("parakit-ipc".into())
            .spawn(move || loop {
                match create_server_pipe(&pipe_name).and_then(|pipe| {
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
        let pipe_name = daemon_pipe_name()?;
        let pipe = connect_client_pipe(&pipe_name)?;
        write_json_line(&pipe, &command).context("write Windows daemon control command")?;
        let response = read_pipe_message(&pipe).context("read Windows daemon control response")?;
        serde_json::from_slice(&response).context("parse Windows daemon control response")
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

        if let Err(err) = write_json_line(&pipe, &outcome.response) {
            log.warn(format!("Windows daemon control response failed: {err:#}"));
        }
        unsafe {
            let _ = FlushFileBuffers(pipe.0);
            let _ = DisconnectNamedPipe(pipe.0);
        }

        if outcome.stop_after_response {
            schedule_exit_after_response(None);
        }
    }

    fn read_command(pipe: &PipeHandle) -> Result<IpcCommand> {
        let bytes =
            read_pipe_message(pipe).context("read Windows daemon control command failed")?;
        serde_json::from_slice(&bytes).context("invalid Windows daemon control command")
    }

    fn daemon_pipe_name() -> Result<Vec<u16>> {
        let sid = super::super::windows_security::current_user_sid_string()
            .context("read current Windows user SID for daemon pipe")?;
        Ok(encode_wide_null(&format!(r"\\.\pipe\parakit-daemon-{sid}")))
    }

    fn create_server_pipe(pipe_name: &[u16]) -> Result<PipeHandle> {
        let mut security = PipeSecurity::current_user_only()?;
        let mut attributes = security.attributes();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(pipe_name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
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

    fn connect_server_pipe(pipe: &PipeHandle) -> Result<()> {
        if unsafe { ConnectNamedPipe(pipe.0, null_mut()) } != 0 {
            return Ok(());
        }
        let err = unsafe { GetLastError() };
        if err == ERROR_PIPE_CONNECTED {
            Ok(())
        } else {
            Err(win32_error("ConnectNamedPipe failed", err))
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
                    FILE_ATTRIBUTE_NORMAL,
                    HANDLE::default(),
                )
            };
            if !is_invalid_handle(handle) {
                return Ok(PipeHandle(handle));
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

    fn read_pipe_message(pipe: &PipeHandle) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let mut chunk = vec![0_u8; PIPE_BUFFER_SIZE as usize];
            let mut read = 0_u32;
            let ok = unsafe {
                ReadFile(
                    pipe.0,
                    chunk.as_mut_ptr().cast(),
                    PIPE_BUFFER_SIZE,
                    &mut read,
                    null_mut(),
                )
            };
            out.extend_from_slice(&chunk[..read as usize]);
            if ok != 0 {
                return Ok(out);
            }
            let err = unsafe { GetLastError() };
            if err != ERROR_MORE_DATA {
                return Err(win32_error(
                    "ReadFile Windows daemon control pipe failed",
                    err,
                ));
            }
        }
    }

    fn write_json_line<T: Serialize>(pipe: &PipeHandle, value: &T) -> Result<()> {
        let mut bytes =
            serde_json::to_vec(value).context("serialize Windows daemon control JSON")?;
        bytes.push(b'\n');
        write_pipe_all(pipe, &bytes)
    }

    fn write_pipe_all(pipe: &PipeHandle, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            let chunk_len = bytes.len().min(PIPE_BUFFER_SIZE as usize);
            let mut written = 0_u32;
            let ok = unsafe {
                WriteFile(
                    pipe.0,
                    bytes.as_ptr().cast(),
                    chunk_len as u32,
                    &mut written,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_error("WriteFile Windows daemon control pipe failed"));
            }
            if written == 0 {
                bail!("WriteFile Windows daemon control pipe wrote zero bytes");
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    struct PipeSecurity {
        descriptor: *mut c_void,
    }

    impl PipeSecurity {
        fn current_user_only() -> Result<Self> {
            let sid = super::super::windows_security::current_user_sid_string()
                .context("read current Windows user SID for daemon pipe security")?;
            let sddl = encode_wide_null(&current_user_only_pipe_sddl(&sid));
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
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
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
            let log = Arc::new(Logger::new(LogLevel::Quiet));
            handle_client(server, &state, PasteMode::Terminal, false, log);
        });
        handler.join().expect("handler should return after timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
