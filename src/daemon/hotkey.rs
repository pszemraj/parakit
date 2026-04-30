//! Push-to-talk hotkey backends.
//!
//! Linux `auto` uses the low-level rdev evdev grab when all input devices are
//! readable; otherwise it uses a desktop X11 hotkey registration. evdev is more
//! resilient across desktop session churn, but requires explicit permissions.

use super::audio::AudioHandle;
use crate::Event_;
use anyhow::Context;
use crossbeam_channel::Sender;
use once_cell::sync::OnceCell;
use rdev::{Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};

static GRAB_TX: OnceCell<Sender<Event_>> = OnceCell::new();
static GRAB_AUDIO: OnceCell<AudioHandle> = OnceCell::new();
static CTRL_HELD: AtomicBool = AtomicBool::new(false);
static SPACE_HELD: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
const X11_HOTKEY_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(target_os = "linux")]
const X11_HOTKEY_REFRESH_FAILURE_LIMIT: u32 = 3;

/// Hotkey backend preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum HotkeyBackend {
    /// Prefer the most stable available backend.
    Auto,
    /// Force the X11 desktop hotkey backend.
    Desktop,
    /// Force the low-level evdev grab backend.
    Evdev,
}

/// Run the platform hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Worker event channel used to post start and stop events.
/// * `audio` - Audio capture handle toggled by the hotkey state.
/// * `backend` - Linux backend preference.
#[cfg(target_os = "linux")]
pub(crate) fn run_grab_loop(tx: Sender<Event_>, audio: AudioHandle, backend: HotkeyBackend) {
    let _ = GRAB_TX.set(tx);
    let _ = GRAB_AUDIO.set(audio);

    match backend {
        HotkeyBackend::Auto => run_auto_hotkey_loop(),
        HotkeyBackend::Desktop => run_desktop_hotkey_loop_or_exit(),
        HotkeyBackend::Evdev => run_evdev_grab_loop_or_exit(),
    }
}

#[cfg(target_os = "linux")]
fn run_auto_hotkey_loop() {
    if super::preflight::linux_evdev_fallback_available() {
        match rdev::grab(grab_callback) {
            Ok(()) => return,
            Err(err) => {
                eprintln!("parakit: rdev evdev grab failed: {err:?}");
                eprintln!("parakit: trying X11 desktop hotkey backend");
            }
        }
    }

    if super::preflight::linux_x11_desktop_hotkey_candidate() {
        match run_x11_desktop_hotkey_loop() {
            Ok(()) => return,
            Err(err) => {
                eprintln!("parakit: X11 desktop hotkey backend failed: {err:#}");
                if !super::preflight::linux_evdev_fallback_available() {
                    eprintln!("{}", linux_no_hotkey_backend_help());
                    std::process::exit(2);
                }
                eprintln!("parakit: falling back to rdev evdev grab");
            }
        }
    }

    run_evdev_grab_loop_or_exit();
}

