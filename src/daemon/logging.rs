//! Terminal-aware daemon logging.
//!
//! Output is best-effort: closing the launching terminal must not kill a daemon thread.

use anstyle::{AnsiColor, Style};
use chrono::{SecondsFormat, Utc};
use parakit::build_info;
use std::fmt::Display;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::daemon::{audio::MicInfo, hotkey};

/// Runtime logging level selected by CLI flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogLevel {
    /// Suppress stdout status output.
    Quiet,
    /// Print concise daemon status and transcripts.
    Normal,
    /// Print diagnostic paths, timings, and backend details.
    Verbose,
}

/// Shared logger used by daemon threads.
#[derive(Debug)]
pub(crate) struct Logger {
    level: LogLevel,
}

impl Logger {
    /// Build a logger for the requested level.
    ///
    /// # Returns
    ///
    /// A logger that writes stdout only when the level is not quiet.
    pub(crate) fn new(level: LogLevel) -> Self {
        Self { level }
    }

    /// Return whether verbose diagnostics are enabled.
    ///
    /// # Returns
    ///
    /// `true` when `--verbose` was passed.
    pub(crate) fn is_verbose(&self) -> bool {
        self.level == LogLevel::Verbose
    }

    /// Print a normal status line.
    pub(crate) fn line(&self, msg: &str) {
        if self.level != LogLevel::Quiet {
            let _ = writeln!(anstream::stdout(), "{msg}");
        }
    }

    /// Print a verbose diagnostic line with an ISO timestamp.
    pub(crate) fn verbose(&self, msg: impl Display) {
        if self.is_verbose() {
            let _ = writeln!(anstream::stdout(), "{} {msg}", style_dim(timestamp()));
        }
    }

    /// Print a warning line to stderr regardless of quiet mode.
    pub(crate) fn warn(&self, msg: impl Display) {
        let _ = writeln!(
            anstream::stderr(),
            "{} {msg}",
            style_warn("parakit: warning:")
        );
    }

    /// Print an error line to stderr regardless of quiet mode.
    pub(crate) fn error(&self, msg: &str) {
        let _ = writeln!(
            anstream::stderr(),
            "{} {msg}",
            style_error("parakit: error:")
        );
    }

    /// Print a concise startup banner.
    pub(crate) fn banner(&self, info: BannerInfo<'_>) {
        if self.level == LogLevel::Quiet {
            return;
        }

        let _ = writeln!(anstream::stdout(), "{}", style_title("parakit"));
        let _ = writeln!(anstream::stdout(), "  model: {}", info.model_name);
        let _ = writeln!(anstream::stdout(), "  dtype: {}", info.dtype);
        let _ = writeln!(anstream::stdout(), "  mic:   {}", info.mic.summary());
        if self.is_verbose() {
            for line in info.mic.detail_lines() {
                let _ = writeln!(anstream::stdout(), "  audio: {line}");
            }
            let _ = writeln!(anstream::stdout(), "  path:  {}", info.model_path.display());
            let _ = writeln!(anstream::stdout(), "  rules: {}", info.cleaning);
            let _ = writeln!(anstream::stdout(), "  sounds: {}", info.sounds);
            let _ = writeln!(
                anstream::stdout(),
                "  logging: {}",
                info.transcription_logging
            );
            let _ = writeln!(anstream::stdout(), "  insert: {}", info.insertion);
            let _ = writeln!(anstream::stdout(), "  threads: {}", info.threads);
            let _ = writeln!(anstream::stdout(), "  backend: {}", info.backend);
            let _ = writeln!(anstream::stdout(), "  device: {}", info.device);
            let _ = writeln!(anstream::stdout(), "  build:");
            for line in build_info::diagnostic_lines() {
                let _ = writeln!(anstream::stdout(), "    {line}");
            }
        }
    }

    /// Print the ready line.
    pub(crate) fn ready(&self) {
        self.line(&format!(
            "Ready: hold {} to dictate.",
            hotkey::default_ptt_hint()
        ));
        if self.is_verbose() {
            self.line("Ctrl+C in this terminal to exit.");
        }
    }

    /// Print a microphone switch notice.
    pub(crate) fn mic_changed(&self, mic: &MicInfo) {
        self.line(&format!("parakit: mic changed: {}", mic.summary()));
    }

    /// Print a transcription-start line.
    ///
    /// # Arguments
    ///
    /// * `audio_secs` - Captured audio duration in seconds.
    /// * `wall_secs` - Wall-clock recording duration in seconds.
    pub(crate) fn transcribing(&self, audio_secs: f32, wall_secs: f32) {
        self.line(&format!(
            "parakit: transcribing ({audio_secs:.2}s audio, {wall_secs:.2}s wall)..."
        ));
    }

