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
    let xauthority = std::env::var("XAUTHORITY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    let evdev = evdev_report();
    let x11_candidate = linux_x11_desktop_hotkey_candidate();
    let x11_probe = if x11_candidate {
        probe_x11_desktop_hotkey()
    } else {
        Err("X11 desktop hotkey backend needs DISPLAY set and a non-Wayland session".to_string())
    };
    let evdev_ready = evdev.grab_likely_available();
    let running_parakit = running_parakit_processes();
    let x11_owned_by_parakit = x11_probe
        .as_ref()
        .err()
        .is_some_and(|err| err.contains("XGrabKey rejected Ctrl+Space"))
        && !running_parakit.is_empty();
    let blocking = x11_probe.is_err() && !x11_owned_by_parakit && !evdev_ready;

    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(
        &mut details,
        "  session:        XDG_SESSION_TYPE={session}, DISPLAY={display}"
    )
    .unwrap();
    writeln!(&mut details, "  xauthority:     {xauthority}").unwrap();
    writeln!(&mut details, "  desktop:        X11 desktop hotkey").unwrap();
    match &x11_probe {
        Ok(()) => writeln!(&mut details, "  desktop status: OK").unwrap(),
        Err(_) if x11_owned_by_parakit => writeln!(
            &mut details,
            "  desktop status: OK (Ctrl+Space already owned by running parakit pid(s): {})",
            format_pids(&running_parakit)
        )
        .unwrap(),
        Err(err) => writeln!(&mut details, "  desktop status: unavailable ({err})").unwrap(),
    }
    writeln!(
        &mut details,
        "  evdev:          rdev grab ({})",
        evdev.status_label()
    )
    .unwrap();
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
    } else if evdev_ready {
        writeln!(
            &mut details,
            "  status:         OK (evdev backend preferred)"
        )
        .unwrap();
    } else if x11_owned_by_parakit {
        writeln!(
            &mut details,
            "  status:         OK (running daemon owns desktop hotkey)"
        )
        .unwrap();
    } else if x11_probe.is_ok() {
        writeln!(
            &mut details,
            "  status:         OK (desktop hotkey backend)"
        )
        .unwrap();
    } else {
        writeln!(
            &mut details,
            "  status:         OK (desktop hotkey backend; evdev fallback incomplete)"
        )
        .unwrap();
    }

    let summary = if blocking {
        let mut summary = String::new();
        writeln!(&mut summary, "hotkey preflight failed before model startup").unwrap();
        writeln!(
            &mut summary,
            "session: XDG_SESSION_TYPE={session}, DISPLAY={display}, XAUTHORITY={xauthority}"
        )
        .unwrap();
        writeln!(
            &mut summary,
            "desktop backend: {}",
            x11_probe
                .as_ref()
                .map(|_| "OK".to_string())
                .unwrap_or_else(|err| format!("unavailable ({err})"))
        )
        .unwrap();
        writeln!(
            &mut summary,
            "evdev backend: {} device(s), {} readable, {} permission denied",
            evdev.event_devices, evdev.readable, evdev.denied
        )
        .unwrap();
        write_linux_fix(&mut summary, &user);
        summary
    } else if x11_owned_by_parakit {
        format!(
            "hotkey preflight skipped because running parakit pid(s) already own Ctrl+Space: {}",
            format_pids(&running_parakit)
        )
    } else if evdev_ready {
        "hotkey preflight passed with evdev backend preferred".to_string()
    } else if x11_probe.is_ok() {
        "hotkey preflight passed with X11 desktop backend".to_string()
    } else {
        "hotkey preflight passed with X11 desktop backend; evdev fallback is incomplete".to_string()
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
impl EvdevReport {
    fn grab_likely_available(&self) -> bool {
        self.event_devices > 0 && self.denied == 0 && self.other_errors.is_empty()
    }

    fn status_label(&self) -> &'static str {
        if self.grab_likely_available() {
            "ready"
        } else if self.readable > 0 {
            "partial permissions"
        } else {
            "unavailable"
        }
    }
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
fn running_parakit_processes() -> Vec<u32> {
    use std::fs;
    use std::path::Path;

    let self_pid = std::process::id();
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };

    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == self_pid {
            continue;
        }

        let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let first_arg = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
        let first_arg = String::from_utf8_lossy(first_arg);
        let binary_name = Path::new(first_arg.as_ref())
            .file_name()
            .and_then(|name| name.to_str());
        if binary_name == Some("parakit") {
            pids.push(pid);
        }
    }

    pids.sort_unstable();
    pids
}

