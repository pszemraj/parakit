//! Desktop hotkey, focus, session, and insertion helpers.

/// Global push-to-talk hotkey registration and event handling.
pub(crate) mod hotkey;
/// Text insertion into the currently focused desktop target.
pub(crate) mod inject;

#[cfg(target_os = "linux")]
/// Linux desktop session detection helpers.
pub(crate) mod session;
#[cfg(target_os = "linux")]
/// Windows Subsystem for Linux environment detection.
pub(crate) mod wsl;
#[cfg(target_os = "linux")]
/// X11 focus and hotkey integration.
pub(crate) mod x11;

#[cfg(target_os = "windows")]
/// Windows foreground-window detection helpers.
pub(crate) mod windows_focus;
#[cfg(target_os = "windows")]
/// Windows keyboard and clipboard insertion helpers.
pub(crate) mod windows_input;
#[cfg(target_os = "windows")]
/// Windows paste-path smoke checks.
pub(crate) mod windows_paste_smoke;
#[cfg(target_os = "windows")]
/// Windows security and process-context helpers.
pub(crate) mod windows_security;
