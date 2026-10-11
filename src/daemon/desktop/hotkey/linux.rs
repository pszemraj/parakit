//! Linux push-to-talk hotkey backends: registered X11, passive X11 listen, and evdev proxy.

use super::{
    handle_key_event, report_hotkey_loop_failure, send_hotkey_transition, HotkeyAction,
    HotkeyBackend, HotkeyLoopFailed, HotkeyState, LinuxHotkeyRoute, RecordingLatch,
    X11HotkeyMapping,
};
use crate::daemon::logging::Logger;
use crate::daemon::recording::HotkeyTransition;
use anyhow::Context as _;
use crossbeam_channel::{RecvTimeoutError, Sender};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState as RegisteredHotKeyState,
};
use rdev::{Event, EventType, Key};
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs::File, io, path::PathBuf};

const REGISTERED_HOTKEY_PHYSICAL_POLL: Duration = Duration::from_millis(25);

/// Run the platform hotkey loop until it ends or the process exits.
///
/// # Arguments
///
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
/// * `backend` - Linux backend preference.
/// * `log` - Logger used for backend diagnostics.
///
/// # Returns
///
/// Success when the loop ends normally.
///
/// # Errors
///
/// Returns [`HotkeyLoopFailed`] after printing recovery help.
pub(crate) fn run_grab_loop(
    tx: Sender<HotkeyTransition>,
    backend: HotkeyBackend,
    log: Arc<Logger>,
) -> Result<(), HotkeyLoopFailed> {
    let route = backend.linux_route();
    if route == LinuxHotkeyRoute::EvdevProxy {
        log.warn(
            "evdev-proxy-experimental grabs keyboard devices and forwards unsuppressed input through uinput",
        );
    }
    log.verbose(format!("parakit: Linux hotkey backend: {}", route.label()));
    match route {
        LinuxHotkeyRoute::RegisteredX11 => report_hotkey_loop_failure(
            run_linux_registered_hotkey_loop(tx),
            "registered X11 hotkey",
            crate::daemon::hotkey_help::registered_linux_failure_help,
        ),
        LinuxHotkeyRoute::PassiveX11 => report_hotkey_loop_failure(
            run_linux_x11_listen_loop(tx),
            "passive X11 hotkey listen",
            crate::daemon::hotkey_help::x11_listen_linux_failure_help,
        ),
        LinuxHotkeyRoute::EvdevProxy => report_hotkey_loop_failure(
            run_linux_evdev_grab_loop(tx, Arc::clone(&log)).map_err(Into::into),
            "evdev keyboard grab",
            crate::daemon::hotkey_help::evdev_linux_failure_help,
        ),
    }
}

fn run_linux_x11_listen_loop(tx: Sender<HotkeyTransition>) -> anyhow::Result<()> {
    super::super::session::ensure_x11_session_supported()?;

    let state = Arc::new(Mutex::new(HotkeyState::default()));
    let callback_state = Arc::clone(&state);
    let callback_tx = tx.clone();
    rdev::listen(move |event| handle_listen_event(event, &callback_state, &callback_tx))
        .map_err(|err| anyhow::anyhow!("rdev::listen: {err:?}"))
}

