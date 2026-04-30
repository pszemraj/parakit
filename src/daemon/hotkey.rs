//! Push-to-talk hotkey backend.
//!
//! Linux v1 uses the low-level `rdev::grab` evdev path for hotkey capture.
//! The custom X11 `XGrabKey` backend is intentionally out of the daemon
//! critical path; X11 remains only for insertion support.

use super::{audio::AudioHandle, logging::Logger};
use crate::Event_;
use crossbeam_channel::Sender;
use rdev::{Event, EventType, Key};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(150);

/// Hotkey backend preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum HotkeyBackend {
    /// Prefer the Linux-stable evdev grab backend.
    Auto,
    /// Legacy desktop hotkey backend. Disabled on Linux v1.
    Desktop,
    /// Force the low-level evdev grab backend.
    Evdev,
}

impl HotkeyBackend {
    /// Return the stable label used in diagnostics.
    ///
    /// # Returns
    ///
    /// The lowercase backend label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Desktop => "desktop",
            Self::Evdev => "evdev",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HotkeyAction {
    Start {
        started_at: Instant,
    },
    Stop {
        started_at: Instant,
        stopped_at: Instant,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct HotkeyState {
    ctrl_left: bool,
    ctrl_right: bool,
    shift_left: bool,
    shift_right: bool,
    alt: bool,
    alt_gr: bool,
    meta_left: bool,
    meta_right: bool,
    space: bool,
    suppress_space_release: bool,
    recording: bool,
    started_at: Option<Instant>,
    last_start: Option<Instant>,
}

impl HotkeyState {
    fn press(&mut self, key: Key, now: Instant) -> (Option<HotkeyAction>, bool) {
        self.set_key(key, true);
        match key {
            Key::Space if self.ctrl_only() => {
                self.suppress_space_release = true;
                (self.start_recording(now), true)
            }
            Key::Space if self.recording || self.suppress_space_release => (None, true),
            _ => (None, false),
        }
    }

    fn release(&mut self, key: Key, now: Instant) -> (Option<HotkeyAction>, bool) {
        let was_recording = self.recording;
        let suppress_space_release = self.suppress_space_release;
        self.set_key(key, false);
        match key {
            Key::Space if was_recording => {
                self.suppress_space_release = false;
                (self.stop_recording(now), true)
            }
            Key::Space if suppress_space_release => {
                self.suppress_space_release = false;
                (None, true)
            }
            Key::ControlLeft | Key::ControlRight if was_recording && !self.ctrl_held() => {
                (self.stop_recording(now), false)
            }
            _ => (None, false),
        }
    }

    fn start_recording(&mut self, now: Instant) -> Option<HotkeyAction> {
        let debounce_ok = self
            .last_start
            .is_none_or(|last| now.duration_since(last) >= HOTKEY_DEBOUNCE);
        if !self.recording && debounce_ok {
            self.recording = true;
            self.started_at = Some(now);
            self.last_start = Some(now);
            Some(HotkeyAction::Start { started_at: now })
        } else {
            None
        }
    }

    fn stop_recording(&mut self, stopped_at: Instant) -> Option<HotkeyAction> {
        if !self.recording {
            return None;
        }

        self.recording = false;
        let started_at = self.started_at.take().unwrap_or(stopped_at);
        Some(HotkeyAction::Stop {
            started_at,
            stopped_at,
        })
    }

    fn ctrl_held(&self) -> bool {
        self.ctrl_left || self.ctrl_right
    }

    fn extra_modifier_held(&self) -> bool {
        self.shift_left
            || self.shift_right
            || self.alt
            || self.alt_gr
            || self.meta_left
            || self.meta_right
    }

    fn ctrl_only(&self) -> bool {
        self.ctrl_held() && !self.extra_modifier_held()
    }

    fn set_key(&mut self, key: Key, pressed: bool) {
        match key {
            Key::ControlLeft => self.ctrl_left = pressed,
            Key::ControlRight => self.ctrl_right = pressed,
            Key::ShiftLeft => self.shift_left = pressed,
            Key::ShiftRight => self.shift_right = pressed,
            Key::Alt => self.alt = pressed,
            Key::AltGr => self.alt_gr = pressed,
            Key::MetaLeft => self.meta_left = pressed,
            Key::MetaRight => self.meta_right = pressed,
            Key::Space => self.space = pressed,
            _ => {}
        }
    }
}

/// Run the platform hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Worker event channel used to post recording events.
/// * `audio` - Audio capture handle controlled by the hotkey coordinator.
/// * `backend` - Linux backend preference.
/// * `log` - Logger used for backend diagnostics.
#[cfg(target_os = "linux")]
pub(crate) fn run_grab_loop(
    tx: Sender<Event_>,
    audio: AudioHandle,
    backend: HotkeyBackend,
    log: Arc<Logger>,
) {
    match backend {
        HotkeyBackend::Auto | HotkeyBackend::Evdev => {
            log.verbose("parakit: Linux hotkey backend: evdev/rdev grab");
            run_rdev_grab_loop_or_exit(tx, audio);
        }
        HotkeyBackend::Desktop => {
            eprintln!(
                "parakit: --hotkey-backend desktop is disabled in the Linux-stable path.\n\
                 Use --hotkey-backend evdev after granting /dev/input access.\n\
                 Future no-/dev/input desktop hotkeys should use global-hotkey or the XDG portal."
            );
            std::process::exit(2);
        }
    }
}

/// Run the platform hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Worker event channel used to post recording events.
/// * `audio` - Audio capture handle controlled by the hotkey coordinator.
/// * `_backend` - Ignored backend preference on platforms with one backend.
/// * `_log` - Logger unused on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub(crate) fn run_grab_loop(
    tx: Sender<Event_>,
    audio: AudioHandle,
    _backend: HotkeyBackend,
    _log: Arc<Logger>,
) {
    run_rdev_grab_loop_or_exit(tx, audio);
}

fn run_rdev_grab_loop_or_exit(tx: Sender<Event_>, audio: AudioHandle) {
    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let callback_state = Arc::clone(&state);
    let callback_audio = audio.clone();
    let callback_tx = tx.clone();

    if let Err(e) = rdev::grab(move |event| {
        handle_grab_event(event, &callback_state, &callback_audio, &callback_tx)
    }) {
        eprintln!("parakit: rdev::grab failed: {e:?}\n{}", grab_failure_help());
        std::process::exit(2);
    }
}

fn handle_grab_event(
    event: Event,
    state: &Arc<Mutex<HotkeyState>>,
    audio: &AudioHandle,
    tx: &Sender<Event_>,
) -> Option<Event> {
    let now = Instant::now();
    let (action, suppress) = match event.event_type {
        EventType::KeyPress(key) => state
            .lock()
            .expect("hotkey state lock poisoned")
            .press(key, now),
        EventType::KeyRelease(key) => state
            .lock()
            .expect("hotkey state lock poisoned")
            .release(key, now),
        _ => return Some(event),
    };

    if let Some(action) = action {
        dispatch_hotkey_action(action, audio, tx);
    }

    if suppress {
        None
    } else {
        Some(event)
    }
}

fn dispatch_hotkey_action(action: HotkeyAction, audio: &AudioHandle, tx: &Sender<Event_>) {
    match action {
        HotkeyAction::Start { started_at } => {
            audio.start_recording();
            let _ = tx.send(Event_::RecordingStarted { started_at });
        }
        HotkeyAction::Stop {
            started_at,
            stopped_at,
        } => {
            let pcm = audio.stop_recording();
            let _ = tx.send(Event_::RecordingStopped {
                started_at,
                stopped_at,
                pcm,
            });
        }
    }
}

#[cfg(target_os = "linux")]
fn grab_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "Linux hotkey capture uses evdev through rdev::grab.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Fix:\n\
           sudo usermod -aG input {user}\n\
         Then log out completely and log back in, or reboot.\n\
         Verify the fresh session with:\n\
           id -nG | tr ' ' '\\n' | grep '^input$'\n\
         Do not run parakit with sudo; audio, clipboard, and insertion belong to the desktop user."
    )
}

