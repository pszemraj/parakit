//! PulseAudio/PipeWire source enrichment through `pactl`.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::daemon::subprocess::wait_with_timeout;

const PACTL_TIMEOUT: Duration = Duration::from_millis(750);

/// Human-readable source details parsed from `pactl list sources`.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct PactlSourceInfo {
    /// PulseAudio/PipeWire source name.
    pub(crate) name: String,
    /// Human-readable source description.
    pub(crate) description: Option<String>,
    /// Source sample rate.
    pub(crate) rate: Option<u32>,
    /// Source channel count.
    pub(crate) channels: Option<u16>,
    /// Source sample format label.
    pub(crate) sample_format: Option<String>,
}

/// Read the current default PulseAudio/PipeWire source with `pactl`.
///
/// # Returns
///
/// Parsed details for the default source, or `None` when `pactl` is missing or
/// the output cannot be matched.
pub(crate) fn pactl_default_source_info() -> Option<PactlSourceInfo> {
    let default_name = pactl_default_source_name()?;

    let sources = pactl_output(&["list", "sources"])?;
    if !sources.status.success() {
        return None;
    }
    let sources = String::from_utf8_lossy(&sources.stdout);
    parse_pactl_sources(&sources)
        .into_iter()
        .find(|source| source.name == default_name)
}

/// Read the current default PulseAudio/PipeWire source name with `pactl`.
///
/// # Returns
///
/// The default source name, or `None` when `pactl` is missing, times out, or
/// returns an empty value.
pub(crate) fn pactl_default_source_name() -> Option<String> {
    let default = pactl_output(&["get-default-source"])?;
    if !default.status.success() {
        return None;
    }
    let default_name = String::from_utf8_lossy(&default.stdout).trim().to_string();
    if default_name.is_empty() {
        return None;
    }
    Some(default_name)
}

fn pactl_output(args: &[&str]) -> Option<Output> {
    let mut command = Command::new("pactl");
    command.args(args);
    command_output_with_timeout(&mut command, PACTL_TIMEOUT)
}

fn command_output_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    let deadline = Instant::now() + timeout;
    let status = wait_with_timeout(&mut child, timeout).ok()??;
    let stdout = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?
        .ok()?;
    Some(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn parse_pactl_sources(text: &str) -> Vec<PactlSourceInfo> {
    let mut out = Vec::new();
    let mut current: Option<PactlSourceInfo> = None;

    for line in text.lines() {
        if line.starts_with("Source #") {
            if let Some(source) = current.take() {
                out.push(source);
            }
            current = Some(PactlSourceInfo::default());
            continue;
        }

        let Some(source) = current.as_mut() else {
            continue;
        };
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("Name: ") {
            source.name = name.trim().to_string();
        } else if let Some(description) = trimmed.strip_prefix("Description: ") {
            source.description = Some(description.trim().to_string());
        } else if let Some(spec) = trimmed.strip_prefix("Sample Specification: ") {
            let (sample_format, channels, rate) = parse_sample_spec(spec.trim());
            source.sample_format = sample_format;
            source.channels = channels;
            source.rate = rate;
        }
    }

    if let Some(source) = current {
        out.push(source);
    }
    out
}

fn parse_sample_spec(spec: &str) -> (Option<String>, Option<u16>, Option<u32>) {
    let mut parts = spec.split_whitespace();
    let sample_format = parts.next().map(str::to_string);
    let channels = parts
        .next()
        .and_then(|part| part.strip_suffix("ch"))
        .and_then(|part| part.parse().ok());
    let rate = parts
        .next()
        .and_then(|part| part.strip_suffix("Hz"))
        .and_then(|part| part.parse().ok());
    (sample_format, channels, rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pactl_source_parser_extracts_description_and_rate() {
        let sources = parse_pactl_sources(
            r#"Source #42
    Name: alsa_input.usb-Test_Speech_Mic-00.mono-fallback
    Description: USB Speech Mic Mono
    Sample Specification: s24le 1ch 48000Hz
Source #43
    Name: alsa_output.pci-0000_00.monitor
    Description: Monitor of HDMI Audio
    Sample Specification: s32le 2ch 48000Hz
"#,
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0],
            PactlSourceInfo {
                name: "alsa_input.usb-Test_Speech_Mic-00.mono-fallback".to_string(),
                description: Some("USB Speech Mic Mono".to_string()),
                rate: Some(48_000),
                channels: Some(1),
                sample_format: Some("s24le".to_string()),
            }
        );
    }

    #[test]
    fn command_output_drains_large_stdout_while_waiting() {
        const CHILD: &str = "PARAKIT_PACTL_OUTPUT_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            std::io::stdout().write_all(&vec![b'x'; 300_000]).unwrap();
            return;
        }

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "daemon::audio::pactl::tests::command_output_drains_large_stdout_while_waiting",
                "--nocapture",
            ])
            .env(CHILD, "1");
        let output = command_output_with_timeout(&mut command, Duration::from_secs(1))
            .expect("large output should not block child completion");

        assert!(output.status.success());
        assert!(output.stdout.len() >= 300_000);
    }

    #[test]
    #[allow(clippy::zombie_processes)] // The descendant outlives its parent to hold stdout open.
    fn command_output_stops_waiting_for_descendant_stdout() {
        const ROLE: &str = "PARAKIT_PACTL_INHERITED_STDOUT_ROLE";
        match std::env::var(ROLE).as_deref() {
            Ok("descendant") => {
                thread::sleep(Duration::from_millis(600));
                return;
            }
            Ok("child") => {
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "daemon::audio::pactl::tests::command_output_stops_waiting_for_descendant_stdout",
                        "--nocapture",
                    ])
                    .env(ROLE, "descendant")
                    .spawn()
                    .unwrap();
                return;
            }
            _ => {}
        }

        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "daemon::audio::pactl::tests::command_output_stops_waiting_for_descendant_stdout",
                "--nocapture",
            ])
            .env(ROLE, "child");
        let started = Instant::now();
        let output = command_output_with_timeout(&mut command, Duration::from_millis(200));

        assert!(
            output.is_none(),
            "inherited stdout should exceed the budget"
        );
        assert!(started.elapsed() < Duration::from_millis(450));
    }
}
