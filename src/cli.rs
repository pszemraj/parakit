//! Command-line interface definitions for the `parakit` binary.

use clap::{Args, Parser, Subcommand};
use parakit::data_log::LogFormat;
use parakit::inference::DeviceMode;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::config::ConfigFile;
#[cfg(target_os = "linux")]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::inject::PasteMode;

/// Parsed command-line options for daemon mode and subcommands.
#[derive(Parser, Debug)]
#[command(
    name = "parakit",
    version,
    about = "Push-to-talk dictation daemon (Parakeet-TDT via CrispASR).",
    long_about = "Push-to-talk dictation daemon. Hold the platform hotkey to record, release to transcribe and insert text at the cursor.\n\nDefault mode prints concise status and transcripts. Pass --verbose for diagnostic paths and timings, or --quiet for background daemon mode."
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
    /// Defaults to `config.toml`'s `daemon.device`, then `auto`.
    #[arg(long, value_enum)]
    pub(crate) device: Option<DeviceMode>,

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
    /// Defaults to `config.toml`'s `hotkey.backend`, then `auto`.
    #[cfg(target_os = "linux")]
    #[arg(long, value_enum)]
    pub(crate) hotkey_backend: Option<HotkeyBackend>,

    /// Directory for transcription logs. One file is written per local day.
    #[arg(long, value_name = "DIR")]
    pub(crate) log_dir: Option<PathBuf>,

    /// Transcription log format. Used only when --log-dir (or config
    /// `logging.dir`) is set. Defaults to `config.toml`'s `logging.format`,
    /// then `jsonl`.
    #[arg(long, value_parser = clap::value_parser!(LogFormat))]
    pub(crate) log_format: Option<LogFormat>,
}

/// Top-level subcommands that run instead of the push-to-talk daemon.
#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Download the default hosted Parakeet Q8_0 GGUF.
    Fetch(FetchCli),
    /// Inspect the parakit model cache.
    Cache(CacheCli),
    /// Inspect or edit the parakit config file.
    Config(ConfigCli),
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

/// Arguments for config file inspection and editing commands.
#[derive(Args, Debug)]
pub(crate) struct ConfigCli {
    /// Config subcommand. Defaults to `show`.
    #[command(subcommand)]
    pub(crate) command: Option<ConfigCommand>,
}

