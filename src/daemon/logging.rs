//! Terminal-aware daemon logging.
//!
//! Output is best-effort: closing the launching terminal must not kill a daemon thread.

use anstyle::{AnsiColor, Style};
use chrono::{SecondsFormat, Utc};
use parakit::build_info;
use parakit::output::ProgressStage;
use std::fmt::Display;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::daemon::{audio::MicInfo, hotkey, ipc::DaemonInfo};

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

    /// Show activity during long work without interleaving verbose diagnostics.
    ///
    /// # Returns
    /// A guard that clears the indicator at the end of the operation.
    pub(crate) fn progress(&self, message: impl Into<String>) -> ProgressStage {
        let message = message.into();
        if self.is_verbose() {
            self.verbose(&message);
        }
        ProgressStage::new(message, self.level != LogLevel::Normal)
    }

    /// Print a verbose diagnostic line with an ISO timestamp.
    pub(crate) fn verbose(&self, msg: impl Display) {
        if self.is_verbose() {
            let _ = writeln!(
                anstream::stdout(),
                "{} {msg}",
                paint(timestamp(), AnsiColor::BrightBlack)
            );
        }
    }

    /// Print a warning line to stderr regardless of quiet mode.
    pub(crate) fn warn(&self, msg: impl Display) {
        let _ = writeln!(
            anstream::stderr(),
            "{} {msg}",
            paint("parakit: warning:", AnsiColor::Yellow)
        );
    }

    /// Print an error line to stderr regardless of quiet mode.
    pub(crate) fn error(&self, msg: &str) {
        let _ = writeln!(
            anstream::stderr(),
            "{} {msg}",
            paint("parakit: error:", AnsiColor::Red)
        );
    }

    /// Print a concise startup banner.
    pub(crate) fn banner(&self, info: BannerInfo<'_>) {
        if self.level == LogLevel::Quiet {
            return;
        }

        let _ = writeln!(anstream::stdout(), "{}", paint("parakit", AnsiColor::Cyan));
        let daemon = info.daemon;
        let _ = writeln!(anstream::stdout(), "  model: {}", daemon.model_name);
        let _ = writeln!(anstream::stdout(), "  dtype: {}", daemon.dtype);
        let _ = writeln!(anstream::stdout(), "  mic:   {}", info.mic.name);
        if self.is_verbose() {
            for line in info.mic.detail_lines() {
                let _ = writeln!(anstream::stdout(), "  audio: {line}");
            }
            let _ = writeln!(anstream::stdout(), "  path:  {}", info.model_path.display());
            let _ = writeln!(anstream::stdout(), "  rules: {}", daemon.cleaning_summary);
            let _ = writeln!(
                anstream::stdout(),
                "  sounds: {}",
                if daemon.sounds_on { "on" } else { "off" }
            );
            let _ = writeln!(
                anstream::stdout(),
                "  logging: {}",
                daemon.log_summary.as_deref().unwrap_or("off")
            );
            let _ = writeln!(anstream::stdout(), "  insert: {}", info.insertion);
            let _ = writeln!(anstream::stdout(), "  threads: {}", daemon.threads);
            let _ = writeln!(anstream::stdout(), "  backend: {}", daemon.backend);
            let _ = writeln!(anstream::stdout(), "  device: {}", daemon.device);
            let _ = writeln!(anstream::stdout(), "  build:");
            for line in build_info::diagnostic_lines() {
                let _ = writeln!(anstream::stdout(), "    {line}");
            }
        }
    }

    /// Print the ready line.
    pub(crate) fn ready(&self) {
        self.line(&format!(
            "Ready: hold {} to dictate. Ctrl+C to stop.",
            hotkey::default_ptt_hint()
        ));
    }

    /// Print a microphone switch notice.
    pub(crate) fn mic_changed(&self, mic: &MicInfo) {
        self.line(&format!("parakit: mic changed: {}", mic.name));
        self.verbose(format!(
            "parakit: microphone configuration: {}",
            mic.summary()
        ));
    }

    /// Print a transcription-start line.
    ///
    /// # Arguments
    ///
    /// * `audio_secs` - Captured audio duration in seconds.
    /// * `wall_secs` - Wall-clock recording duration in seconds.
    pub(crate) fn transcribing(&self, audio_secs: f32, wall_secs: f32) {
        if self.is_verbose() {
            self.line(&format!(
                "parakit: transcribing ({audio_secs:.2}s audio, {wall_secs:.2}s wall)..."
            ));
        } else {
            self.line("parakit: transcribing...");
        }
    }

    /// Print the final transcript; raw text and timing require verbose mode.
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

        if !self.is_verbose() {
            let _ = writeln!(
                anstream::stdout(),
                "{} {}",
                paint("Clean:", AnsiColor::Green),
                paint(cleaned, AnsiColor::BrightGreen)
            );
            return;
        }

        let infer_ms = infer.as_secs_f32() * 1000.0;
        let spacing = if raw == cleaned {
            " "
        } else {
            let _ = writeln!(
                anstream::stdout(),
                "{}    {}",
                paint("Raw:", AnsiColor::Yellow),
                paint(raw, AnsiColor::BrightYellow)
            );
            "  "
        };
        let _ = writeln!(
            anstream::stdout(),
            "{}{spacing}{}  {}",
            paint("Clean:", AnsiColor::Green),
            paint(cleaned, AnsiColor::BrightGreen),
            paint(format!("({infer_ms:.0}ms)"), AnsiColor::BrightBlack)
        );
    }
}

/// Startup fields rendered by [`Logger::banner`].
pub(crate) struct BannerInfo<'a> {
    /// Effective daemon metadata also exposed through IPC status.
    pub(crate) daemon: &'a DaemonInfo,
    /// Full model path for verbose output.
    pub(crate) model_path: &'a Path,
    /// Selected microphone.
    pub(crate) mic: &'a MicInfo,
    /// Text insertion state.
    pub(crate) insertion: String,
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn paint(text: impl Display, color: AnsiColor) -> String {
    let style = Style::new().fg_color(Some(color.into()));
    format!("{}{}{}", style.render(), text, style.render_reset())
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod feedback_tests;

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
                let daemon = DaemonInfo {
                    pid: std::process::id(),
                    model_name: "test.gguf".into(),
                    model_preset: None,
                    qwen_language: None,
                    qwen_max_new_tokens: None,
                    dtype: "Q8_0".into(),
                    mic_summary: mic.summary(),
                    cleaning_summary: "off".into(),
                    sounds_on: false,
                    log_summary: None,
                    threads: 1,
                    backend: "test".into(),
                    device: "cpu".into(),
                    paste_mode: "standard",
                    hotkey_backend_label: None,
                };
                for level in [LogLevel::Quiet, LogLevel::Normal, LogLevel::Verbose] {
                    let log = Logger::new(level);
                    log.warn("focus changed before insertion");
                    log.error("model reload failed");
                    log.line("continuing dictation");
                    log.verbose("diagnostic");
                    log.banner(BannerInfo {
                        daemon: &daemon,
                        model_path: Path::new("target/tmp/test.gguf"),
                        mic: &mic,
                        insertion: "off".into(),
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

        let status = crate::test_support::exact_test_command(
            "daemon::logging::tests::closed_terminal_does_not_stop_runtime_logging",
        )
        .arg("--quiet")
        .env(CHILD, "1")
        .output()
        .unwrap()
        .status;
        assert!(status.success(), "closed-terminal worker exited: {status}");
    }
}
