//! Desktop hotkey, focus, session, and insertion helpers.

pub(crate) mod hotkey;
pub(crate) mod inject;

#[cfg(target_os = "linux")]
pub(crate) mod session;
#[cfg(target_os = "linux")]
pub(crate) mod wsl;
#[cfg(target_os = "linux")]
pub(crate) mod x11;

#[cfg(target_os = "windows")]
pub(crate) mod windows_focus;
#[cfg(target_os = "windows")]
pub(crate) mod windows_input;
#[cfg(target_os = "windows")]
pub(crate) mod windows_paste_smoke;
#[cfg(target_os = "windows")]
pub(crate) mod windows_security;
