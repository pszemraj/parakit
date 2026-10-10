//! Windows named-pipe transport for the daemon control protocol.

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
    let bytes = read_pipe_message(pipe).context("read Windows daemon control command failed")?;
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
                let Some(wait_ms) =
                    remaining_timeout_ms(started, std::time::Instant::now(), IPC_CLIENT_TIMEOUT_MS)
                else {
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

fn read_pipe_chunk(pipe: &PipeHandle, chunk: &mut [u8], timeout_ms: u32) -> Result<PipeReadChunk> {
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
    if unsafe { GetOverlappedResult(pipe.0, overlapped.as_mut_ptr(), &mut transferred, wait) } != 0
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
    if unsafe { GetOverlappedResult(pipe.0, overlapped.as_mut_ptr(), &mut transferred, wait) } != 0
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
#[path = "windows_pipe_tests.rs"]
mod tests;
