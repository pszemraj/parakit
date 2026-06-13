//! Command-line interface definitions for the `parakit` binary.

use clap::{Args, Parser, Subcommand};
use parakit::data_log::LogFormat;
use parakit::inference::DeviceMode;
use std::num::NonZeroUsize;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::inject::PasteMode;

/// Parsed command-line options for daemon mode and subcommands.
#[derive(Parser, Debug)]
#[command(
    name = "parakit",
    version,
    about = "Push-to-talk dictation daemon (Parakeet-TDT via CrispASR).",
    long_about = "Push-to-talk dictation daemon. Hold Ctrl+Space to record, release to transcribe and insert text at the cursor.\n\nDefault mode prints concise status and transcripts. Pass --verbose for diagnostic paths and timings, or --quiet for background daemon mode."
)]
pub(crate) struct Cli {
    /// Subcommand to run instead of the push-to-talk daemon.
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    #[arg(short = 'm', long, value_name = "PATH")]
    pub(crate) model: Option<PathBuf>,

    /// Quiet mode: suppress stdout. Errors and warnings still go to stderr.
    /// Suitable for backgrounding the daemon.
    #[arg(long, short = 'q')]
    pub(crate) quiet: bool,

    /// Verbose diagnostics: paths, backend details, and timing lines.
    #[arg(long, short = 'v', conflicts_with = "quiet")]
    pub(crate) verbose: bool,

    /// CPU inference threads. Defaults to a conservative detected count.
    #[arg(long, value_name = "N")]
    pub(crate) threads: Option<NonZeroUsize>,

    /// Runtime compute device. `auto` uses the best GPU when available and CPU otherwise.
    #[arg(long, value_enum, default_value = "auto")]
    pub(crate) device: DeviceMode,

    /// Batch insertion style. Defaults to terminal paste on Linux and standard paste elsewhere.
    #[arg(long, value_enum)]
    pub(crate) paste_mode: Option<PasteMode>,

    /// Leave dictated text on the clipboard after paste instead of restoring previous clipboard contents.
    #[arg(long)]
    pub(crate) keep_transcript_clipboard: bool,

    /// Disable the audio cues (start / success / error tones).
    #[arg(long)]
    pub(crate) no_sounds: bool,

    /// Disable all text cleaning rules (raw transcript inserted as-is).
    #[arg(long)]
    pub(crate) no_cleaning: bool,

    /// Disable a specific rule by name. Repeatable: `--disable-rule a --disable-rule b`.
    #[arg(long, value_name = "NAME")]
    pub(crate) disable_rule: Vec<String>,

    /// Print all available cleaning rules and exit.
    #[arg(long)]
    pub(crate) list_rules: bool,

    /// Test the rule pipeline against a string and exit. No audio capture.
    /// Useful for iterating on rules.
    ///   `parakit --test-rules "So, um, the the cat ran"`
    #[arg(long, value_name = "INPUT")]
    pub(crate) test_rules: Option<String>,

    /// Hidden validation path: send a WAV through the daemon PTT worker without insertion.
    #[arg(long, hide = true, value_name = "WAV")]
    pub(crate) simulate_ptt_audio: Option<PathBuf>,

    /// Linux hotkey backend. `auto` registers Ctrl+Space with the X11 session.
    #[cfg(target_os = "linux")]
    #[arg(long, value_enum, default_value_t = HotkeyBackend::Auto)]
    pub(crate) hotkey_backend: HotkeyBackend,

    /// Directory for transcription logs. One file is written per local day.
    #[arg(long, value_name = "DIR")]
    pub(crate) log_dir: Option<PathBuf>,

    /// Transcription log format. Used only when --log-dir is set.
    #[arg(long, default_value = "jsonl", value_parser = clap::value_parser!(LogFormat))]
    pub(crate) log_format: LogFormat,
}

/// Top-level subcommands that run instead of the push-to-talk daemon.
#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Download the default hosted Parakeet Q8_0 GGUF.
    Fetch(FetchCli),
    /// Inspect the parakit model cache.
    Cache(CacheCli),
    /// Check runtime prerequisites without starting. Exits 0 when ready, 1 when blocked.
    Doctor(DoctorCli),
    /// Query a running daemon over the local control socket.
    Status,
    /// Stop a running daemon over the local control socket.
    Stop,
    /// Paste the last transcript remembered by the running daemon.
    PasteLast,
    /// Copy the last transcript remembered by the running daemon.
    CopyLast,
    /// Exercise clipboard staging and paste without recording microphone audio.
    TestPaste(TestPasteCli),
}

/// Arguments for model cache inspection commands.
#[derive(Args, Debug)]
pub(crate) struct CacheCli {
    /// Cache subcommand. Defaults to `list`.
    #[command(subcommand)]
    pub(crate) command: Option<CacheCommand>,
}

/// Model cache inspection actions.
#[derive(Subcommand, Debug)]
pub(crate) enum CacheCommand {
    /// List cached model artifacts.
    List,
    /// Print the model cache directory.
    Dir,
}

/// Arguments controlling default model download and rebuild behavior.
#[derive(Args, Debug)]
pub(crate) struct FetchCli {
    /// Ignore cached artifacts and download or rebuild again.
    #[arg(long)]
    pub(crate) force: bool,

    /// Rebuild Q8_0 locally from NVIDIA's official .nemo checkpoint.
    #[arg(long)]
    pub(crate) from_source: bool,

    /// Keep the downloaded 2.4 GB .nemo checkpoint after source rebuild.
    #[arg(long, requires = "from_source")]
    pub(crate) keep_nemo: bool,

    /// Keep the intermediate F16 GGUF after source rebuild.
    #[arg(long, requires = "from_source")]
    pub(crate) keep_f16: bool,
}

/// Arguments for runtime prerequisite checks.
#[derive(Args, Debug)]
pub(crate) struct DoctorCli {
    /// Run active smoke tests in addition to passive preflight checks.
    ///
    /// On Linux/X11 this briefly focuses a tiny probe window and verifies the
    /// configured paste shortcut reaches it.
    #[arg(long)]
    pub(crate) deep: bool,
}

/// Arguments for testing the daemon paste path.
#[derive(Args, Debug)]
pub(crate) struct TestPasteCli {
    /// Text to insert through the running daemon's paste path.
    pub(crate) text: String,
}

impl Cli {
    /// Return the selected paste mode, falling back to the platform default.
    ///
    /// # Returns
    ///
    /// Returns the explicitly configured paste mode or the default for the current platform.
    pub(crate) fn effective_paste_mode(&self) -> PasteMode {
        self.paste_mode.unwrap_or_else(default_paste_mode)
    }

    /// Return the requested runtime compute device mode.
    ///
    /// # Returns
    ///
    /// The library-level device mode selected by the CLI.
    pub(crate) fn effective_device_mode(&self) -> DeviceMode {
        self.device
    }
}

/// Return the platform-specific default paste mode.
///
/// # Returns
///
/// Returns terminal paste on Linux and standard paste on other platforms.
pub(crate) fn default_paste_mode() -> PasteMode {
    #[cfg(target_os = "linux")]
    {
        PasteMode::Terminal
    }

    #[cfg(not(target_os = "linux"))]
    {
        PasteMode::Standard
    }
}
