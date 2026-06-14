//! Windows clipboard-history notification tracking.

#![cfg(target_os = "windows")]

use anyhow::{Context, Result};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::{w, Error as WinError};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetMessageW, PostMessageW, RegisterClassW,
    HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WNDCLASSW,
};

const WM_PARAKIT_CLIPBOARD_LISTENER_STOP: u32 = WM_APP + 0x504b;

type StartupResult = std::result::Result<isize, String>;

/// Persistent clipboard listener owned by the Windows injector.
pub(crate) struct ClipboardHistoryListener {
    handle: ClipboardHistoryHandle,
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardHistoryListener {
    /// Start a hidden message-only clipboard listener.
    ///
    /// # Returns
    ///
    /// A listener whose handle can wait for clipboard sequence notifications.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener window cannot be created or registered
    /// for clipboard update messages.
    pub(crate) fn start() -> Result<Self> {
        let state = Arc::new(ListenerState::new(current_sequence_number()));
        let handle = ClipboardHistoryHandle {
            state: Arc::clone(&state),
        };
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("parakit-clipboard-history".to_string())
            .spawn(move || listener_thread_main(state, ready_tx))
            .context("spawn Windows clipboard-history listener thread")?;

        match ready_rx
            .recv()
            .context("Windows clipboard-history listener thread exited before startup")?
        {
            Ok(hwnd) => Ok(Self {
                handle,
                hwnd,
                thread: Some(thread),
            }),
            Err(message) => {
                let _ = thread.join();
                anyhow::bail!("{message}");
            }
        }
    }

    /// Return a wait handle for clipboard update notifications.
    ///
    /// # Returns
    ///
    /// A cloneable handle that does not own the listener thread.
    pub(crate) fn handle(&self) -> ClipboardHistoryHandle {
        self.handle.clone()
    }
}

impl Drop for ClipboardHistoryListener {
    fn drop(&mut self) {
        // SAFETY: The HWND belongs to the listener thread and is only used to
        // post a private shutdown message. The thread destroys the window.
        unsafe {
            let _ = PostMessageW(
                Some(hwnd_from_raw(self.hwnd)),
                WM_PARAKIT_CLIPBOARD_LISTENER_STOP,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Cloneable handle used by the paste path to wait for one clipboard write.
#[derive(Clone)]
pub(crate) struct ClipboardHistoryHandle {
    state: Arc<ListenerState>,
}

impl ClipboardHistoryHandle {
    /// Return the current Windows clipboard sequence number.
    ///
    /// # Returns
    ///
    /// The current sequence number from `GetClipboardSequenceNumber`.
    pub(crate) fn current_sequence(&self) -> u32 {
        current_sequence_number()
    }

    /// Wait until this listener observes at least `target_sequence`.
    ///
    /// # Arguments
    ///
    /// * `target_sequence` - Clipboard sequence number for the transcript
    ///   write.
    /// * `timeout` - Maximum time to wait for the listener notification.
    ///
    /// # Returns
    ///
    /// `true` when the listener saw the sequence before `timeout`, otherwise
    /// `false`.
    pub(crate) fn wait_for_sequence(&self, target_sequence: u32, timeout: Duration) -> bool {
        self.state.wait_for_sequence(target_sequence, timeout)
    }
}

struct ListenerState {
    shared: Mutex<ListenerShared>,
    next_waiter_id: AtomicU64,
}

impl ListenerState {
    fn new(initial_sequence: u32) -> Self {
        Self {
            shared: Mutex::new(ListenerShared {
                latest_sequence: initial_sequence,
                waiters: Vec::new(),
            }),
            next_waiter_id: AtomicU64::new(1),
        }
    }

    fn observe_sequence(&self, sequence: u32) {
        let mut shared = self
            .shared
            .lock()
            .expect("clipboard listener mutex poisoned");
        if sequence > shared.latest_sequence {
            shared.latest_sequence = sequence;
        }

        let latest = shared.latest_sequence;
        shared.waiters.retain(|waiter| {
            if latest >= waiter.target_sequence {
                waiter.tx.send(()).is_err()
            } else {
                true
            }
        });
    }

    fn wait_for_sequence(&self, target_sequence: u32, timeout: Duration) -> bool {
        let (tx, rx) = mpsc::channel();
        let waiter_id = self.next_waiter_id.fetch_add(1, Ordering::Relaxed);

        {
            let mut shared = self
                .shared
                .lock()
                .expect("clipboard listener mutex poisoned");
            if shared.latest_sequence >= target_sequence {
                return true;
            }
            shared.waiters.push(ClipboardWaiter {
                id: waiter_id,
                target_sequence,
                tx,
            });
        }

        match rx.recv_timeout(timeout) {
            Ok(()) => true,
            Err(_) => {
                let mut shared = self
                    .shared
                    .lock()
                    .expect("clipboard listener mutex poisoned");
                shared.waiters.retain(|waiter| waiter.id != waiter_id);
                false
            }
        }
    }
}

struct ListenerShared {
    latest_sequence: u32,
    waiters: Vec<ClipboardWaiter>,
}

struct ClipboardWaiter {
    id: u64,
    target_sequence: u32,
    tx: mpsc::Sender<()>,
}

fn listener_thread_main(state: Arc<ListenerState>, ready_tx: mpsc::Sender<StartupResult>) {
    if let Err(err) = run_listener_thread(state, ready_tx.clone()) {
        let _ = ready_tx.send(Err(format!("{err:#}")));
    }
}

fn run_listener_thread(
    state: Arc<ListenerState>,
    ready_tx: mpsc::Sender<StartupResult>,
) -> Result<()> {
    register_listener_window_class()?;
    let hwnd = create_message_window()?;
    let _guard = ListenerWindowGuard { hwnd };

    // SAFETY: hwnd is a valid message-only window created on this thread.
    unsafe { AddClipboardFormatListener(hwnd) }
        .context("AddClipboardFormatListener failed for parakit clipboard-history listener")?;
    state.observe_sequence(current_sequence_number());
    let _ = ready_tx.send(Ok(hwnd_to_raw(hwnd)));

    let mut msg = MSG::default();
    loop {
        // SAFETY: msg points to valid writable storage for the Win32 message.
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if status.0 == -1 {
            return Err(WinError::from_thread()).context("clipboard listener GetMessageW failed");
        }
        if status.0 == 0 || msg.message == WM_PARAKIT_CLIPBOARD_LISTENER_STOP {
            return Ok(());
        }
        if msg.message == WM_CLIPBOARDUPDATE {
            state.observe_sequence(current_sequence_number());
        }
    }
}

struct ListenerWindowGuard {
    hwnd: HWND,
}

impl Drop for ListenerWindowGuard {
    fn drop(&mut self) {
        // SAFETY: The window was created on this thread; removing the listener
        // and destroying it here matches the Win32 ownership rules.
        unsafe {
            let _ = RemoveClipboardFormatListener(self.hwnd);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn register_listener_window_class() -> Result<()> {
    // SAFETY: Passing None asks Windows for the module handle of this process.
    let instance = unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW failed")?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(listener_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: w!("ParakitClipboardHistoryListener"),
        ..Default::default()
    };
    // SAFETY: class points to a fully initialized WNDCLASSW. A zero return can
    // also mean the class already exists, which is fine for this process-local
    // helper class.
    unsafe {
        let _ = RegisterClassW(&class);
    }
    Ok(())
}

fn create_message_window() -> Result<HWND> {
    // SAFETY: The registered class uses a no-op window procedure, the parent is
    // HWND_MESSAGE so the window is message-only and never visible.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("ParakitClipboardHistoryListener"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .context("CreateWindowExW failed for parakit clipboard-history listener")
}

fn current_sequence_number() -> u32 {
    // SAFETY: GetClipboardSequenceNumber has no preconditions and does not
    // require opening the clipboard.
    unsafe { GetClipboardSequenceNumber() }
}

fn hwnd_to_raw(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

unsafe extern "system" fn listener_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: The listener does not own message-specific state; all messages
    // are forwarded to the default window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
