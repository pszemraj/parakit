//! Active Windows paste smoke target for `doctor --deep`.

#![cfg(target_os = "windows")]

use anyhow::{bail, Context, Result};
use std::ffi::c_void;
use std::thread;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    PeekMessageW, RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, ES_AUTOHSCROLL, MSG, PM_REMOVE, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WNDCLASSW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};

use super::inject::{ClipboardPolicy, Injector, PasteMode, PasteOutcome};

const FOCUS_SETTLE: Duration = Duration::from_millis(200);
const PASTE_SETTLE: Duration = Duration::from_millis(400);
const HWND_TOPMOST_RAW: isize = -1;
const HWND_NOTOPMOST_RAW: isize = -2;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_SHOWWINDOW: u32 = 0x0040;

#[link(name = "user32")]
unsafe extern "system" {
    fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
    fn GetCursorPos(point: *mut RawPoint) -> i32;
    fn GetWindowRect(hwnd: HWND, rect: *mut RawRect) -> i32;
    fn mouse_event(flags: u32, dx: u32, dy: u32, data: u32, extra_info: usize);
    fn SetActiveWindow(hwnd: HWND) -> HWND;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn SetWindowPos(
        hwnd: HWND,
        hwnd_insert_after: HWND,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn UpdateWindow(hwnd: HWND) -> i32;
}

const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct RawPoint {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct RawRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

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
    parent: HWND,
    edit: HWND,
}

impl TestEditWindow {
    fn create() -> Result<Self> {
        register_smoke_window_class()?;
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("ParakitSmokeWindow"),
                w!("parakit paste smoke"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
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
        .context("CreateWindowExW parent failed")?;

        let edit = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_TABSTOP
                    | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                12,
                20,
                480,
                28,
                Some(parent),
                None,
                None,
                None,
            )
        }
        .context("CreateWindowExW EDIT failed")?;

        unsafe {
            let _ = ShowWindow(parent, SW_SHOW);
        }
        Ok(Self { parent, edit })
    }

    fn focus(&self) -> Result<()> {
        let _attach = ForegroundThreadAttach::attach();

        let _ = super::windows_input::send_foreground_unlock_alt_tap();
        unsafe {
            let _ = ShowWindow(self.parent, SW_RESTORE);
            let _ = SetWindowPos(
                self.parent,
                hwnd_from_raw(HWND_TOPMOST_RAW),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = SetWindowPos(
                self.parent,
                hwnd_from_raw(HWND_NOTOPMOST_RAW),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            let _ = BringWindowToTop(self.parent);
            let _ = SetActiveWindow(self.parent);
            let _ = UpdateWindow(self.parent);
        }
        pump_messages_for(Duration::from_millis(50));

        let foreground = unsafe { SetForegroundWindow(self.parent) };
        if !foreground.as_bool() && !self.is_foreground() {
            self.click_edit_control()
                .context("click Windows paste smoke edit window")?;
            if !self.is_foreground() {
                bail!(
                    "SetForegroundWindow failed for Windows paste smoke edit window; run doctor --deep from an unlocked interactive Windows desktop"
                );
            }
        }

        unsafe { SetFocus(Some(self.edit)) }
            .context("SetFocus failed for Windows paste smoke edit window")?;
        self.wait_until_foreground(FOCUS_SETTLE)?;
        Ok(())
    }

    fn click_edit_control(&self) -> Result<()> {
        let _cursor = CursorRestoreGuard::capture();
        let mut rect = RawRect::default();
        if unsafe { GetWindowRect(self.edit, &mut rect) } == 0 {
            bail!("GetWindowRect failed for Windows paste smoke edit control");
        }
        let x = rect.left + ((rect.right - rect.left) / 2);
        let y = rect.top + ((rect.bottom - rect.top) / 2);
        if unsafe { SetCursorPos(x, y) } == 0 {
            bail!(
                "SetCursorPos failed for Windows paste smoke edit control at ({x}, {y}); run doctor --deep from an unlocked interactive Windows desktop"
            );
        }
        unsafe {
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
        pump_messages_for(Duration::from_millis(100));
        Ok(())
    }

    fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.parent }
    }

    fn wait_until_foreground(&self, timeout: Duration) -> Result<()> {
        let until = Instant::now() + timeout;
        while Instant::now() < until {
            pump_messages_for(Duration::from_millis(10));
            if self.is_foreground() {
                return Ok(());
            }
        }
        bail!("Windows paste smoke edit window did not become foreground");
    }

    fn text(&self) -> Result<String> {
        let len = unsafe { GetWindowTextLengthW(self.edit) };
        let mut buf = vec![0_u16; len as usize + 1];
        let copied = unsafe { GetWindowTextW(self.edit, &mut buf) };
        if copied < 0 {
            bail!("GetWindowTextW failed");
        }
        buf.truncate(copied as usize);
        String::from_utf16(&buf).context("Windows paste smoke text was not valid UTF-16")
    }
}

fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn register_smoke_window_class() -> Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW failed")?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(smoke_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: w!("ParakitSmokeWindow"),
        ..Default::default()
    };
    unsafe {
        let _ = RegisterClassW(&class);
    }
    Ok(())
}

unsafe extern "system" fn smoke_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

struct ForegroundThreadAttach {
    foreground_tid: u32,
    current_tid: u32,
    attached: bool,
}

impl ForegroundThreadAttach {
    fn attach() -> Self {
        let foreground = unsafe { GetForegroundWindow() };
        let foreground_tid = if foreground.0.is_null() {
            0
        } else {
            unsafe { GetWindowThreadProcessId(foreground, None) }
        };
        let current_tid = unsafe { GetCurrentThreadId() };
        let attached = foreground_tid != 0
            && foreground_tid != current_tid
            && unsafe { AttachThreadInput(current_tid, foreground_tid, 1) != 0 };
        Self {
            foreground_tid,
            current_tid,
            attached,
        }
    }
}

impl Drop for ForegroundThreadAttach {
    fn drop(&mut self) {
        if self.attached {
            unsafe {
                let _ = AttachThreadInput(self.current_tid, self.foreground_tid, 0);
            }
        }
    }
}

struct CursorRestoreGuard {
    point: Option<RawPoint>,
}

impl CursorRestoreGuard {
    fn capture() -> Self {
        let mut point = RawPoint::default();
        let point = if unsafe { GetCursorPos(&mut point) } == 0 {
            None
        } else {
            Some(point)
        };
        Self { point }
    }
}

impl Drop for CursorRestoreGuard {
    fn drop(&mut self) {
        if let Some(point) = self.point {
            unsafe {
                let _ = SetCursorPos(point.x, point.y);
            }
        }
    }
}

impl Drop for TestEditWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.edit);
            let _ = DestroyWindow(self.parent);
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
