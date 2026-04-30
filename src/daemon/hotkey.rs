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
use std::sync::Mutex;
use std::time::{Duration, Instant};

static GRAB_TX: OnceCell<Sender<Event_>> = OnceCell::new();
static GRAB_AUDIO: OnceCell<AudioHandle> = OnceCell::new();
static HOTKEY_STATE: OnceCell<Mutex<HotkeyState>> = OnceCell::new();

#[cfg(target_os = "linux")]
const X11_HOTKEY_REFRESH: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const X11_HOTKEY_REFRESH_FAILURE_LIMIT: u32 = 3;
const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(150);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HotkeyAction {
    Start,
    Stop,
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
            Key::Space if self.recording => (None, true),
            _ => (None, false),
        }
    }

    fn release(&mut self, key: Key) -> (Option<HotkeyAction>, bool) {
        let was_recording = self.recording;
        let suppress_space_release = self.suppress_space_release;
        self.set_key(key, false);
        match key {
            Key::Space if was_recording => {
                self.suppress_space_release = false;
                (self.stop_recording(), true)
            }
            Key::Space if suppress_space_release => {
                self.suppress_space_release = false;
                (None, true)
            }
            Key::ControlLeft | Key::ControlRight if was_recording && !self.ctrl_held() => {
                self.space = false;
                self.suppress_space_release = false;
                (self.stop_recording(), true)
            }
            _ => (None, false),
        }
    }

    fn desktop_press(&mut self, now: Instant) -> Option<HotkeyAction> {
        self.ctrl_left = true;
        self.space = true;
        self.suppress_space_release = true;
        self.start_recording(now)
    }

    fn desktop_release(&mut self) -> Option<HotkeyAction> {
        self.ctrl_left = false;
        self.space = false;
        self.suppress_space_release = false;
        self.stop_recording()
    }

    fn start_recording(&mut self, now: Instant) -> Option<HotkeyAction> {
        let debounce_ok = self
            .last_start
            .is_none_or(|last| now.duration_since(last) >= HOTKEY_DEBOUNCE);
        if !self.recording && debounce_ok {
            self.recording = true;
            self.last_start = Some(now);
            Some(HotkeyAction::Start)
        } else {
            None
        }
    }

    fn stop_recording(&mut self) -> Option<HotkeyAction> {
        if self.recording {
            self.recording = false;
            Some(HotkeyAction::Stop)
        } else {
            None
        }
    }

    fn reset_after_backend_loss(&mut self) -> Option<HotkeyAction> {
        let was_recording = self.recording;
        *self = Self::default();
        was_recording.then_some(HotkeyAction::Stop)
    }

    fn is_recording(&self) -> bool {
        self.recording
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

fn hotkey_state() -> &'static Mutex<HotkeyState> {
    HOTKEY_STATE.get_or_init(|| Mutex::new(HotkeyState::default()))
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
    use super::x11;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, Window};
    use x11rb::protocol::Event as X11Event;
    use x11rb::rust_connection::RustConnection;

    struct X11HotkeyBackend {
        conn: RustConnection,
        root: Window,
        keycode: Keycode,
    }

    impl X11HotkeyBackend {
        fn new() -> anyhow::Result<Self> {
            let (conn, screen_num) =
                RustConnection::connect(None).context("could not connect to the X11 display")?;
            let root = x11::root_window(&conn, screen_num)?;
            let keycode = x11::keycode_for_keysym(&conn, x11::SPACE_KEYSYM)?;
            let backend = Self {
                conn,
                root,
                keycode,
            };
            backend
                .grab()
                .context("could not register Ctrl+Space; another shortcut may already own it")?;
            Ok(backend)
        }

        fn grab(&self) -> anyhow::Result<()> {
            for mods in x11::ctrl_grab_mods() {
                let result = self
                    .conn
                    .grab_key(
                        false,
                        self.root,
                        mods,
                        self.keycode,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )
                    .context("could not send XGrabKey request")?;
                if let Err(err) = result.check() {
                    self.ungrab();
                    return Err(anyhow::anyhow!(err)).context("XGrabKey rejected Ctrl+Space");
                }
            }
            self.conn.flush().context("could not flush XGrabKey")?;
            Ok(())
        }

        fn ungrab(&self) {
            for mods in x11::ctrl_grab_mods() {
                if let Ok(result) = self.conn.ungrab_key(self.keycode, self.root, mods) {
                    result.ignore_error();
                }
            }
            let _ = self.conn.flush();
        }

        fn refresh(&mut self) -> anyhow::Result<()> {
            self.ungrab();
            match Self::new() {
                Ok(replacement) => {
                    *self = replacement;
                    Ok(())
                }
                Err(err) => {
                    let _ = self.grab();
                    Err(err)
                }
            }
        }

        fn poll_event(&self) -> anyhow::Result<Option<X11Event>> {
            self.conn
                .poll_for_event()
                .context("X11 hotkey event polling failed")
        }
    }

    let mut backend = X11HotkeyBackend::new()?;
    let mut next_refresh = Instant::now() + X11_HOTKEY_REFRESH;
    let mut refresh_failures = 0_u32;

    loop {
        match backend.poll_event()? {
            Some(event) => match event {
                X11Event::KeyPress(event) if event.detail == backend.keycode => {
                    if let Some(action) = hotkey_state()
                        .lock()
                        .expect("hotkey state lock poisoned")
                        .desktop_press(Instant::now())
                    {
                        dispatch_hotkey_action(action);
                    }
                    refresh_failures = 0;
                }
                X11Event::KeyRelease(event) if event.detail == backend.keycode => {
                    if let Some(action) = hotkey_state()
                        .lock()
                        .expect("hotkey state lock poisoned")
                        .desktop_release()
                    {
                        dispatch_hotkey_action(action);
                    }
                    refresh_failures = 0;
                }
                _ => {}
            },
            None => {
                std::thread::sleep(Duration::from_millis(25));
            }
        }

        if Instant::now() >= next_refresh {
            next_refresh = Instant::now() + X11_HOTKEY_REFRESH;
            if !hotkey_state()
                .lock()
                .expect("hotkey state lock poisoned")
                .is_recording()
            {
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
    }
}

#[cfg(target_os = "linux")]
fn linux_no_hotkey_backend_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let xauthority = std::env::var("XAUTHORITY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "No usable Linux hotkey backend is available.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}, XAUTHORITY={xauthority}\n\
         Preferred path: use an Xorg/X11 session so parakit can register\n\
         Ctrl+Space through the desktop without /dev/input access.\n\
         If this started after GNOME logout/login, restart tmux, terminals,\n\
         and user services from the new desktop session so DISPLAY/XAUTHORITY\n\
         are fresh.\n\
         Session-stable path: grant evdev access, then log out completely and back in:\n\
           sudo usermod -aG input {user}\n\
         Then run: parakit --hotkey-backend evdev\n\
         If Ctrl+Space is already owned by the desktop or input method, disable\n\
         that shortcut and restart parakit."
    )
}