/// Config file actions.
#[derive(Subcommand, Debug)]
pub(crate) enum ConfigCommand {
    /// Print the resolved config file path.
    Path,
    /// Write a commented template config file.
    Init {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Print the resolved config path and effective merged values.
    Show,
    /// Open the config file in $VISUAL or $EDITOR, creating it from the
    /// template first if it does not exist yet.
    Edit,
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
    /// Opens a throwaway probe window and takes focus for a moment:
    /// Linux/X11 checks that the configured paste shortcut reaches it, macOS
    /// and Windows run a full paste round trip and read the result back. The
    /// probe window closes itself.
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
    /// Return the selected paste mode: CLI flag, then config, then the
    /// platform default.
    ///
    /// # Returns
    ///
    /// Returns the explicitly configured paste mode or the default for the current platform.
    pub(crate) fn effective_paste_mode(&self, config: &ConfigFile) -> PasteMode {
        self.paste_mode
            .or(config.daemon.paste_mode)
            .unwrap_or_else(default_paste_mode)
    }

    /// Return the selected model path: CLI `-m/--model`, then config
    /// `daemon.model`. `None` means "fetch the default hosted model".
    ///
    /// # Returns
    ///
    /// The effective model path override, if any.
    pub(crate) fn effective_model(&self, config: &ConfigFile) -> Option<PathBuf> {
        self.model.clone().or_else(|| config.daemon.model.clone())
    }

    /// Return the selected compute device: CLI `--device`, then config
    /// `daemon.device`, then [`DeviceMode::default`] (`auto`).
    ///
    /// # Returns
    ///
    /// The effective device mode.
    pub(crate) fn effective_device(&self, config: &ConfigFile) -> DeviceMode {
        self.device.or(config.daemon.device).unwrap_or_default()
    }

    /// Return the selected CPU thread count override: CLI `--threads`, then
    /// config `daemon.threads`. `None` means "use the detected default".
    ///
    /// # Returns
    ///
    /// The effective thread count override, if any.
    pub(crate) fn effective_threads(&self, config: &ConfigFile) -> Option<NonZeroUsize> {
        self.threads.or(config.daemon.threads)
    }

    /// Return the selected transcription log directory: CLI `--log-dir`,
    /// then config `logging.dir`. `None` means transcription logging is
    /// disabled.
    ///
    /// # Returns
    ///
    /// The effective log directory, if any.
    pub(crate) fn effective_log_dir(&self, config: &ConfigFile) -> Option<PathBuf> {
        self.log_dir.clone().or_else(|| config.logging.dir.clone())
    }

    /// Return the selected transcription log format: CLI `--log-format`,
    /// then config `logging.format`, then `jsonl`.
    ///
    /// # Returns
    ///
    /// The effective log format.
    pub(crate) fn effective_log_format(&self, config: &ConfigFile) -> LogFormat {
        self.log_format
            .or(config.logging.format)
            .unwrap_or(LogFormat::Jsonl)
    }

    /// Return the selected Linux hotkey backend: CLI `--hotkey-backend`,
    /// then config `hotkey.backend`, then [`HotkeyBackend::Auto`].
    ///
    /// # Returns
    ///
    /// The effective hotkey backend.
    #[cfg(target_os = "linux")]
    pub(crate) fn effective_hotkey_backend(&self, config: &ConfigFile) -> HotkeyBackend {
        self.hotkey_backend
            .or(config.hotkey.backend)
            .unwrap_or(HotkeyBackend::Auto)
    }

    /// Return whether audio cues are enabled: `--no-sounds` always wins;
    /// otherwise falls back to config `daemon.sounds` (default enabled).
    ///
    /// # Returns
    ///
    /// `true` when start/success/error tones should play.
    pub(crate) fn effective_sounds_enabled(&self, config: &ConfigFile) -> bool {
        !self.no_sounds && config.daemon.sounds.unwrap_or(true)
    }

    /// Return whether the text-cleaning pipeline is enabled: `--no-cleaning`
    /// always wins; otherwise falls back to config `cleaning.enabled`
    /// (default enabled).
    ///
    /// # Returns
    ///
    /// `true` when the cleaning pipeline should run.
    pub(crate) fn effective_cleaning_enabled(&self, config: &ConfigFile) -> bool {
        !self.no_cleaning && config.cleaning.enabled.unwrap_or(true)
    }

    /// Return whether the transcript should stay on the clipboard after
    /// paste: CLI `--keep-transcript-clipboard` OR config
    /// `daemon.keep_transcript_clipboard` (default false).
    ///
    /// # Returns
    ///
    /// `true` when the previous clipboard contents should not be restored.
    pub(crate) fn effective_keep_transcript_clipboard(&self, config: &ConfigFile) -> bool {
        self.keep_transcript_clipboard || config.daemon.keep_transcript_clipboard.unwrap_or(false)
    }

    /// Return whether verbose diagnostics are enabled: CLI `--verbose` OR
    /// config `daemon.verbose` (default false).
    ///
    /// `--quiet` and `--verbose` conflict at the CLI level
    /// (`conflicts_with`), so clap itself rejects passing both. A config
    /// `verbose = true` cannot participate in that clap-level conflict
    /// check, so callers that also honor `--quiet` must check `self.quiet`
    /// first (see `app.rs::log_level`): quiet always wins, even over a
    /// config file that requests verbose output.
    ///
    /// # Returns
    ///
    /// `true` when verbose diagnostics should print.
    pub(crate) fn effective_verbose(&self, config: &ConfigFile) -> bool {
        self.verbose || config.daemon.verbose.unwrap_or(false)
    }

    /// Return the union of CLI `--disable-rule` names and config
    /// `cleaning.disabled_rules`, without duplicates.
    ///
    /// # Returns
    ///
    /// The merged list of rule names to disable.
    pub(crate) fn effective_disabled_rules(&self, config: &ConfigFile) -> Vec<String> {
        let mut merged = self.disable_rule.clone();
        for name in &config.cleaning.disabled_rules {
            if !merged.contains(name) {
                merged.push(name.clone());
            }
        }
        merged
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_from(args: &[&str]) -> Cli {
        let mut full = vec!["parakit"];
        full.extend_from_slice(args);
        Cli::parse_from(full)
    }

    #[test]
    fn effective_device_prefers_cli_then_config_then_default() {
        let mut config = ConfigFile::default();
        assert_eq!(
            cli_from(&[]).effective_device(&config),
            DeviceMode::default()
        );

        config.daemon.device = Some(DeviceMode::Gpu);
        assert_eq!(cli_from(&[]).effective_device(&config), DeviceMode::Gpu);

        assert_eq!(
            cli_from(&["--device", "cpu"]).effective_device(&config),
            DeviceMode::Cpu
        );
    }

    #[test]
    fn effective_log_format_prefers_cli_then_config_then_jsonl() {
        let mut config = ConfigFile::default();
        assert_eq!(
            cli_from(&[]).effective_log_format(&config),
            LogFormat::Jsonl
        );

        config.logging.format = Some(LogFormat::Tsv);
        assert_eq!(cli_from(&[]).effective_log_format(&config), LogFormat::Tsv);

        assert_eq!(
            cli_from(&["--log-format", "jsonl"]).effective_log_format(&config),
            LogFormat::Jsonl
        );
    }

    #[test]
    fn effective_paste_mode_prefers_cli_then_config_then_platform_default() {
        let mut config = ConfigFile::default();
        assert_eq!(
            cli_from(&[]).effective_paste_mode(&config),
            default_paste_mode()
        );

        config.daemon.paste_mode = Some(PasteMode::Direct);
        assert_eq!(
            cli_from(&[]).effective_paste_mode(&config),
            PasteMode::Direct
        );

        assert_eq!(
            cli_from(&["--paste-mode", "standard"]).effective_paste_mode(&config),
            PasteMode::Standard
        );
    }

    #[test]
    fn effective_model_and_threads_and_log_dir_prefer_cli_then_config() {
        let mut config = ConfigFile::default();
        config.daemon.model = Some(PathBuf::from("/config/model.gguf"));
        config.daemon.threads = NonZeroUsize::new(2);
        config.logging.dir = Some(PathBuf::from("/config/logs"));

        let cli = cli_from(&[]);
        assert_eq!(
            cli.effective_model(&config),
            Some(PathBuf::from("/config/model.gguf"))
        );
        assert_eq!(cli.effective_threads(&config), NonZeroUsize::new(2));
        assert_eq!(
            cli.effective_log_dir(&config),
            Some(PathBuf::from("/config/logs"))
        );

        let cli = cli_from(&[
            "-m",
            "/cli/model.gguf",
            "--threads",
            "8",
            "--log-dir",
            "/cli/logs",
        ]);
        assert_eq!(
            cli.effective_model(&config),
            Some(PathBuf::from("/cli/model.gguf"))
        );
        assert_eq!(cli.effective_threads(&config), NonZeroUsize::new(8));
        assert_eq!(
            cli.effective_log_dir(&config),
            Some(PathBuf::from("/cli/logs"))
        );
    }

    #[test]
    fn effective_sounds_and_cleaning_use_negated_and_semantics() {
        let mut config = ConfigFile::default();

        // No flags, no config: both enabled by default.
        assert!(cli_from(&[]).effective_sounds_enabled(&config));
        assert!(cli_from(&[]).effective_cleaning_enabled(&config));

        // Config explicitly disables: still enabled unless CLI also cares,
        // since disabling is expressed as `Some(false)`.
        config.daemon.sounds = Some(false);
        config.cleaning.enabled = Some(false);
        assert!(!cli_from(&[]).effective_sounds_enabled(&config));
        assert!(!cli_from(&[]).effective_cleaning_enabled(&config));

        // CLI --no-sounds/--no-cleaning always win, regardless of config.
        config.daemon.sounds = Some(true);
        config.cleaning.enabled = Some(true);
        assert!(!cli_from(&["--no-sounds"]).effective_sounds_enabled(&config));
        assert!(!cli_from(&["--no-cleaning"]).effective_cleaning_enabled(&config));
    }

    #[test]
    fn effective_keep_transcript_clipboard_and_verbose_use_or_semantics() {
        let mut config = ConfigFile::default();
        assert!(!cli_from(&[]).effective_keep_transcript_clipboard(&config));
        assert!(!cli_from(&[]).effective_verbose(&config));

        config.daemon.keep_transcript_clipboard = Some(true);
        config.daemon.verbose = Some(true);
        assert!(cli_from(&[]).effective_keep_transcript_clipboard(&config));
        assert!(cli_from(&[]).effective_verbose(&config));

        config.daemon.keep_transcript_clipboard = Some(false);
        config.daemon.verbose = Some(false);
        assert!(
            cli_from(&["--keep-transcript-clipboard"]).effective_keep_transcript_clipboard(&config)
        );
        assert!(cli_from(&["--verbose"]).effective_verbose(&config));
    }

    #[test]
    fn effective_verbose_config_true_does_not_override_cli_quiet() {
        // `--quiet` and `--verbose` conflict at the CLI level, but a config
        // `verbose = true` cannot participate in that check. Callers must
        // check `cli.quiet` before `effective_verbose` (see `app.rs`), so
        // this test documents that `effective_verbose` alone still reports
        // `true` here -- the "quiet wins" behavior lives in the caller.
        let mut config = ConfigFile::default();
        config.daemon.verbose = Some(true);
        let cli = cli_from(&["--quiet"]);
        assert!(cli.quiet);
        assert!(cli.effective_verbose(&config));
    }

    #[test]
    fn effective_disabled_rules_merges_cli_and_config_without_duplicates() {
        let mut config = ConfigFile::default();
        config.cleaning.disabled_rules = vec![
            "fix-trailing-period".to_string(),
            "filler-um-uh".to_string(),
        ];

        let cli = cli_from(&[
            "--disable-rule",
            "filler-um-uh",
            "--disable-rule",
            "lead-so-comma",
        ]);
        let mut merged = cli.effective_disabled_rules(&config);
        merged.sort();
        let mut expected = vec![
            "filler-um-uh".to_string(),
            "fix-trailing-period".to_string(),
            "lead-so-comma".to_string(),
        ];
        expected.sort();
        assert_eq!(merged, expected);
    }
}