#[cfg(target_os = "linux")]
fn format_pids(pids: &[u32]) -> String {
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
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
/// Return whether the rdev evdev fallback is likely able to grab every input
/// device.
///
/// # Returns
///
/// `true` when `/dev/input/event*` devices exist and none fail with permission
/// or other open errors.
pub(crate) fn linux_evdev_fallback_available() -> bool {
    evdev_report().grab_likely_available()
}

#[cfg(target_os = "linux")]
fn probe_x11_desktop_hotkey() -> std::result::Result<(), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, GrabMode, Keycode, ModMask, Window};
    use x11rb::rust_connection::RustConnection;

    const SPACE_KEYSYM: u32 = 0x0020;

    fn space_keycode(conn: &RustConnection) -> std::result::Result<Keycode, String> {
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = max_keycode - min_keycode + 1;
        let mapping = conn
            .get_keyboard_mapping(min_keycode, count)
            .map_err(|err| err.to_string())?
            .reply()
            .map_err(|err| err.to_string())?;
        let keysyms_per_keycode = mapping.keysyms_per_keycode as usize;

        for (offset, keysyms) in mapping.keysyms.chunks(keysyms_per_keycode).enumerate() {
            if keysyms.contains(&SPACE_KEYSYM) {
                return Ok(min_keycode + offset as u8);
            }
        }

        Err("could not map the X11 Space keysym to a keycode".to_string())
    }

    fn grab_mods() -> [ModMask; 4] {
        [
            ModMask::CONTROL,
            ModMask::CONTROL | ModMask::M2,
            ModMask::CONTROL | ModMask::LOCK,
            ModMask::CONTROL | ModMask::M2 | ModMask::LOCK,
        ]
    }

    fn ungrab(conn: &RustConnection, root: Window, keycode: Keycode) {
        for mods in grab_mods() {
            if let Ok(result) = conn.ungrab_key(keycode, root, mods) {
                result.ignore_error();
            }
        }
        let _ = conn.flush();
    }

    let (conn, screen_num) = RustConnection::connect(None).map_err(|err| err.to_string())?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| "X11 display did not expose the requested screen".to_string())?
        .root;
    let keycode = space_keycode(&conn)?;

    for mods in grab_mods() {
        match conn
            .grab_key(false, root, mods, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(|err| err.to_string())?
            .check()
        {
            Ok(()) => {}
            Err(err) => {
                ungrab(&conn, root, keycode);
                return Err(format!("XGrabKey rejected Ctrl+Space: {err}"));
            }
        }
    }
    conn.flush().map_err(|err| err.to_string())?;
    ungrab(&conn, root, keycode);
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_linux_fix(out: &mut String, user: &str) {
    writeln!(
        out,
        "fix:\n  - Preferred: use an Xorg/X11 session and make sure Ctrl+Space is not already bound by the desktop or input method.\n  - If this happened after GNOME logout/login, restart tmux, terminals, and user services from the new desktop session so DISPLAY/XAUTHORITY are fresh.\n  - Session-stable path: grant the desktop user read access to /dev/input/event*:\n      sudo usermod -aG input {user}\n  - After changing groups, log out completely and log back in, or reboot.\n  - Verify the fresh session:\n      id -nG | tr ' ' '\\n' | grep '^input$'\n  - Then run: parakit --hotkey-backend evdev\n  - Do not run parakit with sudo as the normal workaround."
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