    /// Print one transcript pair and inference timing.
    ///
    /// # Arguments
    ///
    /// * `raw` - Transcript returned by the model.
    /// * `cleaned` - Transcript after optional cleanup rules.
    /// * `infer` - Time spent in model inference.
    pub(crate) fn transcript(&self, raw: &str, cleaned: &str, infer: Duration) {
        if self.level == LogLevel::Quiet {
            return;
        }

        let infer_ms = infer.as_secs_f32() * 1000.0;
        if raw == cleaned {
            let _ = writeln!(
                anstream::stdout(),
                "{} {}  {}",
                style_clean("Clean:"),
                style_clean_text(cleaned),
                style_dim(format!("({infer_ms:.0}ms)"))
            );
        } else {
            let _ = writeln!(
                anstream::stdout(),
                "{}    {}",
                style_raw("Raw:"),
                style_raw_text(raw)
            );
            let _ = writeln!(
                anstream::stdout(),
                "{}  {}  {}",
                style_clean("Clean:"),
                style_clean_text(cleaned),
                style_dim(format!("({infer_ms:.0}ms)"))
            );
        }
    }
}

/// Startup fields rendered by [`Logger::banner`].
pub(crate) struct BannerInfo<'a> {
    /// Model file name.
    pub(crate) model_name: &'a str,
    /// Full model path for verbose output.
    pub(crate) model_path: &'a Path,
    /// Dtype and size label.
    pub(crate) dtype: &'a str,
    /// Selected microphone.
    pub(crate) mic: &'a MicInfo,
    /// Cleaning state label.
    pub(crate) cleaning: String,
    /// Sounds state label.
    pub(crate) sounds: &'a str,
    /// Transcription logging state.
    pub(crate) transcription_logging: String,
    /// Text insertion state.
    pub(crate) insertion: String,
    /// Inference thread count.
    pub(crate) threads: usize,
    /// CrispASR backend label.
    pub(crate) backend: String,
    /// Requested runtime compute device.
    pub(crate) device: String,
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn paint(text: impl Display, style: Style) -> String {
    format!("{}{}{}", style.render(), text, style.render_reset())
}

fn style_title(text: impl Display) -> String {
    paint(text, Style::new().fg_color(Some(AnsiColor::Cyan.into())))
}

fn style_raw(text: impl Display) -> String {
    paint(text, Style::new().fg_color(Some(AnsiColor::Yellow.into())))
}

fn style_clean(text: impl Display) -> String {
    paint(text, Style::new().fg_color(Some(AnsiColor::Green.into())))
}

fn style_raw_text(text: impl Display) -> String {
    paint(
        text,
        Style::new().fg_color(Some(AnsiColor::BrightYellow.into())),
    )
}

fn style_clean_text(text: impl Display) -> String {
    paint(
        text,
        Style::new().fg_color(Some(AnsiColor::BrightGreen.into())),
    )
}

fn style_warn(text: impl Display) -> String {
    paint(text, Style::new().fg_color(Some(AnsiColor::Yellow.into())))
}

fn style_error(text: impl Display) -> String {
    paint(text, Style::new().fg_color(Some(AnsiColor::Red.into())))
}

fn style_dim(text: impl Display) -> String {
    paint(
        text,
        Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn closed_terminal_does_not_stop_runtime_logging() {
        const CHILD: &str = "PARAKIT_CLOSED_TERMINAL_LOGGER_CHILD";
        if std::env::var_os(CHILD).is_some() {
            crate::test_support::disconnect_test_terminal();
            std::thread::spawn(|| {
                let mic = MicInfo {
                    name: "test microphone".into(),
                    input_rate: 16_000,
                    channels: 1,
                    sample_format: "f32".into(),
                    source_id: None,
                    resampling: false,
                    config_note: None,
                };
                for level in [LogLevel::Quiet, LogLevel::Normal, LogLevel::Verbose] {
                    let log = Logger::new(level);
                    log.warn("focus changed before insertion");
                    log.error("model reload failed");
                    log.line("continuing dictation");
                    log.verbose("diagnostic");
                    log.banner(BannerInfo {
                        model_name: "test.gguf",
                        model_path: Path::new("target/tmp/test.gguf"),
                        dtype: "Q8_0",
                        mic: &mic,
                        cleaning: "off".into(),
                        sounds: "off",
                        transcription_logging: "off".into(),
                        insertion: "off".into(),
                        threads: 1,
                        backend: "test".into(),
                        device: "cpu".into(),
                    });
                    log.ready();
                    log.mic_changed(&mic);
                    log.transcribing(1.0, 1.0);
                    log.transcript("first", "First", Duration::ZERO);
                    log.transcript("next dictation", "next dictation", Duration::ZERO);
                }
            })
            .join()
            .expect("terminal write failures must not kill the worker");
            // The test harness itself prints through panicking std macros.
            std::process::exit(0);
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::logging::tests::closed_terminal_does_not_stop_runtime_logging",
                "--nocapture",
                "--quiet",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "closed-terminal worker exited: {status}");
    }
}
