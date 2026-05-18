//! Daemon-only subsystems used by the `parakit` binary.

pub(crate) mod audio;
pub(crate) mod desktop;
pub(crate) mod ipc;
pub(crate) mod logging;
pub(crate) mod notifications;
pub(crate) mod preflight;
pub(crate) mod recording;
pub(crate) mod sounds;
pub(crate) mod worker;

pub(crate) use desktop::{hotkey, inject};

#[cfg(target_os = "linux")]
pub(crate) use desktop::{session, wsl, x11};

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub(crate) use desktop::{windows_focus, windows_input, windows_paste_smoke, windows_security};
