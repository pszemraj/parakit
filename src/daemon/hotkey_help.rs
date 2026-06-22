//! Shared user-facing hotkey diagnostics.

#[cfg(target_os = "linux")]
const REGISTERED_LINUX_FIX: &str = "fix:
  - Use an X11 session; Wayland is intentionally rejected.
  - Disable any desktop shortcut, input method, or remapper that already owns Ctrl+Space.
  - On GNOME/Ubuntu, check Settings > Keyboard > Keyboard Shortcuts > Typing/Input Sources, or run:
      gsettings get org.gnome.desktop.wm.keybindings switch-input-source
      gsettings get org.gnome.desktop.wm.keybindings switch-input-source-backward
  - Re-run: parakit doctor
  - The experimental evdev/uinput keyboard proxy is available with: parakit --hotkey-backend evdev-proxy";

#[cfg(target_os = "linux")]
const X11_LISTEN_LINUX_FIX: &str = "fix:
  - Use an X11 session; Wayland is intentionally rejected.
  - Re-run: parakit --hotkey-backend x11-listen
  - This backend passively listens only; it does not grab, suppress, or forward keyboard events.";

#[cfg(target_os = "linux")]
/// Append registered-X11 hotkey remediation steps.
///
/// # Arguments
///
/// * `out` - Diagnostic buffer to append to.
pub(crate) fn write_registered_linux_fix(out: &mut String) {
    push_line(out, REGISTERED_LINUX_FIX);
}

#[cfg(target_os = "linux")]
/// Append passive-X11 listener remediation steps.
///
/// # Arguments
///
/// * `out` - Diagnostic buffer to append to.
pub(crate) fn write_x11_listen_linux_fix(out: &mut String) {
    push_line(out, X11_LISTEN_LINUX_FIX);
}

#[cfg(target_os = "linux")]
/// Append evdev/uinput remediation steps.
///
/// # Arguments
///
/// * `out` - Diagnostic buffer to append to.
/// * `user` - Desktop user name to show in group-membership guidance.
pub(crate) fn write_evdev_linux_fix(out: &mut String, user: &str) {
    push_line(out, &evdev_linux_fix(user));
}

#[cfg(target_os = "linux")]
/// Build runtime failure help for the registered-X11 backend.
///
/// # Returns
///
/// Multi-line diagnostic text including the current session context.
pub(crate) fn registered_linux_failure_help() -> String {
    let (session, display) = linux_session_context();
    format!(
        "Linux default hotkey capture registers Ctrl+Space with the X11 session.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           parakit --verbose doctor\n\
           confirm no desktop shortcut or input method already owns Ctrl+Space\n\
         {REGISTERED_LINUX_FIX}"
    )
}

#[cfg(target_os = "linux")]
/// Build runtime failure help for the passive-X11 listener backend.
///
/// # Returns
///
/// Multi-line diagnostic text including the current session context.
pub(crate) fn x11_listen_linux_failure_help() -> String {
    let (session, display) = linux_session_context();
    format!(
        "The x11-listen backend passively observes Ctrl+Space with rdev::listen.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           parakit --verbose --hotkey-backend x11-listen doctor\n\
         {X11_LISTEN_LINUX_FIX}"
    )
}

#[cfg(target_os = "linux")]
/// Build runtime failure help for the evdev/uinput backend.
///
/// # Returns
///
/// Multi-line diagnostic text including the current session context.
pub(crate) fn evdev_linux_failure_help() -> String {
    let (session, display) = linux_session_context();
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    format!(
        "The evdev-proxy backend uses an evdev keyboard grab and uinput forwarding device.\n\
         Current session: XDG_SESSION_TYPE={session}, DISPLAY={display}\n\
         Checks:\n\
           id -nG | tr ' ' '\\n' | grep '^input$'\n\
           ls -l /dev/uinput /dev/input/event* | head\n\
         {}",
        evdev_linux_fix(&user)
    )
}

#[cfg(target_os = "macos")]
/// Shared macOS Accessibility remediation steps.
pub(crate) const MACOS_ACCESSIBILITY_FIX: &str = "fix:
  - Grant Accessibility to your terminal in System Settings > Privacy & Security > Accessibility.
  - Re-run: parakit doctor";

#[cfg(target_os = "macos")]
/// Append macOS Accessibility remediation steps.
///
/// # Arguments
///
/// * `out` - Diagnostic buffer to append to.
pub(crate) fn write_macos_accessibility_fix(out: &mut String) {
    push_line(out, MACOS_ACCESSIBILITY_FIX);
}

#[cfg(target_os = "macos")]
/// Build runtime failure help for the macOS event-tap backend.
///
/// # Returns
///
/// Multi-line diagnostic text explaining the Accessibility requirement.
pub(crate) fn macos_failure_help() -> String {
    format!(
        "macOS hotkey capture uses Left Control+Space and requires Accessibility for the terminal that launched parakit.\n{}",
        MACOS_ACCESSIBILITY_FIX
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

#[cfg(target_os = "linux")]
fn linux_session_context() -> (String, String) {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    (session, display)
}

#[cfg(target_os = "linux")]
fn evdev_linux_fix(user: &str) -> String {
    format!(
        "fix:\n  - Grant the desktop user read access to /dev/input/event*:\n      sudo usermod -aG input {user}\n  - Ensure /dev/uinput is writable by the desktop user. On many distros this needs a uinput udev rule.\n  - After changing groups or udev rules, log out completely and log back in, or reboot.\n  - Verify the fresh session:\n      id -nG | tr ' ' '\\n' | grep '^input$'\n      ls -l /dev/uinput /dev/input/event* | head\n  - Then run: parakit --hotkey-backend evdev-proxy\n  - Do not run parakit with sudo; audio, clipboard, and insertion belong to the desktop user."
    )
}
