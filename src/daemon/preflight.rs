//! Runtime preflight checks for desktop input and hotkey permissions.

use anyhow::{bail, Result};
use parakit::build_info;
use std::fmt::Write as _;

use super::hotkey::HotkeyBackend;
use super::inject::{self, PasteMode};

/// Run blocking daemon preflight checks before expensive startup work.
///
/// # Arguments
///
/// * `backend` - Selected hotkey backend to validate.
///
/// # Returns
///
/// Returns `Ok(())` when no blocking hotkey problem was detected.
///
/// # Errors
///
/// Returns an actionable error when the global hotkey backend is known to be
/// unavailable in the current desktop session.
pub fn ensure_hotkey_ready(backend: HotkeyBackend) -> Result<()> {
    let report = hotkey_report(backend);
    if report.blocking {
        bail!("{}", report.summary);
    }
    Ok(())
}

/// Run diagnostics and return whether daemon startup should proceed.
///
/// # Arguments
///
/// * `quiet` - Suppress stdout when true.
/// * `verbose` - Print diagnostic details when true.
/// * `paste_mode` - Insertion mode to validate.
/// * `deep` - Run the platform insertion smoke test when true.
/// * `backend` - Selected hotkey backend to validate.
///
/// # Returns
///
/// `true` when no blocking problem was detected.
pub fn print_doctor(
    quiet: bool,
    verbose: bool,
    paste_mode: PasteMode,
    deep: bool,
    backend: HotkeyBackend,
) -> bool {
    let report = hotkey_report(backend);
    let daemon_lock = singleton_lock_probe();
    let mic = super::audio::probe_default_input();
    let insertion = if deep {
        inject::smoke_test(paste_mode)
    } else {
        inject::preflight(paste_mode)
    };
    let ok = !report.blocking && daemon_lock.is_ok() && mic.is_ok() && insertion.is_ok();

    if quiet {
        return ok;
    }

    if verbose {
        print_doctor_details(&report, &daemon_lock, &mic, &insertion, paste_mode, deep);
    } else {
        print_doctor_summary(
            &report,
            &daemon_lock,
            &mic,
            &insertion,
            paste_mode,
            deep,
            ok,
        );
    }

    ok
}

struct HotkeyReport {
    blocking: bool,
    status: String,
    summary: String,
    details: String,
}

fn print_doctor_summary(
    report: &HotkeyReport,
    daemon_lock: &Result<()>,
    mic: &Result<super::audio::MicInfo>,
    insertion: &Result<()>,
    paste_mode: PasteMode,
    deep: bool,
    ok: bool,
) {
    println!("parakit doctor: {}", if ok { "OK" } else { "FAIL" });
    print_status_line("hotkey", !report.blocking, &report.status);
    match daemon_lock {
        Ok(()) => print_status_line("daemon", true, "no existing daemon lock"),
        Err(err) => print_status_line("daemon", false, &format!("{err:#}")),
    }
    match mic {
        Ok(mic) => print_status_line("mic", true, &mic.summary()),
        Err(err) => print_status_line("mic", false, &format!("{err:#}")),
    }
    let insertion_label = if deep {
        format!("{} smoke test", paste_mode.label())
    } else {
        format!("{} preflight", paste_mode.label())
    };
    match insertion {
        Ok(()) => print_status_line("insertion", true, &insertion_label),
        Err(err) => print_status_line("insertion", false, &format!("{err:#}")),
    }

    if ok {
        println!("  run:       parakit");
        println!("  shell:     parakit doctor && parakit");
    } else {
        println!("  details:   parakit --verbose doctor");
    }
}

fn print_status_line(label: &str, ok: bool, detail: &str) {
    println!(
        "  {label:<10} {} ({detail})",
        if ok { "OK " } else { "FAIL" }
    );
}

