//! Push-to-talk hotkey backend.
//!
//! Linux v1 uses a narrow evdev grab for keyboard hotkey capture.
//! The custom X11 `XGrabKey` backend is intentionally out of the daemon
//! critical path; X11 remains only for insertion support.

use super::{audio::AudioHandle, logging::Logger};
use crate::Event_;
use crossbeam_channel::Sender;
#[cfg(not(target_os = "linux"))]
use rdev::Event;
use rdev::{EventType, Key};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[cfg(target_os = "linux")]
use std::{fs::File, io, path::PathBuf};

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
    Start,
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
            Some(HotkeyAction::Start)
        } else {
            None
        }
    }

    fn stop_recording(&mut self, stopped_at: Instant) -> Option<HotkeyAction> {
        if !self.recording {
            return None;
        }

        self.recording = false;
        let started_at = self
            .started_at
            .take()
            .expect("recording state requires a start timestamp");
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
            Key::Space => {}
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
            log.verbose("parakit: Linux hotkey backend: evdev keyboard grab");
            run_linux_evdev_grab_loop_or_exit(tx, audio, log);
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

#[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "linux")]
fn run_linux_evdev_grab_loop_or_exit(tx: Sender<Event_>, audio: AudioHandle, log: Arc<Logger>) {
    if let Err(err) = run_linux_evdev_grab_loop(tx, audio, Arc::clone(&log)) {
        eprintln!(
            "parakit: evdev keyboard grab failed: {err:#}\n{}",
            grab_failure_help()
        );
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_linux_evdev_grab_loop(
    tx: Sender<Event_>,
    audio: AudioHandle,
    log: Arc<Logger>,
) -> io::Result<()> {
    let mut devices = open_keyboard_devices(&log)?;
    if devices.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no readable Ctrl+Space keyboard event devices found",
        ));
    }

    let mut grabbed = Vec::new();
    let mut skipped_busy = Vec::new();
    for mut device in devices.drain(..) {
        match device.device.grab(evdev_rs::GrabMode::Grab) {
            Ok(()) => grabbed.push(device),
            Err(err) if err.kind() == io::ErrorKind::ResourceBusy => {
                skipped_busy.push(device.label);
            }
            Err(err) => {
                return Err(io::Error::new(
                    err.kind(),
                    format!("could not grab {}: {err}", device.label),
                ));
            }
        }
    }

    if !skipped_busy.is_empty() {
        log.verbose(format!(
            "parakit: skipped busy keyboard device(s): {}",
            skipped_busy.join(", ")
        ));
    }

    if grabbed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            format!(
                "all Ctrl+Space keyboard event devices are already grabbed: {}",
                skipped_busy.join(", ")
            ),
        ));
    }

    log.verbose(format!(
        "parakit: grabbed keyboard event device(s): {}",
        grabbed
            .iter()
            .map(|device| device.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let epoll_fd = epoll::create(true)?;
    for (idx, device) in grabbed.iter().enumerate() {
        let fd = device.raw_fd()?;
        epoll::ctl(
            epoll_fd,
            epoll::ControlOptions::EPOLL_CTL_ADD,
            fd,
            epoll::Event::new(epoll::Events::EPOLLIN, idx as u64),
        )?;
    }

    let result = linux_evdev_event_loop(epoll_fd, &mut grabbed, &state, &audio, &tx);

    for device in &mut grabbed {
        let _ = device.device.grab(evdev_rs::GrabMode::Ungrab);
    }
    let _ = epoll::close(epoll_fd);

    result
}

#[cfg(target_os = "linux")]
struct LinuxKeyboardDevice {
    label: String,
    device: evdev_rs::Device,
    output: evdev_rs::UInputDevice,
}

#[cfg(target_os = "linux")]
impl LinuxKeyboardDevice {
    fn raw_fd(&self) -> io::Result<std::os::fd::RawFd> {
        use std::os::fd::IntoRawFd;

        self.device
            .fd()
            .map(IntoRawFd::into_raw_fd)
            .ok_or_else(|| io::Error::other(format!("{} has no file descriptor", self.label)))
    }
}

#[cfg(target_os = "linux")]
fn linux_evdev_event_loop(
    epoll_fd: std::os::fd::RawFd,
    devices: &mut [LinuxKeyboardDevice],
    state: &Arc<Mutex<HotkeyState>>,
    audio: &AudioHandle,
    tx: &Sender<Event_>,
) -> io::Result<()> {
    let mut epoll_buffer = [epoll::Event::new(epoll::Events::empty(), 0); 8];
    loop {
        let num_events = epoll::wait(epoll_fd, -1, &mut epoll_buffer)?;
        for event in &epoll_buffer[..num_events] {
            let idx = event.data as usize;
            let Some(device) = devices.get_mut(idx) else {
                continue;
            };

            while device.device.has_event_pending() {
                let (_, input_event) = match device.device.next_event(evdev_rs::ReadFlag::NORMAL) {
                    Ok(event) => event,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => return Err(err),
                };

                let suppress = linux_evdev_event_suppressed(&input_event, state, audio, tx);
                if !suppress {
                    device.output.write_event(&input_event)?;
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_evdev_event_suppressed(
    event: &evdev_rs::InputEvent,
    state: &Arc<Mutex<HotkeyState>>,
    audio: &AudioHandle,
    tx: &Sender<Event_>,
) -> bool {
    let Some(event_type) = linux_evdev_key_event_type(event) else {
        return false;
    };

    handle_key_event(event_type, state, audio, tx)
}

#[cfg(target_os = "linux")]
fn linux_evdev_key_event_type(event: &evdev_rs::InputEvent) -> Option<EventType> {
    use evdev_rs::enums::EventCode;

    let key = match &event.event_code {
        EventCode::EV_KEY(key) => linux_evdev_key_to_rdev(key.clone())?,
        _ => return None,
    };
    match event.value {
        0 => Some(EventType::KeyRelease(key)),
        1 | 2 => Some(EventType::KeyPress(key)),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn linux_evdev_key_to_rdev(key: evdev_rs::enums::EV_KEY) -> Option<Key> {
    use evdev_rs::enums::EV_KEY;

    match key {
        EV_KEY::KEY_LEFTCTRL => Some(Key::ControlLeft),
        EV_KEY::KEY_RIGHTCTRL => Some(Key::ControlRight),
        EV_KEY::KEY_LEFTSHIFT => Some(Key::ShiftLeft),
        EV_KEY::KEY_RIGHTSHIFT => Some(Key::ShiftRight),
        EV_KEY::KEY_LEFTALT => Some(Key::Alt),
        EV_KEY::KEY_RIGHTALT => Some(Key::AltGr),
        EV_KEY::KEY_LEFTMETA => Some(Key::MetaLeft),
        EV_KEY::KEY_RIGHTMETA => Some(Key::MetaRight),
        EV_KEY::KEY_SPACE => Some(Key::Space),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn open_keyboard_devices(log: &Logger) -> io::Result<Vec<LinuxKeyboardDevice>> {
    use evdev_rs::enums::{EventCode, EV_KEY};

    let mut out = Vec::new();
    for path in linux_event_device_paths()? {
        let file = match open_evdev_input(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(err) => return Err(err),
        };
        let device = match evdev_rs::Device::new_from_fd(file) {
            Ok(device) => device,
            Err(err) => {
                log.verbose(format!("parakit: skipped {} ({err})", path.display()));
                continue;
            }
        };

        let has_space = device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_SPACE));
        let has_ctrl = device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_LEFTCTRL))
            || device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_RIGHTCTRL));
        if !has_space || !has_ctrl {
            continue;
        }

        let label = linux_device_label(&path, &device);
        let output = evdev_rs::UInputDevice::create_from_device(&device).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("could not create uinput forwarding device for {label}: {err}"),
            )
        })?;
        out.push(LinuxKeyboardDevice {
            label,
            device,
            output,
        });
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
fn open_evdev_input(path: &std::path::Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(target_os = "linux")]
fn linux_event_device_paths() -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir("/dev/input")? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(target_os = "linux")]
fn linux_device_label(path: &std::path::Path, device: &evdev_rs::Device) -> String {
    match device.name() {
        Some(name) if !name.is_empty() => format!("{} ({name})", path.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(not(target_os = "linux"))]
fn handle_grab_event(
    event: Event,
    state: &Arc<Mutex<HotkeyState>>,
    audio: &AudioHandle,
    tx: &Sender<Event_>,
) -> Option<Event> {
    let suppress = handle_key_event(event.event_type, state, audio, tx);
    if suppress {
        None
    } else {
        Some(event)
    }
}

fn handle_key_event(
    event_type: EventType,
    state: &Arc<Mutex<HotkeyState>>,
    audio: &AudioHandle,
    tx: &Sender<Event_>,
) -> bool {
    let now = Instant::now();
    let (action, suppress) = match event_type {
        EventType::KeyPress(key) => state
            .lock()
            .expect("hotkey state lock poisoned")
            .press(key, now),
        EventType::KeyRelease(key) => state
            .lock()
            .expect("hotkey state lock poisoned")
            .release(key, now),
        _ => return false,
    };

    if let Some(action) = action {
        dispatch_hotkey_action(action, audio, tx);
    }

    suppress
}

fn dispatch_hotkey_action(action: HotkeyAction, audio: &AudioHandle, tx: &Sender<Event_>) {
    match action {
        HotkeyAction::Start => {
            audio.start_recording();
            let _ = tx.send(Event_::RecordingStarted);
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
        "Linux hotkey capture uses an evdev keyboard grab.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           id -nG | tr ' ' '\\n' | grep '^input$'\n\
           ls -l /dev/uinput /dev/input/event* | head\n\
         If event devices are not readable, run:\n\
           sudo usermod -aG input {user}\n\
         Then log out completely and log back in, or reboot.\n\
         If /dev/uinput is not writable by your user, add a uinput udev rule.\n\
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
            (Some(HotkeyAction::Start), true)
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
            (Some(HotkeyAction::Start), true)
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

    #[cfg(target_os = "linux")]
    #[test]
    fn evdev_input_files_are_opened_nonblocking() {
        use std::os::fd::AsRawFd;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before UNIX epoch")
            .as_nanos();
        let dir = PathBuf::from(format!(
            "target/tmp/parakit-hotkey-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test directory");
        let path = dir.join("event-test");
        std::fs::write(&path, b"").expect("create test input file");

        let file = open_evdev_input(&path).expect("open test input file");
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags, -1);
        assert_ne!(flags & libc::O_NONBLOCK, 0);
    }
}
