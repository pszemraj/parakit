//! Runtime preflight checks for desktop input and hotkey permissions.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use parakit::build_info;
use std::fmt::Write as _;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::hotkey::HotkeyBackend;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::hotkey_help;
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
    let report = hotkey_report(backend, true);
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
    let report = hotkey_report(backend, !quiet);
    let daemon_lock = singleton_lock_probe();
    let mic = super::audio::probe_default_input();
    let insertion = if deep {
        inject::smoke_test(paste_mode)
    } else {
        inject::preflight(paste_mode)
    };
    let ok = doctor_ready(&report, &daemon_lock, &mic, &insertion);

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
        format!("{} guarded smoke test", paste_mode.label())
    } else {
        format!("{} preflight", paste_mode.label())
    };
    match insertion {
        Ok(()) => print_status_line("insertion", true, &insertion_label),
        Err(err) => print_status_line("insertion", false, &format!("{err:#}")),
    }

    if !ok {
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
            for line in mic.detail_lines() {
                println!("  audio detail:   {line}");
            }
            println!("  audio status:   OK");
        }
        Err(err) => {
            println!("  mic:            unavailable ({err:#})");
            println!("  audio status:   FAIL");
        }
    }
    match insertion {
        Ok(()) if deep => println!(
            "  insertion:     OK ({} guarded smoke test)",
            paste_mode.label()
        ),
        Ok(()) => println!("  insertion:     OK ({} preflight)", paste_mode.label()),
        Err(err) => println!("  insertion:     FAIL ({err:#})"),
    }
    println!("  build:");
    for line in build_info::diagnostic_lines() {
        println!("    {line}");
    }
    print_compute_details();
}

#[cfg(feature = "bundled")]
fn print_compute_details() {
    println!("  compute:");
    #[cfg(target_os = "macos")]
    for line in super::macos::architecture_warning_lines() {
        println!("    {line}");
    }
    let devices = super::stderr::with_stderr_suppressed(parakit::gpu::devices);
    if devices.is_empty() {
        println!("    no ggml devices reported");
    } else {
        for device in &devices {
            println!("    {}", device.diagnostic_line());
        }
        if let Some(device) = parakit::gpu::preferred_gpu_device_in(&devices) {
            println!("    auto selects: {}", device.diagnostic_line());
        }
    }
    if build_info::accelerator_enabled()
        && !devices.iter().any(parakit::gpu::DeviceInfo::is_gpu_like)
    {
        println!("    warning: accelerator build has no GPU or iGPU visible to ggml");
    }
}

#[cfg(not(feature = "bundled"))]
fn print_compute_details() {}

fn doctor_ready(
    report: &HotkeyReport,
    daemon_lock: &Result<()>,
    mic: &Result<super::audio::MicInfo>,
    insertion: &Result<()>,
) -> bool {
    !report.blocking && daemon_lock.is_ok() && mic.is_ok() && insertion.is_ok()
}

/// Acquire the per-user daemon lock.
///
/// # Returns
///
/// An owned lock handle. Keep it alive for the daemon lifetime.
///
/// # Errors
///
/// Returns an error if the runtime directory cannot be created, the lock file
/// cannot be opened, or another process already holds the lock.
pub(crate) fn acquire_singleton_lock() -> Result<DaemonLock> {
    let path = singleton_lock_path()?;
    acquire_singleton_lock_at(&path)
}

fn singleton_lock_probe() -> Result<()> {
    let lock = acquire_singleton_lock()?;
    drop(lock);
    Ok(())
}

