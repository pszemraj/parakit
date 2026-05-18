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
    MOD_NOREPEAT, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT, VK_SPACE,
};
#[cfg(test)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

use crate::daemon::recording::HotkeyTransition;

const PARAKIT_HOTKEY_ID: i32 = 0x504b;
const PARAKIT_HOTKEY_PROBE_ID: i32 = PARAKIT_HOTKEY_ID + 1;
const KEY_DOWN_MASK: i16 = i16::MIN;
const HOTKEY_RELEASE_POLL: Duration = Duration::from_millis(10);
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
            return Err(WinError::from_thread()).context("GetMessageW failed");
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
    let mut sender = Win32InputSender;
    send_paste_chord_with(use_shift, &mut sender)
}

fn send_paste_chord_with<S: InputSender>(use_shift: bool, sender: &mut S) -> Result<()> {
    let events = paste_chord_events(use_shift);
    let inputs = events
        .iter()
        .map(|event| key_event(event.vk, event.key_up))
        .collect::<Vec<_>>();
    let sent = sender.send_inputs(&inputs);
    if sent == inputs.len() as u32 {
        return Ok(());
    }

    let cleanup_events = paste_cleanup_events_for_partial_send(&events, sent as usize);
    let cleanup_result = send_key_events_exact(sender, &cleanup_events, "paste chord cleanup");
    let primary_error = anyhow::anyhow!(
        "paste chord: SendInput sent {sent}/{} events; target may be elevated or input injection may be blocked by UIPI",
        inputs.len()
    );

    match cleanup_result {
        Ok(()) => Err(primary_error),
        Err(cleanup_error) => Err(anyhow::anyhow!(
            "{primary_error:#}; paste key cleanup also failed: {cleanup_error:#}"
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct KeyEvent {
    vk: VIRTUAL_KEY,
    key_up: bool,
}

fn paste_chord_events(use_shift: bool) -> Vec<KeyEvent> {
    let mut events = Vec::with_capacity(if use_shift { 6 } else { 4 });
    events.push(KeyEvent {
        vk: VK_CONTROL,
        key_up: false,
    });
    if use_shift {
        events.push(KeyEvent {
            vk: VK_SHIFT,
            key_up: false,
        });
    }
    events.push(KeyEvent {
        vk: VK_V,
        key_up: false,
    });
    events.push(KeyEvent {
        vk: VK_V,
        key_up: true,
    });
    if use_shift {
        events.push(KeyEvent {
            vk: VK_SHIFT,
            key_up: true,
        });
    }
    events.push(KeyEvent {
        vk: VK_CONTROL,
        key_up: true,
    });
    events
}

fn paste_cleanup_events_for_partial_send(events: &[KeyEvent], sent: usize) -> Vec<KeyEvent> {
    let mut down = Vec::new();

    for event in events.iter().take(sent.min(events.len())) {
        if event.key_up {
            if let Some(pos) = down.iter().rposition(|vk| *vk == event.vk) {
                down.remove(pos);
            }
        } else if !down.contains(&event.vk) {
            down.push(event.vk);
        }
    }

    // Release only keys whose synthetic down was accepted without its matching up.
    down.into_iter()
        .rev()
        .map(|vk| KeyEvent { vk, key_up: true })
        .collect()
}

/// Send a short Alt tap to unlock Windows foreground activation.
///
/// # Returns
///
/// `Ok(())` when Windows accepted the synthetic Alt key events.
///
/// # Errors
///
/// Returns an error when `SendInput` rejects the event sequence.
pub(crate) fn send_foreground_unlock_alt_tap() -> Result<()> {
    send_inputs(
        &[key_event(VK_MENU, false), key_event(VK_MENU, true)],
        "foreground unlock Alt tap",
    )
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

trait InputSender {
    /// Send the provided Win32 input events.
    ///
    /// # Returns
    ///
    /// The number of input events accepted by Windows.
    fn send_inputs(&mut self, inputs: &[INPUT]) -> u32;
}

struct Win32InputSender;

impl InputSender for Win32InputSender {
    fn send_inputs(&mut self, inputs: &[INPUT]) -> u32 {
        unsafe { SendInput(inputs, size_of::<INPUT>() as i32) }
    }
}

fn send_key_events_exact<S: InputSender>(
    sender: &mut S,
    events: &[KeyEvent],
    label: &str,
) -> Result<()> {
    let inputs = events
        .iter()
        .map(|event| key_event(event.vk, event.key_up))
        .collect::<Vec<_>>();
    send_inputs_with(sender, &inputs, label)
}

fn send_inputs(inputs: &[INPUT], label: &str) -> Result<()> {
    let mut sender = Win32InputSender;
    send_inputs_with(&mut sender, inputs, label)
}

fn send_inputs_with<S: InputSender>(sender: &mut S, inputs: &[INPUT], label: &str) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }

    let sent = sender.send_inputs(inputs);
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

#[cfg(test)]
mod tests {
    use super::*;

    struct MockInputSender {
        responses: Vec<u32>,
        calls: Vec<usize>,
    }

    impl MockInputSender {
        fn new(responses: &[u32]) -> Self {
            Self {
                responses: responses.iter().rev().copied().collect(),
                calls: Vec::new(),
            }
        }
    }

    impl InputSender for MockInputSender {
        fn send_inputs(&mut self, inputs: &[INPUT]) -> u32 {
            self.calls.push(inputs.len());
            self.responses.pop().unwrap_or(inputs.len() as u32)
        }
    }

    fn event_codes(events: &[KeyEvent]) -> Vec<(u16, bool)> {
        events
            .iter()
            .map(|event| (event.vk.0, event.key_up))
            .collect()
    }

    #[test]
    fn standard_paste_chord_releases_only_owned_control() {
        let events = paste_chord_events(false);

        assert_eq!(
            event_codes(&events),
            vec![
                (VK_CONTROL.0, false),
                (VK_V.0, false),
                (VK_V.0, true),
                (VK_CONTROL.0, true),
            ]
        );
        assert!(!events.iter().any(|event| {
            event.vk == VK_MENU
                || event.vk == VK_LMENU
                || event.vk == VK_RMENU
                || event.vk == VK_LWIN
                || event.vk == VK_RWIN
                || event.vk == VK_LCONTROL
                || event.vk == VK_RCONTROL
                || event.vk == VK_LSHIFT
                || event.vk == VK_RSHIFT
        }));
    }

    #[test]
    fn terminal_paste_chord_includes_shift_in_one_batch() {
        let events = paste_chord_events(true);

        assert_eq!(
            event_codes(&events),
            vec![
                (VK_CONTROL.0, false),
                (VK_SHIFT.0, false),
                (VK_V.0, false),
                (VK_V.0, true),
                (VK_SHIFT.0, true),
                (VK_CONTROL.0, true),
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.vk == VK_SHIFT && event.key_up)
                .count(),
            1
        );
    }

    #[test]
    fn partial_paste_send_cleans_up_unreleased_owned_keys() {
        let standard = paste_chord_events(false);

        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&standard, 0)),
            Vec::<(u16, bool)>::new()
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&standard, 1)),
            vec![(VK_CONTROL.0, true)]
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&standard, 2)),
            vec![(VK_V.0, true), (VK_CONTROL.0, true)]
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&standard, 3)),
            vec![(VK_CONTROL.0, true)]
        );

        let terminal = paste_chord_events(true);

        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&terminal, 2)),
            vec![(VK_SHIFT.0, true), (VK_CONTROL.0, true)]
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&terminal, 3)),
            vec![(VK_V.0, true), (VK_SHIFT.0, true), (VK_CONTROL.0, true)]
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&terminal, 4)),
            vec![(VK_SHIFT.0, true), (VK_CONTROL.0, true)]
        );
        assert_eq!(
            event_codes(&paste_cleanup_events_for_partial_send(&terminal, 5)),
            vec![(VK_CONTROL.0, true)]
        );
    }

    #[test]
    fn partial_send_attempts_owned_key_cleanup() {
        let mut sender = MockInputSender::new(&[2, 2]);
        let err = send_paste_chord_with(false, &mut sender).expect_err("partial send should fail");

        assert_eq!(sender.calls, vec![4, 2]);
        assert!(format!("{err:#}").contains("paste chord: SendInput sent 2/4 events"));
    }

    #[test]
    fn partial_send_reports_cleanup_failure() {
        let mut sender = MockInputSender::new(&[3, 1]);
        let err =
            send_paste_chord_with(true, &mut sender).expect_err("partial cleanup should fail");

        assert_eq!(sender.calls, vec![6, 3]);
        assert!(format!("{err:#}").contains("paste key cleanup also failed"));
    }
}
