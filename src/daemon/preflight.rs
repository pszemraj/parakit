//! Runtime preflight checks for desktop input and hotkey permissions.

use anyhow::{bail, Result};
use parakit::build_info;
use std::fmt::Write as _;

/// Run blocking daemon preflight checks before expensive startup work.
///
/// # Returns
///
/// Returns `Ok(())` when no blocking hotkey problem was detected.
///
/// # Errors
///
/// Returns an actionable error when the global hotkey backend is known to be
/// unavailable in the current desktop session.
pub fn ensure_hotkey_ready() -> Result<()> {
    let report = hotkey_report();
    if report.blocking {
        bail!("{}", report.summary);
    }
    Ok(())
}

/// Run hotkey diagnostics and return whether daemon startup should proceed.
///
/// # Returns
///
/// `true` when no blocking hotkey problem was detected.
pub fn print_doctor(verbose: bool) -> bool {
    let report = hotkey_report();
    if !verbose {
        return !report.blocking;
    }

    let mic = super::audio::probe_default_input();
    println!("{}", report.details.trim_end());
    match &mic {
        Ok(mic) => {
            println!("  mic:            {}", mic.summary());
            println!("  audio status:   OK");
        }
        Err(err) => {
            println!("  mic:            unavailable ({err:#})");
            println!("  audio status:   FAIL");
        }
    }
    println!("  build:");
    for line in build_info::diagnostic_lines() {
        println!("    {line}");
    }
    !report.blocking && mic.is_ok()
}

struct HotkeyReport {
    blocking: bool,
    summary: String,
    details: String,
}

#[cfg(target_os = "linux")]
fn hotkey_report() -> HotkeyReport {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    let evdev = evdev_report();
    let x11_candidate = linux_x11_desktop_hotkey_candidate();
    let x11_probe = if x11_candidate {
        probe_x11_desktop_hotkey()
    } else {
        Err("X11 desktop hotkey backend needs DISPLAY set and a non-Wayland session".to_string())
    };
    let evdev_available = evdev.readable > 0;
    let blocking = x11_probe.is_err() && !evdev_available;

    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(
        &mut details,
        "  session:        XDG_SESSION_TYPE={session}, DISPLAY={display}"
    )
    .unwrap();
    writeln!(&mut details, "  primary:        X11 desktop hotkey").unwrap();
    match &x11_probe {
        Ok(()) => writeln!(&mut details, "  primary status: OK").unwrap(),
        Err(err) => writeln!(&mut details, "  primary status: unavailable ({err})").unwrap(),
    }
    writeln!(&mut details, "  fallback:       rdev evdev grab").unwrap();
    writeln!(
        &mut details,
        "  input devices:  {} event device(s), {} readable, {} permission denied",
        evdev.event_devices, evdev.readable, evdev.denied
    )
    .unwrap();
    if !evdev.other_errors.is_empty() {
        writeln!(&mut details, "  input errors:").unwrap();
        for err in &evdev.other_errors {
            writeln!(&mut details, "    {err}").unwrap();
        }
    }

    if blocking {
        writeln!(&mut details, "  status:         FAIL").unwrap();
        write_linux_fix(&mut details, &user);
    } else if x11_probe.is_ok() {
        writeln!(
            &mut details,
            "  status:         OK (desktop hotkey backend)"
        )
        .unwrap();
    } else {
        writeln!(&mut details, "  status:         OK (evdev fallback)").unwrap();
    }

    let summary = if blocking {
        let mut summary = String::new();
        writeln!(&mut summary, "hotkey preflight failed before model startup").unwrap();
        writeln!(
            &mut summary,
            "session: XDG_SESSION_TYPE={session}, DISPLAY={display}"
        )
        .unwrap();
        writeln!(
            &mut summary,
            "primary backend: {}",
            x11_probe
                .as_ref()
                .map(|_| "OK".to_string())
                .unwrap_or_else(|err| format!("unavailable ({err})"))
        )
        .unwrap();
        writeln!(
            &mut summary,
            "evdev fallback: {} device(s), {} readable, {} permission denied",
            evdev.event_devices, evdev.readable, evdev.denied
        )
        .unwrap();
        write_linux_fix(&mut summary, &user);
        summary
    } else if x11_probe.is_ok() {
        "hotkey preflight passed with X11 desktop backend".to_string()
    } else {
        "hotkey preflight passed with evdev fallback".to_string()
    };

    HotkeyReport {
        blocking,
        summary,
        details,
    }
}

