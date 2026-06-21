//! Terminal-aware daemon logging.

use anstyle::{AnsiColor, Style};
use chrono::{SecondsFormat, Utc};
use parakit::build_info;
use std::fmt::Display;
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
            anstream::println!("{msg}");
        }
    }

    /// Print a verbose diagnostic line with an ISO timestamp.
    pub(crate) fn verbose(&self, msg: impl Display) {
        if self.is_verbose() {
            anstream::println!("{} {msg}", style_dim(timestamp()));
        }
    }

    /// Print a warning line to stderr regardless of quiet mode.
    pub(crate) fn warn(&self, msg: impl Display) {
        anstream::eprintln!("{} {msg}", style_warn("parakit: warning:"));
    }

    /// Print an error line to stderr regardless of quiet mode.
    pub(crate) fn error(&self, msg: &str) {
        anstream::eprintln!("{} {msg}", style_error("parakit: error:"));
    }

    /// Print a concise startup banner.
    pub(crate) fn banner(&self, info: BannerInfo<'_>) {
        if self.level == LogLevel::Quiet {
            return;
        }

        anstream::println!("{}", style_title("parakit"));
        anstream::println!("  model: {}", info.model_name);
        anstream::println!("  dtype: {}", info.dtype);
        anstream::println!("  mic:   {}", info.mic.summary());
        if self.is_verbose() {
            for line in info.mic.detail_lines() {
                anstream::println!("  audio: {line}");
            }
            anstream::println!("  path:  {}", info.model_path.display());
            anstream::println!("  rules: {}", info.cleaning);
            anstream::println!("  sounds: {}", info.sounds);
            anstream::println!("  logging: {}", info.transcription_logging);
            anstream::println!("  insert: {}", info.insertion);
            anstream::println!("  threads: {}", info.threads);
            anstream::println!("  backend: {}", info.backend);
            anstream::println!("  device: {}", info.device);
            anstream::println!("  build:");
            for line in build_info::diagnostic_lines() {
                anstream::println!("    {line}");
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
            anstream::println!(
                "{} {}  {}",
                style_clean("Clean:"),
                style_clean_text(cleaned),
                style_dim(format!("({infer_ms:.0}ms)"))
            );
        } else {
            anstream::println!("{}    {}", style_raw("Raw:"), style_raw_text(raw));
            anstream::println!(
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