#[cfg(target_os = "macos")]
fn grab_failure_help() -> String {
    "macOS hotkey capture requires Accessibility and Input Monitoring permissions for both the terminal and the parakit binary.".to_string()
}

#[cfg(target_os = "windows")]
fn grab_failure_help() -> String {
    "Windows usually allows the hotkey hook. If security software blocked parakit, whitelist the binary and rerun it.".to_string()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn grab_failure_help() -> String {
    "Global hotkey capture is platform-specific and may need OS-level input permissions."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_time() -> Instant {
        Instant::now()
    }

    #[test]
    fn ctrl_space_starts_and_stops() {
        let now = base_time();
        let mut state = HotkeyState::default();
        assert_eq!(state.press(Key::ControlLeft, now), (None, false));
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(10)),
            (
                Some(HotkeyAction::Start {
                    started_at: now + Duration::from_millis(10)
                }),
                true
            )
        );
        assert_eq!(
            state.release(Key::Space, now + Duration::from_millis(300)),
            (
                Some(HotkeyAction::Stop {
                    started_at: now + Duration::from_millis(10),
                    stopped_at: now + Duration::from_millis(300)
                }),
                true
            )
        );
    }

    #[test]
    fn ctrl_release_before_space_stops_without_suppressing_ctrl_release() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::Space, now + Duration::from_millis(10));
        assert_eq!(
            state.release(Key::ControlLeft, now + Duration::from_millis(50)),
            (
                Some(HotkeyAction::Stop {
                    started_at: now + Duration::from_millis(10),
                    stopped_at: now + Duration::from_millis(50)
                }),
                false
            )
        );
        assert!(!state.recording);
    }

    #[test]
    fn space_release_after_ctrl_release_is_still_suppressed() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::Space, now + Duration::from_millis(10));
        state.release(Key::ControlLeft, now + Duration::from_millis(50));

        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(60)),
            (None, true)
        );
        assert_eq!(
            state.release(Key::Space, now + Duration::from_millis(70)),
            (None, true)
        );
        assert!(!state.recording);
    }

    #[test]
    fn repeated_space_press_while_held_is_suppressed_without_restart() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(10)),
            (
                Some(HotkeyAction::Start {
                    started_at: now + Duration::from_millis(10)
                }),
                true
            )
        );
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(20)),
            (None, true)
        );
        assert!(state.recording);
    }

    #[test]
    fn rapid_double_press_is_ignored_and_suppressed() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::Space, now + Duration::from_millis(10));
        state.release(Key::Space, now + Duration::from_millis(20));
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(80)),
            (None, true)
        );
        assert_eq!(
            state.release(Key::Space, now + Duration::from_millis(90)),
            (None, true)
        );
        assert!(!state.recording);
    }

    #[test]
    fn ctrl_shift_space_does_not_start_or_suppress() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::ShiftLeft, now + Duration::from_millis(5));
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(10)),
            (None, false)
        );
        assert!(!state.recording);
    }

    #[test]
    fn unrelated_keys_pass_through() {
        let now = base_time();
        let mut state = HotkeyState::default();
        assert_eq!(state.press(Key::KeyA, now), (None, false));
        assert_eq!(
            state.release(Key::KeyA, now + Duration::from_millis(10)),
            (None, false)
        );
    }

    #[test]
    fn backend_labels_are_stable() {
        assert_eq!(HotkeyBackend::Auto.label(), "auto");
        assert_eq!(HotkeyBackend::Desktop.label(), "desktop");
        assert_eq!(HotkeyBackend::Evdev.label(), "evdev");
    }
}
