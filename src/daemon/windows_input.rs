//! Native Windows keyboard input helpers.

#![cfg(target_os = "windows")]

use anyhow::{bail, Context, Result};
use crossbeam_channel::Sender;
use std::mem::size_of;
use std::thread;
use std::time::{Duration, Instant};
use windows::core::Error as WinError;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_CONTROL,
    MOD_NOREPEAT, VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

use super::recording::HotkeyTransition;

const PARAKIT_HOTKEY_ID: i32 = 0x504b;
const PARAKIT_HOTKEY_PROBE_ID: i32 = PARAKIT_HOTKEY_ID + 1;
const KEY_DOWN_MASK: i16 = i16::MIN;
const HOTKEY_RELEASE_POLL: Duration = Duration::from_millis(10);
const PASTE_KEY_HOLD: Duration = Duration::from_millis(50);
const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

/// Run the native Windows registered-hotkey backend forever.
///
/// # Arguments
///
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
pub(crate) fn run_registered_hotkey_loop_or_exit(tx: Sender<HotkeyTransition>) {
    if let Err(err) = run_registered_hotkey_loop(tx) {
        eprintln!(
            "parakit: Windows registered hotkey failed: {err:#}\n{}",
            windows_hotkey_failure_help()
        );
        std::process::exit(2);
    }
}

/// Probe whether Ctrl+Space can be registered by this process.
///
/// # Returns
///
/// `Ok(())` when Windows accepted and released the registration.
///
/// # Errors
///
/// Returns an error when Ctrl+Space is already owned or cannot be registered.
pub(crate) fn registered_hotkey_probe() -> Result<()> {
    register_ctrl_space(PARAKIT_HOTKEY_PROBE_ID)?;
    let _guard = RegisteredHotkeyGuard {
        id: PARAKIT_HOTKEY_PROBE_ID,
    };
    Ok(())
}

fn run_registered_hotkey_loop(tx: Sender<HotkeyTransition>) -> Result<()> {
    register_ctrl_space(PARAKIT_HOTKEY_ID)?;
    let _registration = RegisteredHotkeyGuard {
        id: PARAKIT_HOTKEY_ID,
    };

    let mut msg = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if status.0 == -1 {
            return Err(WinError::from_win32()).context("GetMessageW failed");
        }
        if status.0 == 0 {
            return Ok(());
        }

        if msg.message == WM_HOTKEY && msg.wParam.0 == PARAKIT_HOTKEY_ID as usize {
            let _ = tx.send(HotkeyTransition::Pressed { at: Instant::now() });
            wait_until_ctrl_space_released();
            let _ = tx.send(HotkeyTransition::Released { at: Instant::now() });
        }
    }
}

fn register_ctrl_space(id: i32) -> Result<()> {
    let modifiers: HOT_KEY_MODIFIERS = MOD_CONTROL | MOD_NOREPEAT;
    unsafe { RegisterHotKey(None, id, modifiers, u32::from(VK_SPACE.0)) }
        .map_err(|err| anyhow::anyhow!("RegisterHotKey Ctrl+Space failed: {err}"))
}

struct RegisteredHotkeyGuard {
    id: i32,
}

impl Drop for RegisteredHotkeyGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}

fn wait_until_ctrl_space_released() {
    while key_is_down(VK_CONTROL) && key_is_down(VK_SPACE) {
        thread::sleep(HOTKEY_RELEASE_POLL);
    }
}

fn key_is_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(i32::from(vk.0)) & KEY_DOWN_MASK) != 0 }
}

/// Send the Windows paste chord.
///
/// # Arguments
///
/// * `use_shift` - `true` for Ctrl+Shift+V, `false` for Ctrl+V.
///
/// # Returns
///
/// `Ok(())` when every keyboard event was accepted by `SendInput`.
///
/// # Errors
///
/// Returns an error when `SendInput` accepts only part of the event sequence,
/// commonly because the target is elevated or input injection is blocked.
pub(crate) fn send_paste_chord(use_shift: bool) -> Result<()> {
    let mut down = Vec::with_capacity(3);
    down.push(key_event(VK_CONTROL, false));
    if use_shift {
        down.push(key_event(VK_SHIFT, false));
    }
    down.push(key_event(VK_V, false));
    send_inputs(&down, "paste chord key-down")?;

    thread::sleep(PASTE_KEY_HOLD);

    let mut up = Vec::with_capacity(14);
    up.push(key_event(VK_V, true));
    if use_shift {
        up.push(key_event(VK_SHIFT, true));
    }
    up.push(key_event(VK_CONTROL, true));
    for vk in [
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_SHIFT,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ] {
        up.push(key_event(vk, true));
    }
    send_inputs(&up, "paste chord key-up/modifier flush")
}

fn key_event(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT], label: &str) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }

    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        bail!(
            "{label}: SendInput sent {sent}/{} events; target may be elevated or input injection may be blocked by UIPI",
            inputs.len()
        );
    }
    Ok(())
}

/// Return the standard Windows hotkey failure help text.
///
/// # Returns
///
/// A static diagnostic string for startup failures.
pub(crate) fn windows_hotkey_failure_help() -> &'static str {
    "Windows hotkey capture uses RegisterHotKey(Ctrl+Space). If registration fails, another application probably owns Ctrl+Space. Close the conflicting application or add a configurable hotkey before using this backend."
}