#[cfg(target_os = "linux")]
fn grab_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let xauthority = std::env::var("XAUTHORITY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "The evdev fallback requires read access to /dev/input/event*.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}, XAUTHORITY={xauthority}\n\
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

fn dispatch_hotkey_action(action: HotkeyAction) {
    match action {
        HotkeyAction::Start => start_hotkey_recording(),
        HotkeyAction::Stop => stop_hotkey_recording(),
    }
}

#[cfg(target_os = "linux")]
fn reset_hotkey_state_after_backend_loss() {
    if let Some(action) = hotkey_state()
        .lock()
        .expect("hotkey state lock poisoned")
        .reset_after_backend_loss()
    {
        dispatch_hotkey_action(action);
    }
}

fn grab_callback(event: Event) -> Option<Event> {
    match event.event_type {
        EventType::KeyPress(key) => {
            let (action, suppress) = hotkey_state()
                .lock()
                .expect("hotkey state lock poisoned")
                .press(key, Instant::now());
            if let Some(action) = action {
                dispatch_hotkey_action(action);
            }
            if suppress {
                return None;
            }
            Some(event)
        }
        EventType::KeyRelease(key) => {
            let (action, suppress) = hotkey_state()
                .lock()
                .expect("hotkey state lock poisoned")
                .release(key);
            if let Some(action) = action {
                dispatch_hotkey_action(action);
            }
            if suppress {
                return None;
            }
            Some(event)
        }
        _ => Some(event),
    }
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
            (Some(HotkeyAction::Start), true)
        );
        assert_eq!(state.release(Key::Space), (Some(HotkeyAction::Stop), true));
    }

    #[test]
    fn ctrl_release_before_space_stops() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::Space, now + Duration::from_millis(10));
        assert_eq!(
            state.release(Key::ControlLeft),
            (Some(HotkeyAction::Stop), true)
        );
        assert!(!state.is_recording());
    }

    #[test]
    fn repeated_space_press_while_held_is_suppressed_without_restart() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(10)),
            (Some(HotkeyAction::Start), true)
        );
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(20)),
            (None, true)
        );
        assert!(state.is_recording());
    }

    #[test]
    fn rapid_double_press_is_ignored_and_suppressed() {
        let now = base_time();
        let mut state = HotkeyState::default();
        state.press(Key::ControlLeft, now);
        state.press(Key::Space, now + Duration::from_millis(10));
        state.release(Key::Space);
        assert_eq!(
            state.press(Key::Space, now + Duration::from_millis(80)),
            (None, true)
        );
        assert_eq!(state.release(Key::Space), (None, true));
        assert!(!state.is_recording());
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
        assert!(!state.is_recording());
    }

    #[test]
    fn unrelated_keys_pass_through() {
        let now = base_time();
        let mut state = HotkeyState::default();
        assert_eq!(state.press(Key::KeyA, now), (None, false));
        assert_eq!(state.release(Key::KeyA), (None, false));
    }
}
