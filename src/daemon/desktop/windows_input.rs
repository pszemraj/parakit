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
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_RMENU, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

use crate::daemon::logging::Logger;
use crate::daemon::recording::{DictationMode, HotkeyTransition};

const PARAKIT_HOTKEY_ID: i32 = 0x504b;
const PARAKIT_HOTKEY_PROBE_ID: i32 = PARAKIT_HOTKEY_ID + 1;
const PARAKIT_CONTEXT_HOTKEY_ID: i32 = PARAKIT_HOTKEY_ID + 2;
const PARAKIT_CONTEXT_PROBE_ID: i32 = PARAKIT_HOTKEY_ID + 3;
const KEY_DOWN_MASK: i16 = i16::MIN;
const HOTKEY_RELEASE_POLL: Duration = Duration::from_millis(10);
const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);

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
    register_ctrl_space(PARAKIT_HOTKEY_PROBE_ID, false)?;
    let _guard = RegisteredHotkeyGuard {
        id: PARAKIT_HOTKEY_PROBE_ID,
    };
    Ok(())
}

/// Probe whether the optional Ctrl+Alt+Space chord can be registered.
///
/// # Returns
/// Success when Windows accepts and releases the context chord.
///
/// # Errors
/// Returns an error if another application owns the chord.
pub(crate) fn context_hotkey_probe() -> Result<()> {
    register_ctrl_space(PARAKIT_CONTEXT_PROBE_ID, true)?;
    let _guard = RegisteredHotkeyGuard {
        id: PARAKIT_CONTEXT_PROBE_ID,
    };
    Ok(())
}

/// Run the native Windows registered-hotkey backend until the message loop
/// ends.
///
/// # Arguments
/// * `tx` - Recording coordinator sender.
/// * `context_enabled` - Enable the additional Qwen context chord.
/// * `log` - Reports unavailable optional registration.
///
/// # Returns
///
/// `Ok(())` when Windows ends the thread's message loop.
///
/// # Errors
///
/// Returns an error when registration, message polling, or coordinator
/// delivery fails.
pub(super) fn run_registered_hotkey_loop(
    tx: Sender<HotkeyTransition>,
    context_enabled: bool,
    log: &Logger,
) -> Result<()> {
    register_ctrl_space(PARAKIT_HOTKEY_ID, false)?;
    let _registration = RegisteredHotkeyGuard {
        id: PARAKIT_HOTKEY_ID,
    };
    let context_registration = if context_enabled {
        match register_ctrl_space(PARAKIT_CONTEXT_HOTKEY_ID, true) {
            Ok(()) => Some(RegisteredHotkeyGuard {
                id: PARAKIT_CONTEXT_HOTKEY_ID,
            }),
            Err(err) => {
                log.warn(format!(
                    "could not register Ctrl+Alt+Space: {err}; Ctrl+Space remains available"
                ));
                None
            }
        }
    } else {
        None
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

        if msg.message == WM_HOTKEY {
            if !key_is_down(VK_CONTROL) || !key_is_down(VK_SPACE) {
                continue;
            }
            let mode = if msg.wParam.0 == PARAKIT_HOTKEY_ID as usize {
                DictationMode::Plain
            } else if context_registration.is_some()
                && msg.wParam.0 == PARAKIT_CONTEXT_HOTKEY_ID as usize
            {
                if key_is_down(VK_RMENU) {
                    continue;
                }
                DictationMode::WithContext
            } else {
                continue;
            };
            let _ = tx.send(HotkeyTransition::Pressed {
                at: Instant::now(),
                mode,
            });
            wait_until_ctrl_space_released();
            let _ = tx.send(HotkeyTransition::Released { at: Instant::now() });
            // Releasing Control ends capture, but held Space must not rearm it.
            while key_is_down(VK_SPACE) {
                thread::sleep(HOTKEY_RELEASE_POLL);
            }
        }
    }
}