#[cfg(target_os = "linux")]
fn run_desktop_hotkey_loop_or_exit() {
    if !super::preflight::linux_x11_desktop_hotkey_candidate() {
        eprintln!("{}", linux_no_hotkey_backend_help());
        std::process::exit(2);
    }

    if let Err(err) = run_x11_desktop_hotkey_loop() {
        eprintln!("parakit: X11 desktop hotkey backend failed: {err:#}");
        eprintln!("{}", linux_no_hotkey_backend_help());
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_evdev_grab_loop_or_exit() {
    if let Err(e) = rdev::grab(grab_callback) {
        eprintln!("parakit: rdev::grab failed: {e:?}\n{}", grab_failure_help());
        std::process::exit(2);
    }
}

/// Run the platform hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Worker event channel used to post start and stop events.
/// * `audio` - Audio capture handle toggled by the hotkey state.
/// * `_backend` - Ignored backend preference on platforms with one backend.
#[cfg(not(target_os = "linux"))]
pub(crate) fn run_grab_loop(tx: Sender<Event_>, audio: AudioHandle, _backend: HotkeyBackend) {
    let _ = GRAB_TX.set(tx);
    let _ = GRAB_AUDIO.set(audio);

    if let Err(e) = rdev::grab(grab_callback) {
        eprintln!("parakit: rdev::grab failed: {e:?}\n{}", grab_failure_help());
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_x11_desktop_hotkey_loop() -> anyhow::Result<()> {
    use crossbeam_channel::RecvTimeoutError;
    use global_hotkey::HotKeyState;
    use global_hotkey::{hotkey::Code, hotkey::HotKey, hotkey::Modifiers, GlobalHotKeyEvent};
    use std::time::{Duration, Instant};

    struct X11HotkeyBackend {
        manager: global_hotkey::GlobalHotKeyManager,
        hotkey: HotKey,
    }

    impl X11HotkeyBackend {
        fn new() -> anyhow::Result<Self> {
            let manager = global_hotkey::GlobalHotKeyManager::new()
                .map_err(|err| anyhow::anyhow!(err))
                .context("could not create global hotkey manager")?;
            let hotkey = HotKey::new(Some(Modifiers::CONTROL), Code::Space);
            manager
                .register(hotkey)
                .map_err(|err| anyhow::anyhow!(err))
                .context(
                    "could not register Ctrl+Space; another desktop shortcut may already own it",
                )?;
            Ok(Self { manager, hotkey })
        }

        fn refresh(&mut self) -> anyhow::Result<()> {
            let _ = self.manager.unregister(self.hotkey);
            match Self::new() {
                Ok(replacement) => {
                    *self = replacement;
                    CTRL_HELD.store(false, Ordering::SeqCst);
                    Ok(())
                }
                Err(err) => {
                    let _ = self.manager.register(self.hotkey);
                    Err(err)
                }
            }
        }
    }

    let mut backend = X11HotkeyBackend::new()?;
    let receiver = GlobalHotKeyEvent::receiver();
    let mut next_refresh = Instant::now() + X11_HOTKEY_REFRESH;
    let mut refresh_failures = 0_u32;

    loop {
        let timeout = next_refresh
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);

        match receiver.recv_timeout(timeout) {
            Ok(event) => {
                if event.id != backend.hotkey.id() {
                    continue;
                }

                refresh_failures = 0;
                match event.state {
                    HotKeyState::Pressed => {
                        CTRL_HELD.store(true, Ordering::SeqCst);
                        if !SPACE_HELD.swap(true, Ordering::SeqCst) {
                            start_hotkey_recording();
                        }
                    }
                    HotKeyState::Released => {
                        CTRL_HELD.store(false, Ordering::SeqCst);
                        if SPACE_HELD.swap(false, Ordering::SeqCst) {
                            stop_hotkey_recording();
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                next_refresh = Instant::now() + X11_HOTKEY_REFRESH;
                if !SPACE_HELD.load(Ordering::SeqCst) {
                    match backend.refresh() {
                        Ok(()) => refresh_failures = 0,
                        Err(err) => {
                            reset_hotkey_state_after_backend_loss();
                            refresh_failures += 1;
                            if refresh_failures >= X11_HOTKEY_REFRESH_FAILURE_LIMIT {
                                return Err(err).context(
                                    "X11 hotkey refresh failed repeatedly after desktop/session churn",
                                );
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("global hotkey event channel closed");
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_no_hotkey_backend_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "No usable Linux hotkey backend is available.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Preferred path: use an Xorg/X11 session so parakit can register\n\
         Ctrl+Space through the desktop without /dev/input access.\n\
         Fallback path: grant evdev access, then log out completely and back in:\n\
           sudo usermod -aG input {user}\n\
         If Ctrl+Space is already owned by the desktop or input method, disable\n\
         that shortcut and restart parakit."
    )
}

#[cfg(target_os = "linux")]
fn grab_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "The evdev fallback requires read access to /dev/input/event*.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Prefer an Xorg/X11 session so parakit can use the desktop hotkey\n\
         backend without evdev permissions. Otherwise add your user to the\n\
         input group, then log out completely and log back in:\n\
           sudo usermod -aG input {user}\n\
         Verify the new login session with:\n\
           id -nG | tr ' ' '\\n' | grep '^input$'\n\
         Restart tmux, terminals, or user services that were started before the\n\
         group change. Avoid running parakit with sudo; audio, X11, and text\n\
         insertion usually belongs to the regular desktop user."
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

fn start_hotkey_recording() {
    if let Some(audio) = GRAB_AUDIO.get() {
        audio.start_recording();
    }
    if let Some(tx) = GRAB_TX.get() {
        let _ = tx.send(Event_::Start);
    }
}

fn stop_hotkey_recording() {
    if let Some(tx) = GRAB_TX.get() {
        let _ = tx.send(Event_::Stop);
    }
}

#[cfg(target_os = "linux")]
fn reset_hotkey_state_after_backend_loss() {
    CTRL_HELD.store(false, Ordering::SeqCst);
    if SPACE_HELD.swap(false, Ordering::SeqCst) {
        stop_hotkey_recording();
    }
}

fn grab_callback(event: Event) -> Option<Event> {
    match event.event_type {
        EventType::KeyPress(Key::ControlLeft) | EventType::KeyPress(Key::ControlRight) => {
            CTRL_HELD.store(true, Ordering::SeqCst);
            Some(event)
        }
        EventType::KeyRelease(Key::ControlLeft) | EventType::KeyRelease(Key::ControlRight) => {
            CTRL_HELD.store(false, Ordering::SeqCst);
            // If user released Ctrl while still holding Space, end the recording.
            if SPACE_HELD.swap(false, Ordering::SeqCst) {
                stop_hotkey_recording();
                return None;
            }
            Some(event)
        }
        EventType::KeyPress(Key::Space) => {
            if CTRL_HELD.load(Ordering::SeqCst) {
                if !SPACE_HELD.swap(true, Ordering::SeqCst) {
                    start_hotkey_recording();
                }
                return None;
            }
            Some(event)
        }
        EventType::KeyRelease(Key::Space) => {
            if SPACE_HELD.swap(false, Ordering::SeqCst) {
                stop_hotkey_recording();
                return None;
            }
            Some(event)
        }
        _ => Some(event),
    }
}