#[cfg(target_os = "linux")]
struct EvdevReport {
    event_devices: usize,
    readable: usize,
    denied: usize,
    other_errors: Vec<String>,
}

#[cfg(target_os = "linux")]
fn evdev_report() -> EvdevReport {
    use std::fs::{self, File};
    use std::io::ErrorKind;

    let mut event_devices = 0_usize;
    let mut readable = 0_usize;
    let mut denied = 0_usize;
    let mut other_errors = Vec::new();

    match fs::read_dir("/dev/input") {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
                {
                    continue;
                }

                event_devices += 1;
                match File::open(&path) {
                    Ok(_) => readable += 1,
                    Err(err) if err.kind() == ErrorKind::PermissionDenied => denied += 1,
                    Err(err) => other_errors.push(format!("{}: {err}", path.display())),
                }
            }
        }
        Err(err) => {
            other_errors.push(format!("/dev/input: {err}"));
        }
    }

    EvdevReport {
        event_devices,
        readable,
        denied,
        other_errors,
    }
}

#[cfg(target_os = "linux")]
/// Return whether the current environment can plausibly use X11 hotkeys.
///
/// # Returns
///
/// `true` when `DISPLAY` is set and the session is X11 or unspecified.
pub(crate) fn linux_x11_desktop_hotkey_candidate() -> bool {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let display = std::env::var("DISPLAY").unwrap_or_default();
    !display.is_empty() && (session.is_empty() || session.eq_ignore_ascii_case("x11"))
}

#[cfg(target_os = "linux")]
/// Return whether the rdev evdev fallback can open at least one event device.
///
/// # Returns
///
/// `true` when at least one `/dev/input/event*` device can be opened.
pub(crate) fn linux_evdev_fallback_available() -> bool {
    evdev_report().readable > 0
}

#[cfg(target_os = "linux")]
fn probe_x11_desktop_hotkey() -> std::result::Result<(), String> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::GlobalHotKeyManager;

    let manager = GlobalHotKeyManager::new().map_err(|err| err.to_string())?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL), Code::Space);
    manager.register(hotkey).map_err(|err| err.to_string())?;
    manager.unregister(hotkey).map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_linux_fix(out: &mut String, user: &str) {
    writeln!(
        out,
        "fix:\n  - Preferred: use an Xorg/X11 session and make sure Ctrl+Space is not already bound by the desktop or input method.\n  - Fallback: grant the desktop user read access to /dev/input/event*:\n      sudo usermod -aG input {user}\n  - After changing groups, log out completely and log back in, or reboot.\n  - Restart tmux, terminals, and user services started before the group change.\n  - Verify the fresh session:\n      id -nG | tr ' ' '\\n' | grep '^input$'\n  - Do not run parakit with sudo as the normal workaround."
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
fn hotkey_report() -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         manual check\n  fix: grant Accessibility and Input Monitoring permissions to both the terminal and the parakit binary.".to_string();
    HotkeyReport {
        blocking: false,
        summary: details.clone(),
        details,
    }
}

#[cfg(target_os = "windows")]
fn hotkey_report() -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         OK unless security software blocks the binary.".to_string();
    HotkeyReport {
        blocking: false,
        summary: details.clone(),
        details,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn hotkey_report() -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         unsupported platform preflight".to_string();
    HotkeyReport {
        blocking: false,
        summary: details.clone(),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_result_is_boolean() {
        let _ = print_doctor(false);
    }
}