fn run_linux_registered_hotkey_loop(tx: Sender<HotkeyTransition>) -> anyhow::Result<()> {
    super::super::session::ensure_x11_session_supported()?;
    let manager =
        GlobalHotKeyManager::new().map_err(|err| anyhow::anyhow!("init hotkey manager: {err}"))?;
    let hotkey = ctrl_space_hotkey();
    manager
        .register(hotkey)
        .map_err(|err| anyhow::anyhow!("register Ctrl+Space: {err}"))?;

    let receiver = GlobalHotKeyEvent::receiver();
    let mut latch = RegisteredHotkeyLatch::default();
    let mut physical = X11PhysicalHotkeyProbe::open()
        .context("could not initialize physical Ctrl+Space state probe")?;
    loop {
        let event = if latch.needs_physical_poll() {
            match receiver.recv_timeout(REGISTERED_HOTKEY_PHYSICAL_POLL) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    let physical_state = physical_hotkey_state(&mut physical)?;
                    if let Some(action) = latch.physical_poll(physical_state, now) {
                        send_hotkey_transition(action, &tx);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(anyhow::anyhow!("hotkey event channel closed"));
                }
            }
        } else {
            receiver
                .recv()
                .map_err(|err| anyhow::anyhow!("hotkey event channel closed: {err}"))?
        };
        if event.id != hotkey.id() {
            continue;
        }

        let now = Instant::now();
        let action = latch.event(event.state, physical_hotkey_state(&mut physical)?, now);
        if let Some(action) = action {
            send_hotkey_transition(action, &tx);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RegisteredHotkeyLatch {
    recording: RecordingLatch,
    suppress_until_space_release: bool,
}

impl RegisteredHotkeyLatch {
    fn needs_physical_poll(&self) -> bool {
        self.recording.is_recording() || self.suppress_until_space_release
    }

    fn is_recording(&self) -> bool {
        self.recording.is_recording()
    }

    fn event(
        &mut self,
        state: RegisteredHotKeyState,
        physical: PhysicalHotkeyState,
        now: Instant,
    ) -> Option<HotkeyAction> {
        self.sync_space_release(physical);
        match state {
            RegisteredHotKeyState::Pressed if self.suppress_until_space_release => None,
            RegisteredHotKeyState::Pressed => self.recording.start(now),
            RegisteredHotKeyState::Released => {
                if physical.chord_down() {
                    return None;
                }
                if physical.space {
                    self.suppress_until_space_release = true;
                }
                self.recording.stop(now)
            }
        }
    }

    fn physical_poll(
        &mut self,
        physical: PhysicalHotkeyState,
        now: Instant,
    ) -> Option<HotkeyAction> {
        self.sync_space_release(physical);
        match self.is_recording() && !physical.chord_down() {
            true => {
                if physical.space {
                    self.suppress_until_space_release = true;
                }
                self.recording.stop(now)
            }
            false => None,
        }
    }

    fn sync_space_release(&mut self, physical: PhysicalHotkeyState) {
        if !physical.space {
            self.suppress_until_space_release = false;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PhysicalHotkeyState {
    ctrl: bool,
    space: bool,
}

impl PhysicalHotkeyState {
    fn chord_down(self) -> bool {
        self.ctrl && self.space
    }
}

fn physical_hotkey_state(
    physical: &mut X11PhysicalHotkeyProbe,
) -> anyhow::Result<PhysicalHotkeyState> {
    // A failed probe is unknown state, not a physical release. Propagate it
    // through the loop so the daemon shuts down after releasing worker resources
    // rather than submitting an in-progress recording on an invented key-up.
    physical
        .state()
        .context("could not refresh physical Ctrl+Space state")
}

struct X11PhysicalHotkeyProbe {
    conn: x11rb::rust_connection::RustConnection,
    mapping: X11HotkeyMapping,
}

impl X11PhysicalHotkeyProbe {
    fn open() -> anyhow::Result<Self> {
        let (conn, _) = x11rb::rust_connection::RustConnection::connect(None)
            .context("could not connect to X11 for physical hotkey probe")?;
        let mapping = X11HotkeyMapping::resolve(&conn)?;

        Ok(Self { conn, mapping })
    }

    fn state(&mut self) -> anyhow::Result<PhysicalHotkeyState> {
        use x11rb::protocol::xproto::ConnectionExt as _;

        // Each X11 connection owns its MappingNotify queue and mapping cache;
        // the paste connection cannot share this refresh.
        let mapping_changed = super::super::x11::mapping_changed(&self.conn)?;
        if let Some(err) = self
            .mapping
            .refresh_with(mapping_changed, || X11HotkeyMapping::resolve(&self.conn))
        {
            let _ =
                writeln!(std::io::stderr(),
                "parakit: could not refresh X11 hotkey mapping; keeping previous keycodes: {err:#}"
            );
        }

        let reply = self
            .conn
            .query_keymap()
            .context("could not query X11 keymap")?
            .reply()
            .context("could not read X11 keymap")?;

        Ok(physical_state_from_keycodes(
            &reply.keys,
            &self.mapping.control,
            &self.mapping.space,
        ))
    }
}

fn physical_state_from_keycodes(
    keys: &[u8; 32],
    control: &[u8],
    space: &[u8],
) -> PhysicalHotkeyState {
    PhysicalHotkeyState {
        ctrl: control
            .iter()
            .any(|keycode| super::super::x11::keycode_down(keys, *keycode)),
        space: space
            .iter()
            .any(|keycode| super::super::x11::keycode_down(keys, *keycode)),
    }
}

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
    super::super::session::ensure_x11_session_supported()?;
    let _physical = X11PhysicalHotkeyProbe::open()
        .context("could not initialize physical Ctrl+Space state probe")?;
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

fn ctrl_space_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL), Code::Space)
}

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

struct LinuxKeyboardDevice {
    label: String,
    device: evdev_rs::Device,
    output: evdev_rs::UInputDevice,
}

impl LinuxKeyboardDevice {
    fn raw_fd(&self) -> io::Result<std::os::fd::RawFd> {
        use std::os::fd::IntoRawFd;

        self.device
            .fd()
            .map(IntoRawFd::into_raw_fd)
            .ok_or_else(|| io::Error::other(format!("{} has no file descriptor", self.label)))
    }
}

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

fn open_evdev_input(path: &std::path::Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

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

fn linux_device_label(path: &std::path::Path, device: &evdev_rs::Device) -> String {
    match device.name() {
        Some(name) if !name.is_empty() => format!("{} ({name})", path.display()),
        _ => path.display().to_string(),
    }
}

fn handle_listen_event(
    event: Event,
    state: &Arc<Mutex<HotkeyState>>,
    tx: &Sender<HotkeyTransition>,
) {
    let _ = handle_key_event(event.event_type, state, tx);
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
