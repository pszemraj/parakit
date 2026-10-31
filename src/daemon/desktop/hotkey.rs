//! Push-to-talk hotkey backend.
//!
//! Linux uses a registered global shortcut on X11 and focused-field IBus PTT
//! on Wayland. Passive X11 listening and the evdev proxy are explicit backends.

#[cfg(target_os = "windows")]
use crate::daemon::logging::Logger;
#[cfg(any(not(target_os = "windows"), test))]
use crate::daemon::recording::DictationMode;
use crate::daemon::recording::HotkeyTransition;
#[cfg(target_os = "linux")]
use anyhow::Context as _;
use crossbeam_channel::Sender;
#[cfg(all(target_os = "windows", test))]
use rdev::Key;
#[cfg(target_os = "macos")]
use rdev::Key;
#[cfg(target_os = "linux")]
use rdev::{EventType, Key};
use std::io::Write as _;
#[cfg(not(target_os = "macos"))]
use std::sync::Arc;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use std::sync::Mutex;
#[cfg(any(not(target_os = "windows"), test))]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

/// Virtual keycode for the configured macOS push-to-talk Control key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_PTT_LEFT_CONTROL_KEYCODE: u16 = 59;
/// Virtual keycode for the configured macOS push-to-talk Space key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_PTT_SPACE_KEYCODE: u16 = 49;
/// Virtual keycode for the macOS right Command key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_RIGHT_COMMAND_KEYCODE: u16 = 54;
/// Virtual keycode for the macOS left Command key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_LEFT_COMMAND_KEYCODE: u16 = 55;
/// Virtual keycode for the macOS left Shift key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_LEFT_SHIFT_KEYCODE: u16 = 56;
/// Virtual keycode for the macOS left Option key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_LEFT_OPTION_KEYCODE: u16 = 58;
/// Virtual keycode for the macOS right Shift key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_RIGHT_SHIFT_KEYCODE: u16 = 60;
/// Virtual keycode for the macOS right Option key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_RIGHT_OPTION_KEYCODE: u16 = 61;
/// Virtual keycode for the macOS right Control key.
#[cfg(target_os = "macos")]
pub(crate) const MACOS_RIGHT_CONTROL_KEYCODE: u16 = 62;

#[cfg(any(not(target_os = "windows"), test))]
const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(150);

/// Hotkey backend preference.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HotkeyBackend {
    /// Prefer the platform desktop hotkey backend.
    Auto,
    /// Force the platform desktop hotkey backend.
    Desktop,
    /// Use focused-field Ctrl+Space through the active Parakit IBus engine.
    #[cfg(target_os = "linux")]
    Ibus,
    /// Force the registered X11 global hotkey backend.
    #[cfg(target_os = "linux")]
    #[value(name = "x11-global-hotkey")]
    #[serde(rename = "x11-global-hotkey")]
    X11GlobalHotkey,
    /// Force the passive X11 event listener backend.
    #[cfg(target_os = "linux")]
    #[value(name = "x11-listen")]
    #[serde(rename = "x11-listen")]
    X11Listen,
    /// Force the experimental low-level evdev/uinput keyboard proxy backend.
    #[cfg(target_os = "linux")]
    #[value(name = "evdev-proxy-experimental")]
    #[serde(rename = "evdev-proxy-experimental")]
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
            Self::Ibus => "ibus",
            #[cfg(target_os = "linux")]
            Self::X11GlobalHotkey => "x11-global-hotkey",
            #[cfg(target_os = "linux")]
            Self::X11Listen => "x11-listen",
            #[cfg(target_os = "linux")]
            Self::EvdevProxyExperimental => "evdev-proxy-experimental",
        }
    }

    /// Resolve aliases to the Linux implementation that will actually run.
    ///
    /// # Returns
    ///
    /// The concrete input-method, registered-X11, passive-X11, or evdev route.
    #[cfg(target_os = "linux")]
    pub(crate) fn linux_route(self) -> LinuxHotkeyRoute {
        self.linux_route_for_session(super::session::is_wayland())
    }

    /// Resolve desktop aliases for the supplied display-session type.
    ///
    /// # Returns
    /// The selected native, X11, or experimental evdev route.
    #[cfg(target_os = "linux")]
    pub(crate) const fn linux_route_for_session(self, wayland: bool) -> LinuxHotkeyRoute {
        match self {
            Self::Auto | Self::Desktop if wayland => LinuxHotkeyRoute::Ibus,
            Self::Auto | Self::Desktop | Self::X11GlobalHotkey => LinuxHotkeyRoute::RegisteredX11,
            Self::Ibus => LinuxHotkeyRoute::Ibus,
            Self::X11Listen => LinuxHotkeyRoute::PassiveX11,
            Self::EvdevProxyExperimental => LinuxHotkeyRoute::EvdevProxy,
        }
    }
}