fn print_doctor_details(
    report: &HotkeyReport,
    daemon_lock: &Result<()>,
    mic: &Result<super::audio::MicInfo>,
    insertion: &Result<()>,
    paste_mode: PasteMode,
    deep: bool,
) {
    println!("{}", report.details.trim_end());
    match daemon_lock {
        Ok(()) => println!("  daemon lock:   OK"),
        Err(err) => println!("  daemon lock:   FAIL ({err:#})"),
    }
    match mic {
        Ok(mic) => {
            println!("  mic:            {}", mic.summary());
            println!("  audio status:   OK");
        }
        Err(err) => {
            println!("  mic:            unavailable ({err:#})");
            println!("  audio status:   FAIL");
        }
    }
    match insertion {
        Ok(()) if deep => println!("  insertion:     OK ({} smoke test)", paste_mode.label()),
        Ok(()) => println!("  insertion:     OK ({} preflight)", paste_mode.label()),
        Err(err) => println!("  insertion:     FAIL ({err:#})"),
    }
    println!("  build:");
    for line in build_info::diagnostic_lines() {
        println!("    {line}");
    }
}

#[cfg(target_os = "linux")]
/// Acquire the per-user daemon lock.
///
/// # Returns
///
/// An open lock file. Keep it alive for the daemon lifetime.
///
/// # Errors
///
/// Returns an error if the runtime directory cannot be created, the lock file
/// cannot be opened, or another process already holds the lock.
pub(crate) fn acquire_singleton_lock() -> Result<std::fs::File> {
    acquire_singleton_lock_at(&singleton_lock_path())
}

#[cfg(target_os = "linux")]
fn singleton_lock_probe() -> Result<()> {
    let lock = acquire_singleton_lock()?;
    drop(lock);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn singleton_lock_probe() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn singleton_lock_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime_dir.join("parakit").join("parakit.lock")
}

#[cfg(target_os = "linux")]
fn acquire_singleton_lock_at(path: &std::path::Path) -> Result<std::fs::File> {
    use anyhow::Context as _;
    use std::fs::{create_dir_all, OpenOptions};
    use std::os::fd::AsRawFd;

    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .with_context(|| format!("create daemon lock dir {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open daemon lock {}", path.display()))?;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!(
            "another parakit daemon is already running or lock is held: {}",
            path.display()
        );
    }

    Ok(file)
}

#[cfg(target_os = "linux")]
fn linux_hotkey_startup_blocked(backend: HotkeyBackend, evdev_ready: bool) -> bool {
    match backend {
        HotkeyBackend::Auto | HotkeyBackend::Evdev => !evdev_ready,
        HotkeyBackend::Desktop => true,
    }
}

#[cfg(target_os = "linux")]
fn linux_hotkey_success_label(backend: HotkeyBackend) -> &'static str {
    match backend {
        HotkeyBackend::Auto => "evdev keyboard grab",
        HotkeyBackend::Evdev => "evdev keyboard grab",
        HotkeyBackend::Desktop => unreachable!("desktop backend is disabled on Linux"),
    }
}

