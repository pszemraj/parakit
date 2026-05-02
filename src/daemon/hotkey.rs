//! Push-to-talk hotkey backend.
//!
//! Linux defaults to a registered X11 desktop hotkey. Passive X11 listening
//! and the evdev/uinput keyboard proxy remain explicit non-default backends.

use super::{logging::Logger, recording::HotkeyTransition};
#[cfg(target_os = "linux")]
use anyhow::Context as _;
use crossbeam_channel::Sender;
#[cfg(target_os = "linux")]
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState as RegisteredHotKeyState,
};
use rdev::{Event, EventType, Key};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[cfg(target_os = "linux")]
use std::{fs::File, io, path::PathBuf};

const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(150);

/// Hotkey backend preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum HotkeyBackend {
    /// Prefer the platform desktop hotkey backend.
    Auto,
    /// Force the platform desktop hotkey backend.
    Desktop,
    /// Force the registered X11 global hotkey backend.
    #[cfg(target_os = "linux")]
    #[value(name = "x11-global-hotkey")]
    X11GlobalHotkey,
    /// Force the passive X11 event listener backend.
    #[cfg(target_os = "linux")]
    #[value(name = "x11-listen")]
    X11Listen,
    /// Force the experimental low-level evdev/uinput keyboard proxy backend.
    #[cfg(target_os = "linux")]
    #[value(name = "evdev-proxy-experimental", alias = "evdev-proxy")]
    EvdevProxyExperimental,
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
            #[cfg(target_os = "linux")]
            Self::X11GlobalHotkey => "x11-global-hotkey",
            #[cfg(target_os = "linux")]
            Self::X11Listen => "x11-listen",
            #[cfg(target_os = "linux")]
            Self::EvdevProxyExperimental => "evdev-proxy-experimental",
        }
    }

    #[cfg(target_os = "linux")]
    /// Return whether this Linux backend uses the registered X11 hotkey path.
    ///
    /// # Returns
    ///
    /// `true` for `auto`, `desktop`, and `x11-global-hotkey`.
    pub(crate) fn uses_registered_x11(self) -> bool {
        matches!(self, Self::Auto | Self::Desktop | Self::X11GlobalHotkey)
    }

    #[cfg(target_os = "linux")]
    /// Return whether this Linux backend passively listens for X11 events.
    ///
    /// # Returns
    ///
    /// `true` for `x11-listen`.
    pub(crate) fn uses_passive_x11_listen(self) -> bool {
        matches!(self, Self::X11Listen)
    }

    #[cfg(target_os = "linux")]
    /// Return whether this Linux backend uses the experimental evdev proxy.
    ///
    /// # Returns
    ///
    /// `true` for `evdev-proxy-experimental` and its explicit proxy alias.
    pub(crate) fn uses_evdev_proxy(self) -> bool {
        matches!(self, Self::EvdevProxyExperimental)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HotkeyAction {
    Start { started_at: Instant },
    Stop { stopped_at: Instant },
}

#[derive(Clone, Copy, Debug, Default)]
struct RecordingLatch {
    started_at: Option<Instant>,
}

impl RecordingLatch {
    fn is_recording(&self) -> bool {
        self.started_at.is_some()
    }

    fn start(&mut self, now: Instant) -> Option<HotkeyAction> {
        if self.is_recording() {
            return None;
        }
        self.started_at = Some(now);
        Some(HotkeyAction::Start { started_at: now })
    }

    fn stop(&mut self, stopped_at: Instant) -> Option<HotkeyAction> {
        self.started_at
            .take()
            .map(|_| HotkeyAction::Stop { stopped_at })
    }
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
    recording: RecordingLatch,
    last_start: Option<Instant>,
}

impl HotkeyState {
    fn press(&mut self, key: Key, now: Instant) -> (Option<HotkeyAction>, bool) {
        let space_was_held = self.space;
        self.set_key(key, true);
        match key {
            Key::Space if self.is_recording() || self.suppress_space_release || space_was_held => {
                (None, true)
            }
            Key::Space if self.ctrl_only() => {
                self.suppress_space_release = true;
                (self.start_recording(now), true)
            }
            _ => (None, false),
        }
    }

    fn release(&mut self, key: Key, now: Instant) -> (Option<HotkeyAction>, bool) {
        let was_recording = self.is_recording();
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
        if !self.is_recording() && debounce_ok {
            self.last_start = Some(now);
            self.recording.start(now)
        } else {
            None
        }
    }

    fn stop_recording(&mut self, stopped_at: Instant) -> Option<HotkeyAction> {
        self.recording.stop(stopped_at)
    }

    fn is_recording(&self) -> bool {
        self.recording.is_recording()
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
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
/// * `backend` - Linux backend preference.
/// * `log` - Logger used for backend diagnostics.
#[cfg(target_os = "linux")]
pub(crate) fn run_grab_loop(
    tx: Sender<HotkeyTransition>,
    backend: HotkeyBackend,
    log: Arc<Logger>,
) {
    match backend {
        HotkeyBackend::Auto | HotkeyBackend::Desktop | HotkeyBackend::X11GlobalHotkey => {
            log.verbose("parakit: Linux hotkey backend: registered X11 Ctrl+Space");
            run_linux_registered_hotkey_loop_or_exit(tx);
        }
        HotkeyBackend::X11Listen => {
            log.verbose("parakit: Linux hotkey backend: passive X11 Ctrl+Space listen");
            run_linux_x11_listen_or_exit(tx);
        }
        HotkeyBackend::EvdevProxyExperimental => {
            log.warn(
                "evdev-proxy is experimental; it grabs keyboard devices and forwards unsuppressed input through uinput",
            );
            log.verbose("parakit: Linux hotkey backend: experimental evdev/uinput keyboard proxy");
            run_linux_evdev_grab_loop_or_exit(tx, log);
        }
    }
}

/// Run the platform hotkey loop until the process exits.
///
/// # Arguments
///
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
/// * `_backend` - Ignored backend preference on platforms with one backend.
/// * `_log` - Logger unused on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub(crate) fn run_grab_loop(
    tx: Sender<HotkeyTransition>,
    _backend: HotkeyBackend,
    _log: Arc<Logger>,
) {
    run_rdev_grab_loop_or_exit(tx);
}

#[cfg(not(target_os = "linux"))]
fn run_rdev_grab_loop_or_exit(tx: Sender<HotkeyTransition>) {
    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let callback_state = Arc::clone(&state);
    let callback_tx = tx.clone();

    if let Err(e) = rdev::grab(move |event| handle_grab_event(event, &callback_state, &callback_tx))
    {
        eprintln!("parakit: rdev::grab failed: {e:?}\n{}", grab_failure_help());
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_linux_registered_hotkey_loop_or_exit(tx: Sender<HotkeyTransition>) {
    if let Err(err) = run_linux_registered_hotkey_loop(tx) {
        eprintln!(
            "parakit: registered X11 hotkey failed: {err:#}\n{}",
            registered_hotkey_failure_help()
        );
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_linux_x11_listen_or_exit(tx: Sender<HotkeyTransition>) {
    if let Err(err) = run_linux_x11_listen_loop(tx) {
        eprintln!(
            "parakit: passive X11 hotkey listen failed: {err:#}\n{}",
            x11_listen_failure_help()
        );
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_linux_x11_listen_loop(tx: Sender<HotkeyTransition>) -> anyhow::Result<()> {
    super::session::ensure_x11_session_supported()?;

    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let callback_state = Arc::clone(&state);
    let callback_tx = tx.clone();
    rdev::listen(move |event| handle_listen_event(event, &callback_state, &callback_tx))
        .map_err(|err| anyhow::anyhow!("rdev::listen: {err:?}"))
}

#[cfg(target_os = "linux")]
fn run_linux_registered_hotkey_loop(tx: Sender<HotkeyTransition>) -> anyhow::Result<()> {
    super::session::ensure_x11_session_supported()?;
    let manager =
        GlobalHotKeyManager::new().map_err(|err| anyhow::anyhow!("init hotkey manager: {err}"))?;
    let hotkey = ctrl_space_hotkey();
    manager
        .register(hotkey)
        .map_err(|err| anyhow::anyhow!("register Ctrl+Space: {err}"))?;

    let receiver = GlobalHotKeyEvent::receiver();
    let mut latch = RecordingLatch::default();
    let physical = X11PhysicalHotkeyProbe::open().ok();
    loop {
        let event = receiver
            .recv()
            .map_err(|err| anyhow::anyhow!("hotkey event channel closed: {err}"))?;
        if event.id != hotkey.id() {
            continue;
        }

        let now = Instant::now();
        let action = match event.state {
            RegisteredHotKeyState::Pressed => latch.start(now),
            RegisteredHotKeyState::Released => {
                if physical
                    .as_ref()
                    .and_then(|probe| probe.ctrl_space_down().ok())
                    .unwrap_or(false)
                {
                    continue;
                }
                latch.stop(now)
            }
        };
        if let Some(action) = action {
            send_hotkey_transition(action, &tx);
        }
    }
}

#[cfg(target_os = "linux")]
struct X11PhysicalHotkeyProbe {
    conn: x11rb::rust_connection::RustConnection,
    space: u8,
    ctrl_l: u8,
    ctrl_r: u8,
}

#[cfg(target_os = "linux")]
impl X11PhysicalHotkeyProbe {
    fn open() -> anyhow::Result<Self> {
        let (conn, _) = x11rb::rust_connection::RustConnection::connect(None)
            .context("could not connect to X11 for physical hotkey probe")?;
        let space = super::x11::keycode_for_keysym(&conn, super::x11::SPACE_KEYSYM)
            .context("could not resolve X11 Space keycode")?;
        let ctrl_l = super::x11::keycode_for_keysym(&conn, super::x11::CONTROL_L_KEYSYM)
            .context("could not resolve X11 Control_L keycode")?;
        let ctrl_r = super::x11::keycode_for_keysym(&conn, super::x11::CONTROL_R_KEYSYM)
            .context("could not resolve X11 Control_R keycode")?;

        Ok(Self {
            conn,
            space,
            ctrl_l,
            ctrl_r,
        })
    }

    fn ctrl_space_down(&self) -> anyhow::Result<bool> {
        use x11rb::protocol::xproto::ConnectionExt as _;

        let reply = self
            .conn
            .query_keymap()
            .context("could not query X11 keymap")?
            .reply()
            .context("could not read X11 keymap")?;

        Ok(keycode_down(&reply.keys, self.space)
            && (keycode_down(&reply.keys, self.ctrl_l) || keycode_down(&reply.keys, self.ctrl_r)))
    }
}

#[cfg(target_os = "linux")]
fn keycode_down(keys: &[u8; 32], keycode: u8) -> bool {
    let idx = usize::from(keycode / 8);
    let bit = keycode % 8;
    keys.get(idx).is_some_and(|byte| byte & (1_u8 << bit) != 0)
}

#[cfg(target_os = "linux")]
/// Probe whether the default registered `Ctrl+Space` hotkey can be claimed.
///
/// # Returns
///
/// `Ok(())` when the X11 session accepted the registration and unregister.
///
/// # Errors
///
/// Returns an error when X11 is unavailable or the hotkey is already owned.
pub(crate) fn registered_hotkey_probe() -> anyhow::Result<()> {
    super::session::ensure_x11_session_supported()?;
    let manager =
        GlobalHotKeyManager::new().map_err(|err| anyhow::anyhow!("init hotkey manager: {err}"))?;
    let hotkey = ctrl_space_hotkey();
    manager
        .register(hotkey)
        .map_err(|err| anyhow::anyhow!("register Ctrl+Space: {err}"))?;
    manager
        .unregister(hotkey)
        .map_err(|err| anyhow::anyhow!("unregister Ctrl+Space: {err}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn ctrl_space_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL), Code::Space)
}

#[cfg(target_os = "linux")]
fn run_linux_evdev_grab_loop_or_exit(tx: Sender<HotkeyTransition>, log: Arc<Logger>) {
    if let Err(err) = run_linux_evdev_grab_loop(tx, Arc::clone(&log)) {
        eprintln!(
            "parakit: evdev keyboard grab failed: {err:#}\n{}",
            grab_failure_help()
        );
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn run_linux_evdev_grab_loop(tx: Sender<HotkeyTransition>, log: Arc<Logger>) -> io::Result<()> {
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

    let result = linux_evdev_event_loop(epoll_fd, &mut grabbed, &state, &tx);

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
    tx: &Sender<HotkeyTransition>,
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

                let suppress = linux_evdev_event_suppressed(&input_event, state, tx);
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
    tx: &Sender<HotkeyTransition>,
) -> bool {
    let Some(event_type) = linux_evdev_key_event_type(event) else {
        return false;
    };

    handle_key_event(event_type, state, tx)
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

        if !linux_device_has_ctrl_space(&device) {
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
/// Return whether an evdev device can produce the configured Ctrl+Space chord.
///
/// # Returns
///
/// `true` when the device advertises Space and either Ctrl key.
pub(crate) fn linux_device_has_ctrl_space(device: &evdev_rs::Device) -> bool {
    use evdev_rs::enums::{EventCode, EV_KEY};

    let has_space = device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_SPACE));
    let has_ctrl = device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_LEFTCTRL))
        || device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_RIGHTCTRL));
    has_space && has_ctrl
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
/// Return sorted Linux evdev event device paths.
///
/// # Returns
///
/// Paths named `event*` under `/dev/input`.
///
/// # Errors
///
/// Returns an error if `/dev/input` cannot be read.
pub(crate) fn linux_event_device_paths() -> io::Result<Vec<PathBuf>> {
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

#[cfg(target_os = "linux")]
fn handle_listen_event(
    event: Event,
    state: &Arc<Mutex<HotkeyState>>,
    tx: &Sender<HotkeyTransition>,
) {
    let _ = handle_key_event(event.event_type, state, tx);
}

#[cfg(not(target_os = "linux"))]
fn handle_grab_event(
    event: Event,
    state: &Arc<Mutex<HotkeyState>>,
    tx: &Sender<HotkeyTransition>,
) -> Option<Event> {
    let suppress = handle_key_event(event.event_type, state, tx);
    if suppress {
        None
    } else {
        Some(event)
    }
}

fn handle_key_event(
    event_type: EventType,
    state: &Arc<Mutex<HotkeyState>>,
    tx: &Sender<HotkeyTransition>,
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
        send_hotkey_transition(action, tx);
    }

    suppress
}

fn send_hotkey_transition(action: HotkeyAction, tx: &Sender<HotkeyTransition>) {
    let transition = match action {
        HotkeyAction::Start { started_at } => HotkeyTransition::Pressed { at: started_at },
        HotkeyAction::Stop { stopped_at } => HotkeyTransition::Released { at: stopped_at },
    };
    let _ = tx.send(transition);
}

#[cfg(target_os = "linux")]
fn registered_hotkey_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());

    format!(
        "Linux default hotkey capture registers Ctrl+Space with the X11 session.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           parakit --verbose doctor\n\
           confirm no desktop shortcut or input method already owns Ctrl+Space\n\
         Use an X11 session. Wayland is intentionally rejected.\n\
         The experimental evdev/uinput keyboard proxy is available with --hotkey-backend evdev-proxy."
    )
}

#[cfg(target_os = "linux")]
fn x11_listen_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());

    format!(
        "The x11-listen backend passively observes Ctrl+Space with rdev::listen.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           parakit --verbose doctor --hotkey-backend x11-listen\n\
         Use an X11 session. Wayland is intentionally rejected.\n\
         This backend does not grab, suppress, or forward keyboard events."
    )
}

#[cfg(target_os = "linux")]
fn grab_failure_help() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

    format!(
        "The evdev-proxy backend uses an evdev keyboard grab and uinput forwarding device.\n\
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
#[path = "hotkey_tests.rs"]
mod tests;
