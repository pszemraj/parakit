//! Windows clipboard-history notification tracking.

#![cfg(target_os = "windows")]

use anyhow::{Context, Result};
use std::ffi::c_void;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::{w, Error as WinError, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetMessageW, PostMessageW, RegisterClassW,
    HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WNDCLASSW,
    WNDCLASS_STYLES,
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
    latest_sequence: Mutex<u32>,
    changed: Condvar,
}

impl ListenerState {
    fn new(initial_sequence: u32) -> Self {
        Self {
            latest_sequence: Mutex::new(initial_sequence),
            changed: Condvar::new(),
        }
    }

    fn observe_sequence(&self, sequence: u32) {
        let mut latest = self
            .latest_sequence
            .lock()
            .expect("clipboard listener mutex poisoned");
        if sequence > *latest {
            *latest = sequence;
            self.changed.notify_all();
        }
    }

    fn wait_for_sequence(&self, target_sequence: u32, timeout: Duration) -> bool {
        let latest = self
            .latest_sequence
            .lock()
            .expect("clipboard listener mutex poisoned");
        let (latest, _) = self
            .changed
            .wait_timeout_while(latest, timeout, |latest| *latest < target_sequence)
            .expect("clipboard listener mutex poisoned");
        *latest >= target_sequence
    }
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
    register_window_class(w!("ParakitClipboardHistoryListener"), WNDCLASS_STYLES(0))
}

/// Register a message-only window class backed by the shared no-op window
/// procedure.
///
/// Shared by the clipboard-history listener and the paste smoke test, which
/// both need a throwaway `WNDCLASSW` registration and never process window
/// messages themselves.
///
/// # Arguments
///
/// * `class_name` - The class name to register, as a `'static` wide string.
/// * `style` - The `WNDCLASSW` style flags for the class.
///
/// # Returns
///
/// `Ok(())` once the class is registered (or already existed).
///
/// # Errors
///
/// Returns an error if the module handle for this process is unavailable.
pub(super) fn register_window_class(class_name: PCWSTR, style: WNDCLASS_STYLES) -> Result<()> {
    // SAFETY: Passing None asks Windows for the module handle of this process.
    let instance = unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW failed")?;
    let class = WNDCLASSW {
        style,
        lpfnWndProc: Some(noop_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
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

/// Build an `HWND` from a raw pointer-sized value.
///
/// Shared by the clipboard-history listener, which carries a handle across
/// its own thread boundary, and the paste smoke test, which also uses it to
/// build the `HWND_TOPMOST`/`HWND_NOTOPMOST` sentinels for `SetWindowPos`.
///
/// # Returns
///
/// The `HWND` wrapping `raw` as a pointer.
pub(super) fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

/// No-op window procedure shared by the clipboard-history listener and the
/// paste smoke test windows; neither owns message-specific state, so every
/// message is forwarded to the default window procedure.
unsafe extern "system" fn noop_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: No message-specific state is owned by either caller; all
    // messages are forwarded to the default window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_observed_sequence_does_not_wait() {
        let state = ListenerState::new(2);
        assert!(state.wait_for_sequence(2, Duration::ZERO));
        state.observe_sequence(1);
        assert!(state.wait_for_sequence(2, Duration::ZERO));
    }

    #[test]
    fn unobserved_sequence_times_out() {
        let state = ListenerState::new(1);
        assert!(!state.wait_for_sequence(2, Duration::ZERO));
        assert!(!state.wait_for_sequence(2, Duration::from_millis(5)));
    }

    #[test]
    fn sequence_update_wakes_all_matching_waiters() {
        let state = ListenerState::new(1);
        let ready = std::sync::Barrier::new(4);
        thread::scope(|scope| {
            let waits: Vec<_> = (0..3)
                .map(|_| {
                    scope.spawn(|| {
                        ready.wait();
                        state.wait_for_sequence(3, Duration::from_secs(2))
                    })
                })
                .collect();
            ready.wait();
            state.observe_sequence(2);
            state.observe_sequence(3);
            for wait in waits {
                assert!(wait.join().unwrap());
            }
        });
    }

    #[test]
    fn unrelated_notification_does_not_satisfy_a_waiter() {
        let state = ListenerState::new(1);
        thread::scope(|scope| {
            let wait = scope.spawn(|| state.wait_for_sequence(3, Duration::from_millis(20)));
            state.changed.notify_all();
            state.observe_sequence(2);
            assert!(!wait.join().unwrap());
        });
    }
}