/// Per-user daemon singleton lock.
pub(crate) struct DaemonLock {
    file: File,
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn singleton_lock_path() -> Result<PathBuf> {
    Ok(daemon_runtime_dir()?.join("parakit.lock"))
}

/// Return the per-user daemon control socket path.
///
/// # Returns
///
/// A Unix-domain socket path under the daemon runtime directory.
///
/// # Errors
///
/// Returns an error when the runtime directory cannot be determined.
#[cfg(unix)]
pub(crate) fn control_socket_path() -> Result<PathBuf> {
    Ok(daemon_runtime_dir()?.join("control.sock"))
}

/// Return the per-user daemon runtime directory.
///
/// # Returns
///
/// A platform-appropriate directory for daemon lock files.
///
/// # Errors
///
/// Returns an error when the user's base directories cannot be determined.
pub(crate) fn daemon_runtime_dir() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !runtime_dir.as_os_str().is_empty() {
            return Ok(PathBuf::from(runtime_dir).join("parakit"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let dirs =
            directories::BaseDirs::new().context("could not determine user runtime directory")?;
        Ok(dirs.data_local_dir().join("parakit").join("run"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(parakit::model::xdg_cache_base()?
            .join("parakit")
            .join("run"))
    }
}

fn acquire_singleton_lock_at(path: &Path) -> Result<DaemonLock> {
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

    match file.try_lock_exclusive() {
        Ok(()) => Ok(DaemonLock { file }),
        Err(err) if err.kind() == ErrorKind::WouldBlock => bail!(
            "another parakit daemon is already running or lock is held: {}",
            path.display()
        ),
        Err(err) => Err(err).with_context(|| format!("lock daemon lock {}", path.display())),
    }
}

#[cfg(target_os = "linux")]
fn linux_hotkey_startup_blocked(
    backend: HotkeyBackend,
    x11_ready: bool,
    evdev_ready: bool,
) -> bool {
    if backend.uses_registered_x11() || backend.uses_passive_x11_listen() {
        !x11_ready
    } else if backend.uses_evdev_proxy() {
        !evdev_ready
    } else {
        false
    }
}

#[cfg(target_os = "linux")]
fn linux_hotkey_success_label(backend: HotkeyBackend) -> &'static str {
    match backend {
        HotkeyBackend::Auto | HotkeyBackend::Desktop | HotkeyBackend::X11GlobalHotkey => {
            "registered X11 Ctrl+Space"
        }
        HotkeyBackend::X11Listen => "passive X11 Ctrl+Space listen",
        HotkeyBackend::EvdevProxyExperimental => "experimental evdev/uinput keyboard proxy",
    }
}

#[cfg(target_os = "linux")]
fn hotkey_report(backend: HotkeyBackend, _prompt_accessibility: bool) -> HotkeyReport {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string());
    let xauthority = std::env::var("XAUTHORITY").unwrap_or_else(|_| "<unset>".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    let registered = if backend.uses_registered_x11() {
        Some(super::hotkey::registered_hotkey_probe())
    } else {
        None
    };
    let registered_ready = registered.as_ref().is_some_and(|result| result.is_ok());
    let x11_listen = backend
        .uses_passive_x11_listen()
        .then(super::session::ensure_x11_session_supported);
    let x11_listen_ready = x11_listen.as_ref().is_some_and(|result| result.is_ok());
    let x11_ready = if backend.uses_passive_x11_listen() {
        x11_listen_ready
    } else {
        registered_ready
    };
    let evdev = backend.uses_evdev_proxy().then(evdev_report);
    let evdev_ready = evdev
        .as_ref()
        .is_some_and(EvdevReport::grab_likely_available);
    let blocking = linux_hotkey_startup_blocked(backend, x11_ready, evdev_ready);
    let status = linux_hotkey_status(
        backend,
        registered.as_ref(),
        x11_listen.as_ref(),
        evdev.as_ref(),
        blocking,
    );

    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(
        &mut details,
        "  session:        XDG_SESSION_TYPE={session}, DISPLAY={display}"
    )
    .unwrap();
    writeln!(&mut details, "  xauthority:     {xauthority}").unwrap();
    writeln!(&mut details, "  selected:       {}", backend.label()).unwrap();
    match registered.as_ref() {
        Some(Ok(())) => writeln!(&mut details, "  registered:     Ctrl+Space available").unwrap(),
        Some(Err(err)) => {
            writeln!(&mut details, "  registered:     unavailable ({err:#})").unwrap();
        }
        None => writeln!(&mut details, "  registered:     not selected").unwrap(),
    }
    match x11_listen.as_ref() {
        Some(Ok(())) => writeln!(&mut details, "  x11-listen:     session available").unwrap(),
        Some(Err(err)) => {
            writeln!(&mut details, "  x11-listen:     unavailable ({err:#})").unwrap();
        }
        None => writeln!(&mut details, "  x11-listen:     not selected").unwrap(),
    }

    if let Some(evdev) = &evdev {
        writeln!(
            &mut details,
            "  evdev-proxy:    keyboard grab ({})",
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
            Some(err) => writeln!(&mut details, "  uinput:         unavailable ({err})").unwrap(),
            None => writeln!(&mut details, "  uinput:         writable").unwrap(),
        };
        if !evdev.other_errors.is_empty() {
            writeln!(&mut details, "  input errors:").unwrap();
            for err in &evdev.other_errors {
                writeln!(&mut details, "    {err}").unwrap();
            }
        }
    } else {
        writeln!(
            &mut details,
            "  evdev-proxy:    not selected (/dev/input and /dev/uinput not checked)"
        )
        .unwrap();
    }

    if blocking {
        writeln!(&mut details, "  status:         FAIL").unwrap();
        if backend.uses_registered_x11() {
            hotkey_help::write_registered_linux_fix(&mut details);
        } else if backend.uses_passive_x11_listen() {
            hotkey_help::write_x11_listen_linux_fix(&mut details);
        } else {
            hotkey_help::write_evdev_linux_fix(&mut details, &user);
        }
    } else {
        writeln!(
            &mut details,
            "  status:         OK ({})",
            linux_hotkey_success_label(backend)
        )
        .unwrap();
    }
    append_wsl_warning(&mut details);

    let summary = if blocking {
        let mut summary = String::new();
        writeln!(&mut summary, "hotkey preflight failed before model startup").unwrap();
        writeln!(&mut summary, "selected backend: {}", backend.label()).unwrap();
        writeln!(
            &mut summary,
            "session: XDG_SESSION_TYPE={session}, DISPLAY={display}, XAUTHORITY={xauthority}"
        )
        .unwrap();
        if backend.uses_registered_x11() {
            if let Some(Err(err)) = registered.as_ref() {
                writeln!(&mut summary, "registered hotkey: unavailable ({err:#})").unwrap();
            }
            hotkey_help::write_registered_linux_fix(&mut summary);
        } else if backend.uses_passive_x11_listen() {
            if let Some(Err(err)) = x11_listen.as_ref() {
                writeln!(&mut summary, "x11-listen: unavailable ({err:#})").unwrap();
            }
            hotkey_help::write_x11_listen_linux_fix(&mut summary);
        } else if let Some(evdev) = &evdev {
            writeln!(
                &mut summary,
                "evdev-proxy backend: {} device(s), {} readable, {} Ctrl+Space keyboard candidate(s), {} permission denied",
                evdev.event_devices, evdev.readable, evdev.hotkey_keyboards, evdev.denied
            )
            .unwrap();
            if let Some(err) = &evdev.uinput_error {
                writeln!(&mut summary, "uinput: unavailable ({err})").unwrap();
            }
            hotkey_help::write_evdev_linux_fix(&mut summary, &user);
        }
        summary
    } else {
        format!(
            "hotkey preflight passed with {}",
            linux_hotkey_success_label(backend)
        )
    };

    HotkeyReport {
        blocking,
        status,
        summary,
        details,
    }
}

#[cfg(target_os = "linux")]
fn append_wsl_warning(details: &mut String) {
    if !super::wsl::running_under_wsl() {
        return;
    }
    writeln!(details, "  wsl:            detected").unwrap();
    writeln!(details, "  note:           {}", super::wsl::warning()).unwrap();
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
fn linux_hotkey_status(
    backend: HotkeyBackend,
    registered: Option<&Result<()>>,
    x11_listen: Option<&Result<()>>,
    evdev: Option<&EvdevReport>,
    blocking: bool,
) -> String {
    if !blocking {
        return format!("{} ready", linux_hotkey_success_label(backend));
    }

    if backend.uses_registered_x11() {
        return match registered {
            Some(Err(err)) => format!("registered Ctrl+Space unavailable ({err:#})"),
            _ => "registered Ctrl+Space unavailable".to_string(),
        };
    }

    if backend.uses_passive_x11_listen() {
        return match x11_listen {
            Some(Err(err)) => format!("passive X11 listen unavailable ({err:#})"),
            _ => "passive X11 listen unavailable".to_string(),
        };
    }

    let Some(evdev) = evdev else {
        return "evdev-proxy unavailable".to_string();
    };
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

    unreachable!("blocking evdev report without a failure reason")
}

#[cfg(target_os = "linux")]
fn evdev_report() -> EvdevReport {
    use std::fs::{File, OpenOptions};
    use std::io::ErrorKind;

    let mut event_devices = 0_usize;
    let mut readable = 0_usize;
    let mut hotkey_keyboards = 0_usize;
    let mut denied = 0_usize;
    let mut other_errors = Vec::new();

    match super::hotkey::linux_event_device_paths() {
        Ok(paths) => {
            for path in paths {
                event_devices += 1;
                match File::open(&path) {
                    Ok(file) => {
                        readable += 1;
                        if evdev_rs::Device::new_from_fd(file)
                            .is_ok_and(|device| super::hotkey::linux_device_has_ctrl_space(&device))
                        {
                            hotkey_keyboards += 1;
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

#[cfg(target_os = "macos")]
fn hotkey_report(_backend: HotkeyBackend, prompt_accessibility: bool) -> HotkeyReport {
    let permissions = super::macos::permission_report(prompt_accessibility);
    let blocking = !permissions.accessibility.granted();
    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(&mut details, "  hotkey backend: CoreGraphics event tap").unwrap();
    writeln!(&mut details, "  ptt hotkey:     Left Control+Space").unwrap();
    writeln!(
        &mut details,
        "  accessibility: {}",
        permissions.accessibility.label()
    )
    .unwrap();
    writeln!(
        &mut details,
        "  input monitor: {} (diagnostic only)",
        permissions.input_monitoring.label()
    )
    .unwrap();
    writeln!(
        &mut details,
        "  microphone:    {}",
        permissions.microphone.label()
    )
    .unwrap();
    if blocking {
        writeln!(&mut details, "  status:        FAIL").unwrap();
        hotkey_help::write_macos_accessibility_fix(&mut details);
    } else {
        writeln!(&mut details, "  status:        OK").unwrap();
    }
    let status = if blocking {
        "macOS Accessibility permission missing".to_string()
    } else {
        "macOS Accessibility ready for Left Control+Space".to_string()
    };
    let summary = if blocking {
        details.clone()
    } else {
        "macOS Accessibility permission granted; hotkey Left Control+Space".to_string()
    };
    HotkeyReport {
        blocking,
        status,
        summary,
        details,
    }
}

#[cfg(target_os = "windows")]
fn hotkey_report(_backend: HotkeyBackend, _prompt_accessibility: bool) -> HotkeyReport {
    let registered = super::windows_input::registered_hotkey_probe();
    let security = super::windows_security::current_process_security_report();
    let blocking = registered.is_err();
    let mut details = String::new();
    writeln!(&mut details, "parakit doctor").unwrap();
    writeln!(&mut details, "  hotkey backend: RegisterHotKey Ctrl+Space").unwrap();
    match &registered {
        Ok(()) => writeln!(&mut details, "  registered:     Ctrl+Space available").unwrap(),
        Err(err) => writeln!(&mut details, "  registered:     unavailable ({err:#})").unwrap(),
    }
    writeln!(
        &mut details,
        "  elevated:       {}",
        security
            .elevated
            .map(|value| if value { "yes" } else { "no" })
            .unwrap_or("unknown")
    )
    .unwrap();
    writeln!(
        &mut details,
        "  integrity:      {}",
        security.integrity.as_deref().unwrap_or("unknown")
    )
    .unwrap();
    writeln!(
        &mut details,
        "  input note:     SendInput cannot inject into higher-integrity/elevated target applications"
    )
    .unwrap();
    if blocking {
        writeln!(&mut details, "  status:         FAIL").unwrap();
        writeln!(
            &mut details,
            "fix:\n  - Close any application that already owns Ctrl+Space.\n  - Re-run: parakit doctor\n  - Elevated target apps may still reject paste input from a normal user process."
        )
        .unwrap();
    } else {
        writeln!(
            &mut details,
            "  status:         OK (registered Windows Ctrl+Space)"
        )
        .unwrap();
    }

    let status = match &registered {
        Ok(()) => "registered Windows Ctrl+Space ready".to_string(),
        Err(err) => format!("registered Ctrl+Space unavailable ({err:#})"),
    };
    let summary = if blocking {
        details.clone()
    } else {
        "hotkey preflight passed with registered Windows Ctrl+Space".to_string()
    };
    HotkeyReport {
        blocking,
        status,
        summary,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::super::audio::MicInfo;
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn selected_hotkey_backend_controls_readiness() {
        let cases = [
            (HotkeyBackend::Auto, true, false, false),
            (HotkeyBackend::Auto, false, true, true),
            (HotkeyBackend::Desktop, true, false, false),
            (HotkeyBackend::Desktop, false, true, true),
            (HotkeyBackend::X11GlobalHotkey, true, false, false),
            (HotkeyBackend::X11GlobalHotkey, false, true, true),
            (HotkeyBackend::X11Listen, true, false, false),
            (HotkeyBackend::X11Listen, false, true, true),
            (HotkeyBackend::EvdevProxyExperimental, false, true, false),
            (HotkeyBackend::EvdevProxyExperimental, true, false, true),
        ];
        for (backend, x11_ready, evdev_ready, expected) in cases {
            assert_eq!(
                linux_hotkey_startup_blocked(backend, x11_ready, evdev_ready),
                expected
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn evdev_report_with_hotkey_keyboards(hotkey_keyboards: usize) -> EvdevReport {
        EvdevReport {
            event_devices: 4,
            readable: 1,
            hotkey_keyboards,
            denied: 3,
            uinput_writable: true,
            uinput_error: None,
            other_errors: Vec::new(),
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn evdev_readiness_allows_denied_non_candidates() {
        let report = evdev_report_with_hotkey_keyboards(1);

        assert!(report.grab_likely_available());
        assert_eq!(report.status_label(), "ready");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn evdev_readiness_still_requires_hotkey_candidate() {
        let report = evdev_report_with_hotkey_keyboards(0);

        assert!(!report.grab_likely_available());
        assert_eq!(report.status_label(), "no keyboard candidates");
    }

    #[test]
    fn doctor_ready_requires_free_daemon_lock() {
        let report = HotkeyReport {
            blocking: false,
            status: "ready".to_string(),
            summary: String::new(),
            details: String::new(),
        };
        let mic = Ok(MicInfo {
            name: "Test Mic".to_string(),
            input_rate: 16_000,
            channels: 1,
            sample_format: "F32".to_string(),
            source_id: None,
            resampling: false,
            config_note: None,
        });
        let insertion = Ok(());

        let free_lock = Ok(());
        assert!(doctor_ready(&report, &free_lock, &mic, &insertion));

        let held_lock: Result<()> = Err(anyhow::anyhow!("already running"));
        assert!(!doctor_ready(&report, &held_lock, &mic, &insertion));
    }

    #[test]
    fn singleton_lock_blocks_second_holder() {
        let path = crate::test_support::fixture_root("parakit-lock-test", "singleton")
            .join("parakit.lock");

        let first = acquire_singleton_lock_at(&path).expect("first lock should succeed");
        let second = acquire_singleton_lock_at(&path);
        assert!(second.is_err());
        drop(first);
        let third = acquire_singleton_lock_at(&path).expect("lock should release after drop");
        drop(third);
    }
}