/// Concrete Linux hotkey implementation after resolving CLI/config aliases.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxHotkeyRoute {
    /// Focused-field Ctrl+Space from the selected Parakit input method.
    Ibus,
    /// Registered X11 `Ctrl+Space`.
    RegisteredX11,
    /// Passive X11 keyboard-event listening.
    PassiveX11,
    /// Experimental evdev grab plus uinput forwarding.
    EvdevProxy,
}

#[cfg(target_os = "linux")]
impl LinuxHotkeyRoute {
    /// Return the stable success/selection label for this route.
    ///
    /// # Returns
    ///
    /// A concise user-facing Linux backend description.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Ibus => "focused-field IBus Ctrl+Space",
            Self::RegisteredX11 => "registered X11 Ctrl+Space",
            Self::PassiveX11 => "passive X11 Ctrl+Space listen",
            Self::EvdevProxy => "experimental evdev/uinput keyboard proxy",
        }
    }
}

/// Return the user-facing default push-to-talk hotkey hint.
///
/// # Returns
///
/// A concise hotkey label for startup and documentation-facing diagnostics.
pub(crate) fn default_ptt_hint() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Left Control+Space"
    }

    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl+Space"
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(not(target_os = "windows"), test))]
enum HotkeyAction {
    Start {
        started_at: Instant,
        mode: DictationMode,
    },
    Stop {
        stopped_at: Instant,
    },
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg(any(not(target_os = "windows"), test))]
struct RecordingLatch {
    started_at: Option<Instant>,
}

#[cfg(any(not(target_os = "windows"), test))]
impl RecordingLatch {
    fn is_recording(&self) -> bool {
        self.started_at.is_some()
    }

    fn start(&mut self, now: Instant, mode: DictationMode) -> Option<HotkeyAction> {
        if self.is_recording() {
            return None;
        }
        self.started_at = Some(now);
        Some(HotkeyAction::Start {
            started_at: now,
            mode,
        })
    }

    fn stop(&mut self, stopped_at: Instant) -> Option<HotkeyAction> {
        self.started_at
            .take()
            .map(|_| HotkeyAction::Stop { stopped_at })
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg(any(not(target_os = "windows"), test))]
struct HotkeyState {
    context_enabled: bool,
    modifiers: ModifierState,
    space: bool,
    suppress_space_release: bool,
    recording: RecordingLatch,
    last_start: Option<Instant>,
}

#[cfg(any(not(target_os = "windows"), test))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ModifierState {
    ctrl_left: bool,
    ctrl_right: bool,
    shift_left: bool,
    shift_right: bool,
    alt: bool,
    alt_gr: bool,
    meta_left: bool,
    meta_right: bool,
}

#[cfg(any(not(target_os = "windows"), test))]
impl HotkeyState {
    fn new(context_enabled: bool) -> Self {
        Self {
            context_enabled,
            ..Self::default()
        }
    }