fn register_ctrl_space(id: i32, context: bool) -> Result<()> {
    let modifiers: HOT_KEY_MODIFIERS = MOD_CONTROL
        | MOD_NOREPEAT
        | if context {
            MOD_ALT
        } else {
            HOT_KEY_MODIFIERS(0)
        };
    unsafe { RegisterHotKey(None, id, modifiers, u32::from(VK_SPACE.0)) }.map_err(|err| {
        anyhow::anyhow!(
            "RegisterHotKey {} failed: {err}",
            if context {
                "Ctrl+Alt+Space"
            } else {
                "Ctrl+Space"
            }
        )
    })
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
    super::paste_chord_steps(VK_CONTROL, use_shift.then_some(VK_SHIFT), VK_V)
        .into_iter()
        .map(|(vk, press)| KeyEvent { vk, key_up: !press })
        .collect()
}

fn paste_cleanup_events_for_partial_send(events: &[KeyEvent], sent: usize) -> Vec<KeyEvent> {
    // Release only keys whose synthetic down was accepted without its matching up.
    // Paste chords contain each key once, unlike arbitrary X11 step sequences.
    super::unreleased_keys(
        events
            .iter()
            .take(sent.min(events.len()))
            .map(|event| (event.vk, !event.key_up)),
    )
    .into_iter()
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

    fn assert_event_codes(events: &[KeyEvent], expected: &[(u16, bool)]) {
        assert_eq!(event_codes(events), expected);
    }

    fn assert_cleanup(events: &[KeyEvent], sent: usize, expected: &[(u16, bool)]) {
        assert_event_codes(
            &paste_cleanup_events_for_partial_send(events, sent),
            expected,
        );
    }

    #[test]
    fn standard_paste_chord_releases_only_owned_control() {
        let events = paste_chord_events(false);

        assert_event_codes(
            &events,
            &[
                (VK_CONTROL.0, false),
                (VK_V.0, false),
                (VK_V.0, true),
                (VK_CONTROL.0, true),
            ],
        );
    }

    #[test]
    fn terminal_paste_chord_includes_shift_in_one_batch() {
        let events = paste_chord_events(true);

        assert_event_codes(
            &events,
            &[
                (VK_CONTROL.0, false),
                (VK_SHIFT.0, false),
                (VK_V.0, false),
                (VK_V.0, true),
                (VK_SHIFT.0, true),
                (VK_CONTROL.0, true),
            ],
        );
    }

    #[test]
    fn partial_paste_send_cleans_up_unreleased_owned_keys() {
        let standard = paste_chord_events(false);

        assert_cleanup(&standard, 0, &[]);
        assert_cleanup(&standard, 1, &[(VK_CONTROL.0, true)]);
        assert_cleanup(&standard, 2, &[(VK_V.0, true), (VK_CONTROL.0, true)]);
        assert_cleanup(&standard, 3, &[(VK_CONTROL.0, true)]);

        let terminal = paste_chord_events(true);

        assert_cleanup(&terminal, 2, &[(VK_SHIFT.0, true), (VK_CONTROL.0, true)]);
        assert_cleanup(
            &terminal,
            3,
            &[(VK_V.0, true), (VK_SHIFT.0, true), (VK_CONTROL.0, true)],
        );
        assert_cleanup(&terminal, 4, &[(VK_SHIFT.0, true), (VK_CONTROL.0, true)]);
        assert_cleanup(&terminal, 5, &[(VK_CONTROL.0, true)]);
    }

    #[test]
    fn partial_send_attempts_owned_key_cleanup_and_reports_failures() {
        for (name, terminal, responses, expected_calls, expected_error) in [
            (
                "partial send with successful cleanup",
                false,
                [2, 2],
                [4, 2],
                "paste chord: SendInput sent 2/4 events",
            ),
            (
                "partial send with failed cleanup",
                true,
                [3, 1],
                [6, 3],
                "paste key cleanup also failed",
            ),
        ] {
            let mut sender = MockInputSender::new(&responses);
            let err = send_paste_chord_with(terminal, &mut sender).expect_err(name);

            assert_eq!(sender.calls, expected_calls, "{name}");
            assert!(
                format!("{err:#}").contains(expected_error),
                "{name}: {err:#}"
            );
        }
    }
}
