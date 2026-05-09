//! Active Windows paste smoke target for `doctor --deep`.

#![cfg(target_os = "windows")]

use anyhow::{bail, Context, Result};
use std::thread;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextLengthW, GetWindowTextW,
    PeekMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, ES_AUTOHSCROLL, MSG,
    PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::inject::{ClipboardPolicy, Injector, PasteMode, PasteOutcome};

const FOCUS_SETTLE: Duration = Duration::from_millis(200);
const PASTE_SETTLE: Duration = Duration::from_millis(400);

/// Paste a sentinel into an owned edit window and verify read-back.
///
/// # Arguments
///
/// * `mode` - Paste shortcut mode to validate.
///
/// # Returns
///
/// `Ok(())` when the sentinel reaches the owned edit window.
///
/// # Errors
///
/// Returns an error when the test window cannot be created or focused, the
/// paste chord fails, or the edit-control text does not match the sentinel.
pub(crate) fn windows_paste_smoke_test(mode: PasteMode) -> Result<()> {
    let sentinel = format!("parakit-smoke-{}", std::process::id());
    let window = TestEditWindow::create().context("create Windows paste smoke edit window")?;
    window
        .focus()
        .context("focus Windows paste smoke edit window")?;
    pump_messages_for(FOCUS_SETTLE);

    let mut injector = Injector::new()?;
    injector.prepare_for_mode(mode)?;
    let outcome = injector
        .paste_text_guarded(&sentinel, mode, ClipboardPolicy::RestorePrevious, || {
            Ok(true)
        })
        .context("Windows paste smoke insertion failed")?;
    if outcome != PasteOutcome::Pasted {
        bail!("Windows paste smoke did not send a paste chord");
    }

    pump_messages_for(PASTE_SETTLE);
    let actual = window
        .text()
        .context("read Windows paste smoke edit text")?;
    if actual != sentinel {
        bail!("Windows paste smoke inserted wrong text: expected {sentinel:?}, got {actual:?}");
    }
    Ok(())
}

struct TestEditWindow {
    hwnd: HWND,
}

impl TestEditWindow {
    fn create() -> Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                100,
                100,
                520,
                120,
                None,
                None,
                None,
                None,
            )
        }
        .context("CreateWindowExW EDIT failed")?;

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        Ok(Self { hwnd })
    }

    fn focus(&self) -> Result<()> {
        let foreground = unsafe { SetForegroundWindow(self.hwnd) };
        if !foreground.as_bool() {
            bail!("SetForegroundWindow failed for Windows paste smoke edit window");
        }
        unsafe { SetFocus(Some(self.hwnd)) }
            .context("SetFocus failed for Windows paste smoke edit window")?;
        Ok(())
    }

    fn text(&self) -> Result<String> {
        let len = unsafe { GetWindowTextLengthW(self.hwnd) };
        let mut buf = vec![0_u16; len as usize + 1];
        let copied = unsafe { GetWindowTextW(self.hwnd, &mut buf) };
        if copied < 0 {
            bail!("GetWindowTextW failed");
        }
        buf.truncate(copied as usize);
        String::from_utf16(&buf).context("Windows paste smoke text was not valid UTF-16")
    }
}

impl Drop for TestEditWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn pump_messages_for(duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() } {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}