    fn press(&mut self, key: Key, now: Instant) -> (Option<HotkeyAction>, bool) {
        let space_was_held = self.space;
        self.set_key(key, true);
        match key {
            Key::Space if self.is_recording() || self.suppress_space_release => (None, true),
            Key::Space if space_was_held => (None, false),
            Key::Space if self.ctrl_only() || self.context_chord() => {
                self.suppress_space_release = true;
                let mode = if self.context_chord() {
                    DictationMode::WithContext
                } else {
                    DictationMode::Plain
                };
                (self.start_recording(now, mode), true)
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
            Key::ControlLeft | Key::ControlRight if was_recording && !self.ptt_ctrl_held() => {
                (self.stop_recording(now), false)
            }
            _ => (None, false),
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_sync_modifiers(
        &mut self,
        physical: ModifierState,
        now: Instant,
    ) -> Option<HotkeyAction> {
        let was_ctrl_left = self.modifiers.ctrl_left;
        self.modifiers = physical;
        if was_ctrl_left && !physical.ctrl_left {
            self.release(Key::ControlLeft, now).0
        } else {
            None
        }
    }

    #[cfg(target_os = "macos")]
    fn reset_after_tap_disabled(&mut self, now: Instant) -> Option<HotkeyAction> {
        self.modifiers = ModifierState::default();
        self.space = false;
        self.suppress_space_release = false;
        self.stop_recording(now)
    }

    fn start_recording(&mut self, now: Instant, mode: DictationMode) -> Option<HotkeyAction> {
        let debounce_ok = self
            .last_start
            .is_none_or(|last| now.duration_since(last) >= HOTKEY_DEBOUNCE);
        if !self.is_recording() && debounce_ok {
            self.last_start = Some(now);
            self.recording.start(now, mode)
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

    #[cfg(not(target_os = "macos"))]
    fn ctrl_held(&self) -> bool {
        self.modifiers.ctrl_left || self.modifiers.ctrl_right
    }

    fn ptt_ctrl_held(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.modifiers.ctrl_left
        }

        #[cfg(not(target_os = "macos"))]
        {
            self.ctrl_held()
        }
    }

    fn extra_modifier_held(&self) -> bool {
        self.modifiers.shift_left
            || self.modifiers.shift_right
            || self.modifiers.alt
            || self.modifiers.alt_gr
            || self.modifiers.meta_left
            || self.modifiers.meta_right
    }

    fn ctrl_only(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.modifiers.ctrl_left && !self.modifiers.ctrl_right && !self.extra_modifier_held()
        }

        #[cfg(not(target_os = "macos"))]
        {
            self.ctrl_held() && !self.extra_modifier_held()
        }
    }

    fn context_chord(&self) -> bool {
        self.context_enabled
            && self.ptt_ctrl_held()
            && self.modifiers.alt
            && !self.modifiers.alt_gr
            && !self.modifiers.shift_left
            && !self.modifiers.shift_right
            && !self.modifiers.meta_left
            && !self.modifiers.meta_right
            && (!cfg!(target_os = "macos") || !self.modifiers.ctrl_right)
    }

    fn set_key(&mut self, key: Key, pressed: bool) {
        match key {
            Key::ControlLeft => self.modifiers.ctrl_left = pressed,
            Key::ControlRight => self.modifiers.ctrl_right = pressed,
            Key::ShiftLeft => self.modifiers.shift_left = pressed,
            Key::ShiftRight => self.modifiers.shift_right = pressed,
            Key::Alt => self.modifiers.alt = pressed,
            Key::AltGr => self.modifiers.alt_gr = pressed,
            Key::MetaLeft => self.modifiers.meta_left = pressed,
            Key::MetaRight => self.modifiers.meta_right = pressed,
            Key::Space => self.space = pressed,
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{
    context_hotkey_probe, linux_device_has_ctrl_space, linux_event_device_paths,
    registered_hotkey_probe, run_grab_loop,
};

/// Run the platform hotkey loop until it ends or the process exits.
///
/// # Arguments
///
/// * `tx` - Coordinator channel used to post logical hotkey transitions.
/// * `_backend` - Ignored backend preference on Windows.
/// * `context_enabled` - Enable the additional Qwen context chord.
/// * `log` - Logger used for backend diagnostics.
///
/// # Returns
///
/// Success when the loop ends normally.
///
/// # Errors
///
/// Returns [`HotkeyLoopFailed`] after printing recovery help.
#[cfg(target_os = "windows")]
pub(crate) fn run_grab_loop(
    tx: Sender<HotkeyTransition>,
    _backend: HotkeyBackend,
    context_enabled: bool,
    log: Arc<Logger>,
) -> Result<(), HotkeyLoopFailed> {
    log.verbose("parakit: Windows hotkey backend: RegisterHotKey Ctrl+Space");
    report_hotkey_loop_failure(
        super::windows_input::run_registered_hotkey_loop(tx, context_enabled, &log),
        "Windows registered hotkey",
        crate::daemon::hotkey_help::windows_failure_help,
    )
}

#[cfg(target_os = "macos")]
pub(crate) use macos::run_grab_loop;

/// A hotkey backend failed after its recovery help was printed.
///
/// The daemon owner exits with status 2 once the worker has released any
/// native session, so process teardown never runs beside live device buffers.
#[derive(Debug)]
pub(crate) struct HotkeyLoopFailed;

/// Print platform-specific hotkey recovery help after a backend loop fails.
///
/// # Arguments
///
/// * `result` - Completed backend loop result.
/// * `backend` - Human-readable backend name for the error prefix.
/// * `help` - Lazy platform recovery guidance.
///
/// # Returns
///
/// Success when the backend loop ended normally.
///
/// # Errors
///
/// Returns [`HotkeyLoopFailed`] after printing the failure and help.
pub(super) fn report_hotkey_loop_failure(
    result: anyhow::Result<()>,
    backend: &str,
    help: impl FnOnce() -> String,
) -> Result<(), HotkeyLoopFailed> {
    result.map_err(|err| {
        let _ = writeln!(
            std::io::stderr(),
            "parakit: {backend} failed: {err:#}\n{}",
            help()
        );
        HotkeyLoopFailed
    })
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Eq, PartialEq)]
struct X11HotkeyMapping {
    space: Vec<u8>,
    control: Vec<u8>,
    alt_gr: Vec<u8>,
    needs_refresh: bool,
}

#[cfg(any(target_os = "linux", test))]
impl X11HotkeyMapping {
    #[cfg(target_os = "linux")]
    fn resolve(conn: &x11rb::rust_connection::RustConnection) -> anyhow::Result<Self> {
        let space = super::x11::keycodes_for_keysyms(conn, &[super::x11::SPACE_KEYSYM])
            .context("could not resolve X11 Space keycodes")?;
        let control = super::x11::keycodes_for_keysyms(
            conn,
            &[super::x11::CONTROL_L_KEYSYM, super::x11::CONTROL_R_KEYSYM],
        )
        .context("could not resolve X11 Control keycodes")?;
        let alt_gr = super::x11::keycodes_for_keysyms(conn, &[super::x11::ISO_LEVEL3_SHIFT_KEYSYM])
            .context("could not resolve X11 AltGr keycodes")?;
        anyhow::ensure!(!space.is_empty(), "could not resolve X11 Space keycode");
        anyhow::ensure!(!control.is_empty(), "could not resolve X11 Control keycode");
        Ok(Self {
            space,
            control,
            alt_gr,
            needs_refresh: false,
        })
    }

    fn refresh_with(
        &mut self,
        mapping_changed: bool,
        resolve: impl FnOnce() -> anyhow::Result<Self>,
    ) -> Option<anyhow::Error> {
        // MappingNotify is consumed before resolution. Keep it pending on failure
        // so the next physical-state query retries without needing another event.
        self.needs_refresh |= mapping_changed;
        if !self.needs_refresh {
            return None;
        }
        match resolve() {
            Ok(mapping) => {
                *self = mapping;
                self.needs_refresh = false;
                None
            }
            // Retries run on every hotkey poll. Report a failure once per
            // mapping change rather than at the poll rate.
            Err(err) => mapping_changed.then_some(err),
        }
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
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

#[cfg(any(not(target_os = "windows"), test))]
fn send_hotkey_transition(action: HotkeyAction, tx: &Sender<HotkeyTransition>) {
    let transition = match action {
        HotkeyAction::Start { started_at, mode } => HotkeyTransition::Pressed {
            at: started_at,
            mode,
        },
        HotkeyAction::Stop { stopped_at } => HotkeyTransition::Released { at: stopped_at },
    };
    let _ = tx.send(transition);
}

#[cfg(test)]
#[path = "hotkey_tests.rs"]
mod tests;
