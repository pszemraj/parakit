//! Runtime preflight checks for desktop input permissions.

use anyhow::{bail, Result};
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
    if verbose {
        println!("{}", report.details);
    }
    !report.blocking
}

struct HotkeyReport {
    blocking: bool,
    summary: String,
    details: String,
}

#[cfg(target_os = "linux")]
fn hotkey_report() -> HotkeyReport {
    use std::fs::{self, File};
    use std::io::ErrorKind;

    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());

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

    let wayland = session.eq_ignore_ascii_case("wayland");
    let missing_display = display == "<unset>";
    let no_events = event_devices == 0 && other_errors.is_empty();
    let permission_blocked = event_devices > 0 && denied > 0;
    let blocking = wayland || missing_display || no_events || permission_blocked;

    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(&mut details, "  hotkey backend: rdev::grab").unwrap();
    writeln!(
        &mut details,
        "  session:        XDG_SESSION_TYPE={session}, DISPLAY={display}"
    )
    .unwrap();
    writeln!(
        &mut details,
        "  input devices:  {event_devices} event device(s), {readable} readable, {denied} permission denied"
    )
    .unwrap();
    if !other_errors.is_empty() {
        writeln!(&mut details, "  input errors:").unwrap();
        for err in &other_errors {
            writeln!(&mut details, "    {err}").unwrap();
        }
    }

    if blocking {
        writeln!(&mut details, "  status:         FAIL").unwrap();
        write_linux_fix(&mut details, &user);
    } else {
        writeln!(&mut details, "  status:         OK").unwrap();
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
            "/dev/input/event*: {event_devices} device(s), {readable} readable, {denied} permission denied"
        )
        .unwrap();
        write_linux_fix(&mut summary, &user);
        summary
    } else {
        "hotkey preflight passed".to_string()
    };

    HotkeyReport {
        blocking,
        summary,
        details,
    }
}

#[cfg(target_os = "linux")]
fn write_linux_fix(out: &mut String, user: &str) {
    writeln!(
        out,
        "fix:\n  - Use an Xorg/X11 session, not Wayland.\n  - Grant the desktop user read access to /dev/input/event*:\n      sudo usermod -aG input {user}\n  - Log out completely and log back in, or reboot.\n  - Restart tmux, terminals, and user services started before the group change.\n  - Verify the fresh session:\n      id -nG | tr ' ' '\\n' | grep '^input$'\n  - Do not run parakit with sudo as the normal workaround."
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
