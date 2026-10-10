//! Client side of the control protocol: helper-command dispatch, daemon-absent handling, and output.

use super::*;

/// Check whether the daemon control endpoint answers a status request.
///
/// # Returns
///
/// `true` only when a live endpoint returns a status response.
///
/// # Errors
///
/// Returns transport errors other than an absent endpoint.
#[cfg(any(unix, target_os = "windows"))]
pub(crate) fn daemon_responsive() -> Result<bool> {
    match send_command(&IpcCommand::Status) {
        Ok(IpcResponse::Status { .. }) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) if error.is::<DaemonNotRunning>() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Run one IPC client command and print a concise response.
///
/// # Arguments
///
/// * `command` - Command to send to the running daemon.
/// * `quiet` - Suppress stdout on success.
/// * `verbose` - Print an extended detail block for `Status` responses. Has
///   no effect on other commands, so their output is unaffected either way.
///
/// # Returns
///
/// `Ok(())` when the command completed successfully.
///
/// # Errors
///
/// Returns an error when a command that needs live daemon state is used while
/// no daemon is listening, or when the daemon reports failure. An absent
/// daemon is a successful state for `Status` and `Stop`.
pub(crate) fn run_client(command: IpcCommand, quiet: bool, verbose: bool) -> Result<()> {
    let stop_deadline =
        Instant::now() + STOP_INSERTION_WAIT + STOP_RESPONSE_GRACE + IPC_TRANSPORT_TIMEOUT;
    let response = match send_command(&command) {
        Ok(response) => response,
        Err(err) if err.is::<DaemonNotRunning>() => {
            return handle_daemon_not_running(&command, quiet, stop_deadline);
        }
        Err(err) => return Err(err),
    };
    match response {
        IpcResponse::Ok { message } => {
            if matches!(&command, IpcCommand::Stop) {
                wait_for_daemon_stop(stop_deadline, false)?;
            }
            if !quiet {
                println!("{}", completed_command_message(&command, &message));
            }
            Ok(())
        }
        IpcResponse::Status {
            phase,
            last_transcript_len,
            detail,
        } => {
            if !quiet {
                // These two lines must stay byte-identical to the pre-verbose
                // output: existing scripts parse them.
                println!("parakit: {phase}");
                println!(
                    "last transcript: {}",
                    last_transcript_summary(last_transcript_len, detail.as_deref())
                );
                if verbose {
                    print_status_detail(detail.as_deref());
                }
            }
            Ok(())
        }
        IpcResponse::History { entries } => {
            if !quiet {
                print_history(&entries);
            }
            Ok(())
        }
        IpcResponse::Err { message } => bail!("{message}"),
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn completed_command_message<'a>(command: &IpcCommand, server_message: &'a str) -> &'a str {
    if matches!(command, IpcCommand::Stop) {
        "stopped"
    } else {
        server_message
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn wait_for_daemon_stop(mut deadline: Instant, stop_pending: bool) -> Result<()> {
    if stop_pending {
        // An absent endpoint can mean startup, not just teardown. Deliver the
        // pending stop within a bounded discovery window, then allow the
        // daemon its full existing worker/insertion shutdown budget.
        while preflight::singleton_lock_held()? && Instant::now() < deadline {
            match send_command(&IpcCommand::Stop) {
                Ok(IpcResponse::Ok { .. }) => {
                    deadline = Instant::now()
                        + STOP_INSERTION_WAIT
                        + STOP_RESPONSE_GRACE
                        + IPC_TRANSPORT_TIMEOUT;
                    break;
                }
                Ok(IpcResponse::Err { message }) => bail!("{message}"),
                Ok(_) => bail!("unexpected daemon stop response"),
                Err(error) if error.is::<DaemonNotRunning>() => {}
                Err(error) => return Err(error),
            }
            thread::sleep(STOP_LOCK_POLL);
        }
    }
    wait_for_daemon_stop_with_probe(deadline, preflight::singleton_lock_held)
}

#[cfg(any(unix, target_os = "windows"))]
fn wait_for_daemon_stop_with_probe(
    deadline: Instant,
    mut lock_held: impl FnMut() -> Result<bool>,
) -> Result<()> {
    while lock_held().context("check daemon shutdown")? {
        if Instant::now() >= deadline {
            #[cfg(not(target_os = "windows"))]
            bail!("daemon stop timed out; if this persists, identify the daemon with `pgrep -af parakit` and end that process");
            #[cfg(target_os = "windows")]
            bail!("daemon stop timed out; if this persists, identify the daemon with `Get-Process parakit` and end it with `Stop-Process -Id <PID>`");
        }
        thread::sleep(STOP_LOCK_POLL);
    }
    Ok(())
}

#[cfg(any(unix, target_os = "windows"))]
#[derive(Debug, Eq, PartialEq)]
enum DaemonNotRunningDisposition {
    Success(&'static str),
    WaitForStop,
    Error(&'static str),
}

/// Return the user-facing result for a command when no daemon endpoint exists.
///
/// Status queries and stop requests are idempotent state checks once the
/// singleton lock is free. A held lock means the daemon may still be starting
/// or stopping, so reporting a settled "not running" state would be misleading.
#[cfg(any(unix, target_os = "windows"))]
fn daemon_not_running_disposition(
    command: &IpcCommand,
    singleton_held: bool,
) -> DaemonNotRunningDisposition {
    match command {
        IpcCommand::Stop if singleton_held => DaemonNotRunningDisposition::WaitForStop,
        IpcCommand::Status if singleton_held => {
            DaemonNotRunningDisposition::Error(
                "daemon control endpoint is unavailable while the singleton lock is held; it may still be starting or stopping",
            )
        }
        IpcCommand::Status => DaemonNotRunningDisposition::Success("parakit: not running"),
        IpcCommand::Stop => {
            DaemonNotRunningDisposition::Success("parakit: not running; nothing to stop")
        }
        IpcCommand::CopyLast { .. } | IpcCommand::History { .. } | IpcCommand::TestPaste { .. } => {
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            )
        }
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn handle_daemon_not_running(command: &IpcCommand, quiet: bool, deadline: Instant) -> Result<()> {
    let singleton_held = if matches!(command, IpcCommand::Status | IpcCommand::Stop) {
        preflight::singleton_lock_held().context("probe existing daemon singleton lock")?
    } else {
        false
    };
    match daemon_not_running_disposition(command, singleton_held) {
        DaemonNotRunningDisposition::WaitForStop => {
            wait_for_daemon_stop(deadline, true)?;
            if !quiet {
                println!("stopped");
            }
            Ok(())
        }
        DaemonNotRunningDisposition::Success(message) => {
            if !quiet {
                println!("{message}");
            }
            Ok(())
        }
        DaemonNotRunningDisposition::Error(message) => bail!("{message}"),
    }
}

/// Print the `History` command's transcript listing.
///
/// # Arguments
///
/// * `entries` - Transcripts remembered by the daemon, newest first.
fn print_history(entries: &[HistoryEntry]) {
    if entries.is_empty() {
        println!("no transcripts remembered in this daemon session");
        return;
    }
    for entry in entries {
        println!(
            "{index:<3}{age:<10}{chars:>4} chars  {preview}",
            index = entry.index,
            age = format_age(entry.age_secs),
            chars = entry.chars,
            preview = entry.preview,
        );
    }
}

/// Print the `--verbose status` detail block.
///
/// # Arguments
///
/// * `detail` - Extended runtime detail from the daemon's `Status` response,
///   or `None` when the daemon has not yet called `set_info` (e.g. still
///   starting up).
fn print_status_detail(detail: Option<&StatusDetail>) {
    let Some(detail) = detail else {
        println!("  detail unavailable (daemon starting)");
        return;
    };
    println!("  pid:        {}", detail.pid);
    println!("  uptime:     {}", format_uptime(detail.uptime_secs));
    println!("  dictations: {}", detail.dictation_count);
    println!("  mic:        {}", detail.mic);
    println!("  model:      {} ({})", detail.model, detail.dtype);
    if let Some(model) = &detail.model_status {
        println!("  residency:  {}", model.residency.label());
        if model.idle_minutes == 0 {
            println!("  model idle: disabled");
        } else {
            println!("  model idle: {} minutes", model.idle_minutes);
        }
        if let Some(error) = &model.last_error {
            println!("  model error: {error}");
        }
    }
    println!(
        "  device:     {} ({}, {} threads)",
        detail.device, detail.backend, detail.threads
    );
    println!("  paste mode: {}", detail.paste_mode);
    println!("  sounds:     {}", if detail.sounds { "on" } else { "off" });
    println!("  cleaning:   {}", detail.cleaning);
    println!("  logging:    {}", detail.log.as_deref().unwrap_or("off"));
    println!("  history:    {}", detail.history);
    if let Some(hotkey_backend) = &detail.hotkey_backend {
        println!("  hotkey:     {hotkey_backend}");
    }
}

/// Humanize a duration in seconds as `1h 23m`, `23m 5s`, or `5s`.
///
/// # Returns
///
/// The largest two non-zero units, or `0s` for a zero duration.
fn format_uptime(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Humanize a transcript's age as `format_uptime` output plus `" ago"`.
///
/// # Returns
///
/// e.g. `"12s ago"`, `"3m 5s ago"`, or `"1h 23m ago"`.
fn format_age(age_secs: u64) -> String {
    format!("{} ago", format_uptime(age_secs))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