#[cfg(target_os = "linux")]
fn hotkey_report(backend: HotkeyBackend) -> HotkeyReport {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let xauthority = std::env::var("XAUTHORITY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    let evdev = evdev_report();
    let evdev_ready = evdev.grab_likely_available();
    let blocking = linux_hotkey_startup_blocked(backend, evdev_ready);
    let status = linux_hotkey_status(backend, &evdev, blocking);

    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(
        &mut details,
        "  session:        XDG_SESSION_TYPE={session}, DISPLAY={display}"
    )
    .unwrap();
    writeln!(&mut details, "  xauthority:     {xauthority}").unwrap();
    writeln!(&mut details, "  selected:       {}", backend.label()).unwrap();
    writeln!(
        &mut details,
        "  desktop:        disabled for Linux-stable hotkey capture"
    )
    .unwrap();
    writeln!(
        &mut details,
        "  evdev:          keyboard grab ({})",
        evdev.status_label()
    )
    .unwrap();
    writeln!(
        &mut details,
        "  input devices:  {} event device(s), {} readable, {} permission denied",
        evdev.event_devices, evdev.readable, evdev.denied
    )
    .unwrap();
    writeln!(
        &mut details,
        "  hotkey devices: {} Ctrl+Space keyboard candidate(s)",
        evdev.hotkey_keyboards
    )
    .unwrap();
    match &evdev.uinput_error {
        Some(err) => writeln!(&mut details, "  uinput:        unavailable ({err})").unwrap(),
        None => writeln!(&mut details, "  uinput:        writable").unwrap(),
    };
    if !evdev.other_errors.is_empty() {
        writeln!(&mut details, "  input errors:").unwrap();
        for err in &evdev.other_errors {
            writeln!(&mut details, "    {err}").unwrap();
        }
    }

    if blocking {
        writeln!(&mut details, "  status:         FAIL").unwrap();
        if backend == HotkeyBackend::Desktop {
            writeln!(
                &mut details,
                "  reason:         desktop backend is disabled in the Linux-stable path"
            )
            .unwrap();
        }
        write_linux_fix(&mut details, &user);
    } else if evdev_ready {
        writeln!(
            &mut details,
            "  status:         OK ({})",
            linux_hotkey_success_label(backend)
        )
        .unwrap();
    }

    let summary = if blocking {
        let mut summary = String::new();
        writeln!(&mut summary, "hotkey preflight failed before model startup").unwrap();
        writeln!(&mut summary, "selected backend: {}", backend.label()).unwrap();
        writeln!(
            &mut summary,
            "session: XDG_SESSION_TYPE={session}, DISPLAY={display}, XAUTHORITY={xauthority}"
        )
        .unwrap();
        writeln!(
            &mut summary,
            "desktop backend: disabled in the Linux-stable path"
        )
        .unwrap();
        writeln!(
            &mut summary,
            "evdev backend: {} device(s), {} readable, {} Ctrl+Space keyboard candidate(s), {} permission denied",
            evdev.event_devices, evdev.readable, evdev.hotkey_keyboards, evdev.denied
        )
        .unwrap();
        if let Some(err) = &evdev.uinput_error {
            writeln!(&mut summary, "uinput: unavailable ({err})").unwrap();
        }
        write_linux_fix(&mut summary, &user);
        summary
    } else if evdev_ready {
        format!(
            "hotkey preflight passed with {}",
            linux_hotkey_success_label(backend)
        )
    } else {
        unreachable!("non-blocking Linux hotkey report requires evdev readiness")
    };

    HotkeyReport {
        blocking,
        status,
        summary,
        details,
    }
}

#[cfg(target_os = "linux")]
struct EvdevReport {
    event_devices: usize,
    readable: usize,
    hotkey_keyboards: usize,
    denied: usize,
    uinput_writable: bool,
    uinput_error: Option<String>,
    other_errors: Vec<String>,
}

#[cfg(target_os = "linux")]
impl EvdevReport {
    fn grab_likely_available(&self) -> bool {
        self.event_devices > 0
            && self.hotkey_keyboards > 0
            && self.denied == 0
            && self.uinput_writable
            && self.other_errors.is_empty()
    }

    fn status_label(&self) -> &'static str {
        if self.grab_likely_available() {
            "ready"
        } else if !self.uinput_writable {
            "uinput unavailable"
        } else if self.hotkey_keyboards == 0 {
            "no keyboard candidates"
        } else if self.readable > 0 {
            "partial permissions"
        } else {
            "unavailable"
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_hotkey_status(backend: HotkeyBackend, evdev: &EvdevReport, blocking: bool) -> String {
    if backend == HotkeyBackend::Desktop {
        return "desktop backend disabled on Linux".to_string();
    }

    if !blocking {
        return format!("{} ready", linux_hotkey_success_label(backend));
    }

    if !evdev.uinput_writable {
        return "uinput unavailable".to_string();
    }
    if evdev.hotkey_keyboards == 0 {
        return format!(
            "no Ctrl+Space keyboard device found ({} input device(s) readable)",
            evdev.readable
        );
    }
    if evdev.denied > 0 {
        return format!(
            "input permissions incomplete ({} permission denied)",
            evdev.denied
        );
    }
    if !evdev.other_errors.is_empty() {
        return "input device scan errors".to_string();
    }

    "evdev keyboard grab unavailable".to_string()
}

#[cfg(target_os = "linux")]
fn evdev_report() -> EvdevReport {
    use evdev_rs::enums::{EventCode, EV_KEY};
    use std::fs::{self, File, OpenOptions};
    use std::io::ErrorKind;

    let mut event_devices = 0_usize;
    let mut readable = 0_usize;
    let mut hotkey_keyboards = 0_usize;
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
                    Ok(file) => {
                        readable += 1;
                        if let Ok(device) = evdev_rs::Device::new_from_fd(file) {
                            let has_space =
                                device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_SPACE));
                            let has_ctrl = device
                                .has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_LEFTCTRL))
                                || device.has_event_code(&EventCode::EV_KEY(EV_KEY::KEY_RIGHTCTRL));
                            if has_space && has_ctrl {
                                hotkey_keyboards += 1;
                            }
                        }
                    }
                    Err(err) if err.kind() == ErrorKind::PermissionDenied => denied += 1,
                    Err(err) => other_errors.push(format!("{}: {err}", path.display())),
                }
            }
        }
        Err(err) => {
            other_errors.push(format!("/dev/input: {err}"));
        }
    }

    let (uinput_writable, uinput_error) = match OpenOptions::new().write(true).open("/dev/uinput") {
        Ok(_) => (true, None),
        Err(err) => (false, Some(err.to_string())),
    };

    EvdevReport {
        event_devices,
        readable,
        hotkey_keyboards,
        denied,
        uinput_writable,
        uinput_error,
        other_errors,
    }
}

#[cfg(target_os = "linux")]
fn write_linux_fix(out: &mut String, user: &str) {
    writeln!(
        out,
        "fix:\n  - Grant the desktop user read access to /dev/input/event*:\n      sudo usermod -aG input {user}\n  - Ensure /dev/uinput is writable by the desktop user. On many distros this needs a uinput udev rule.\n  - After changing groups or udev rules, log out completely and log back in, or reboot.\n  - Verify the fresh session:\n      id -nG | tr ' ' '\\n' | grep '^input$'\n      ls -l /dev/uinput /dev/input/event* | head\n  - Then run: parakit --hotkey-backend evdev\n  - Do not run parakit with sudo; audio, clipboard, and insertion belong to the desktop user.\n  - The Linux desktop hotkey backend is disabled until it is replaced by global-hotkey or the XDG portal."
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
fn hotkey_report(_backend: HotkeyBackend) -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         manual check\n  fix: grant Accessibility and Input Monitoring permissions to both the terminal and the parakit binary.".to_string();
    HotkeyReport {
        blocking: false,
        status: "manual permission check required".to_string(),
        summary: details.clone(),
        details,
    }
}

#[cfg(target_os = "windows")]
fn hotkey_report(_backend: HotkeyBackend) -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         OK unless security software blocks the binary.".to_string();
    HotkeyReport {
        blocking: false,
        status: "global hook available unless blocked by security software".to_string(),
        summary: details.clone(),
        details,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn hotkey_report(_backend: HotkeyBackend) -> HotkeyReport {
    let details = "parakit doctor\n  hotkey backend: rdev::grab\n  status:         unsupported platform preflight".to_string();
    HotkeyReport {
        blocking: false,
        status: "unsupported platform preflight".to_string(),
        summary: details.clone(),
        details,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn auto_and_evdev_require_evdev_readiness() {
        for backend in [HotkeyBackend::Auto, HotkeyBackend::Evdev] {
            assert!(!linux_hotkey_startup_blocked(backend, true));
            assert!(linux_hotkey_startup_blocked(backend, false));
        }
    }

    #[test]
    fn forced_desktop_is_disabled() {
        assert!(linux_hotkey_startup_blocked(HotkeyBackend::Desktop, true));
        assert!(linux_hotkey_startup_blocked(HotkeyBackend::Desktop, false));
    }

    #[test]
    fn singleton_lock_blocks_second_holder() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before UNIX epoch")
            .as_nanos();
        let path = std::path::PathBuf::from(format!(
            "target/tmp/parakit-lock-test-{}-{unique}/parakit.lock",
            std::process::id()
        ));

        let first = acquire_singleton_lock_at(&path).expect("first lock should succeed");
        let second = acquire_singleton_lock_at(&path);
        assert!(second.is_err());
        drop(first);
        let third = acquire_singleton_lock_at(&path).expect("lock should release after drop");
        drop(third);
    }
}
