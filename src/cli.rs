//! Command-line interface definitions for the `parakit` binary.

use clap::{Args, Parser, Subcommand};
use parakit::inference::DeviceMode;
use parakit::rules::CleaningProfile;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::config::ConfigFile;
#[cfg(target_os = "linux")]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::inject::PasteMode;
use crate::daemon::ipc::DEFAULT_TRANSCRIPT_HISTORY;

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

    /// Leave dictated text on the clipboard after paste instead of restoring
    /// previous clipboard contents. Overrides a configured
    /// `daemon.keep_transcript_clipboard = false`; conflicts with
    /// `--no-keep-transcript-clipboard`.
    #[arg(long, conflicts_with = "no_keep_transcript_clipboard")]
    pub(crate) keep_transcript_clipboard: bool,

    /// Restore the previous clipboard contents after paste instead of leaving
    /// the transcript. Overrides a configured
    /// `daemon.keep_transcript_clipboard = true`; conflicts with
    /// `--keep-transcript-clipboard`.
    #[arg(long, conflicts_with = "keep_transcript_clipboard")]
    pub(crate) no_keep_transcript_clipboard: bool,

    /// Disable the audio cues (start / success / error tones). Overrides a
    /// configured `daemon.sounds = true`; conflicts with `--sounds`.
    #[arg(long, conflicts_with = "sounds")]
    pub(crate) no_sounds: bool,

    /// Enable the audio cues (start / success / error tones). Overrides a
    /// configured `daemon.sounds = false`; conflicts with `--no-sounds`.
    #[arg(long, conflicts_with = "no_sounds")]
    pub(crate) sounds: bool,

    /// Disable all text cleaning rules (raw transcript inserted as-is).
    /// Overrides a configured `cleaning.enabled = true`; conflicts with
    /// `--cleaning`.
    #[arg(long, conflicts_with = "cleaning")]
    pub(crate) no_cleaning: bool,

    /// Enable the text cleaning pipeline. Overrides a configured
    /// `cleaning.enabled = false`; conflicts with `--no-cleaning`.
    #[arg(long, conflicts_with = "no_cleaning")]
    pub(crate) cleaning: bool,

    /// Cleanup behavior tier. `safe` keeps semantic discourse markers such as
    /// comparative `like`; `aggressive` also deletes them. Defaults to
    /// `config.toml`'s `cleaning.profile`, then `safe`.
    #[arg(long, value_name = "PROFILE", value_parser = clap::value_parser!(CleaningProfile))]
    pub(crate) cleaning_profile: Option<CleaningProfile>,

    /// Keep the single terminal period that messaging-style cleanup removes by
    /// default. Overrides a configured `cleaning.keep_trailing_period = false`;
    /// conflicts with `--no-keep-trailing-period`.
    #[arg(long, conflicts_with = "no_keep_trailing_period")]
    pub(crate) keep_trailing_period: bool,

    /// Drop the single terminal period even when configured to keep it.
    /// Overrides a configured `cleaning.keep_trailing_period = true`;
    /// conflicts with `--keep-trailing-period`.
    #[arg(long, conflicts_with = "keep_trailing_period")]
    pub(crate) no_keep_trailing_period: bool,

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

    /// Directory for JSONL transcription logs. One file is written per local day.
    #[arg(long, value_name = "DIR")]
    pub(crate) log_dir: Option<PathBuf>,
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
    /// Paste a transcript remembered by the running daemon.
    PasteLast(HistoryRefCli),
    /// Copy a transcript remembered by the running daemon.
    CopyLast(HistoryRefCli),
    /// List transcripts the running daemon is holding in memory.
    History(HistoryCli),
    /// Exercise clipboard staging and paste without recording microphone audio.
    TestPaste(TestPasteCli),
}

/// Arguments shared by `paste-last` and `copy-last`.
#[derive(Args, Debug)]
pub(crate) struct HistoryRefCli {
    /// Which remembered transcript to use, counting back from the most
    /// recent. 1 is the latest.
    pub(crate) index: Option<usize>,
}

/// Arguments for listing remembered transcripts.
#[derive(Args, Debug)]
pub(crate) struct HistoryCli {
    /// Show at most this many entries.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
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

    /// Return whether audio cues are enabled: explicit `--sounds` forces
    /// `true`, explicit `--no-sounds` forces `false`; with neither flag, falls
    /// back to config `daemon.sounds` (default enabled). The two flags
    /// conflict with each other, so at most one applies per invocation.
    ///
    /// # Returns
    ///
    /// `true` when start/success/error tones should play.
    pub(crate) fn effective_sounds_enabled(&self, config: &ConfigFile) -> bool {
        if self.sounds {
            true
        } else if self.no_sounds {
            false
        } else {
            config.daemon.sounds.unwrap_or(true)
        }
    }

    /// Return whether the text-cleaning pipeline is enabled: explicit
    /// `--cleaning` forces `true`, explicit `--no-cleaning` forces `false`;
    /// with neither flag, falls back to config `cleaning.enabled` (default
    /// enabled). The two flags conflict with each other, so at most one
    /// applies per invocation.
    ///
    /// # Returns
    ///
    /// `true` when the cleaning pipeline should run.
    pub(crate) fn effective_cleaning_enabled(&self, config: &ConfigFile) -> bool {
        if self.cleaning {
            true
        } else if self.no_cleaning {
            false
        } else {
            config.cleaning.enabled.unwrap_or(true)
        }
    }

    /// Return the cleanup behavior tier: CLI `--cleaning-profile`, then config
    /// `cleaning.profile`, then [`CleaningProfile::Safe`].
    ///
    /// The safe default is deliberate. Broad semantic editing, in particular
    /// deleting discourse markers such as `like`, `you know`, and `I mean`,
    /// only happens when the caller opts in to `aggressive`.
    ///
    /// # Arguments
    ///
    /// * `config` - Loaded configuration file values.
    ///
    /// # Returns
    ///
    /// The profile the cleaner should be built with.
    pub(crate) fn effective_cleaning_profile(&self, config: &ConfigFile) -> CleaningProfile {
        self.cleaning_profile
            .or(config.cleaning.profile)
            .unwrap_or(CleaningProfile::Safe)
    }

    /// Return whether cleanup should drop one terminal period: explicit
    /// `--keep-trailing-period` forces it kept (`false`), explicit
    /// `--no-keep-trailing-period` forces it dropped (`true`); with neither
    /// flag, falls back to the inverse of config
    /// `cleaning.keep_trailing_period` (default kept `false`, i.e. dropped by
    /// default). The two flags conflict with each other, so at most one
    /// applies per invocation.
    ///
    /// Dropping the period is the default because parakit is used mostly for
    /// messaging-style dictation, where a trailing period reads as terse.
    ///
    /// # Arguments
    ///
    /// * `config` - Loaded configuration file values.
    ///
    /// # Returns
    ///
    /// `true` when the terminal-period pass should be enabled.
    pub(crate) fn effective_drops_trailing_period(&self, config: &ConfigFile) -> bool {
        if self.keep_trailing_period {
            false
        } else if self.no_keep_trailing_period {
            true
        } else {
            !config.cleaning.keep_trailing_period.unwrap_or(false)
        }
    }

    /// Return whether the transcript should stay on the clipboard after
    /// paste: explicit `--keep-transcript-clipboard` forces `true`, explicit
    /// `--no-keep-transcript-clipboard` forces `false`; with neither flag,
    /// falls back to config `daemon.keep_transcript_clipboard` (default
    /// false). The two flags conflict with each other, so at most one applies
    /// per invocation.
    ///
    /// # Returns
    ///
    /// `true` when the previous clipboard contents should not be restored.
    pub(crate) fn effective_keep_transcript_clipboard(&self, config: &ConfigFile) -> bool {
        if self.keep_transcript_clipboard {
            true
        } else if self.no_keep_transcript_clipboard {
            false
        } else {
            config.daemon.keep_transcript_clipboard.unwrap_or(false)
        }
    }

    /// Return whether verbose diagnostics are enabled: not CLI `--quiet`,
    /// and either CLI `--verbose` OR config `daemon.verbose` (default false).
    ///
    /// `--quiet` and `--verbose` conflict at the CLI level
    /// (`conflicts_with`), but a config `verbose = true` cannot participate
    /// in that check. Folding `quiet` into this helper keeps native-library
    /// logging and application logging under the same quiet-mode contract.
    ///
    /// # Returns
    ///
    /// `true` when verbose diagnostics should print.
    pub(crate) fn effective_verbose(&self, config: &ConfigFile) -> bool {
        !self.quiet && (self.verbose || config.daemon.verbose.unwrap_or(false))
    }

    /// Return the number of transcripts kept in daemon memory: config
    /// `daemon.transcript_history`, then [`DEFAULT_TRANSCRIPT_HISTORY`].
    /// `0` disables `paste-last`, `copy-last`, and `history`.
    ///
    /// # Returns
    ///
    /// The effective transcript history depth.
    pub(crate) fn effective_transcript_history(&self, config: &ConfigFile) -> usize {
        config
            .daemon
            .transcript_history
            .unwrap_or(DEFAULT_TRANSCRIPT_HISTORY)
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
    fn removed_log_format_flag_is_rejected() {
        let error = Cli::try_parse_from(["parakit", "--log-format", "jsonl"])
            .expect_err("--log-format must not remain a supported surface");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
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
    fn sounds_flag_pair_overrides_config_in_both_directions() {
        let mut config = ConfigFile::default();

        // (c) No flags: falls through to config, then built-in default (true).
        assert!(cli_from(&[]).effective_sounds_enabled(&config));
        config.daemon.sounds = Some(false);
        assert!(!cli_from(&[]).effective_sounds_enabled(&config));

        // (a) Positive flag overrides a config `false`.
        assert!(cli_from(&["--sounds"]).effective_sounds_enabled(&config));

        // (b) Negative flag overrides a config `true`.
        config.daemon.sounds = Some(true);
        assert!(!cli_from(&["--no-sounds"]).effective_sounds_enabled(&config));

        // (d) The pair conflicts.
        let error = Cli::try_parse_from(["parakit", "--sounds", "--no-sounds"])
            .expect_err("--sounds and --no-sounds must conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn cleaning_flag_pair_overrides_config_in_both_directions() {
        let mut config = ConfigFile::default();

        // (c) No flags: falls through to config, then built-in default (true).
        assert!(cli_from(&[]).effective_cleaning_enabled(&config));
        config.cleaning.enabled = Some(false);
        assert!(!cli_from(&[]).effective_cleaning_enabled(&config));

        // (a) Positive flag overrides a config `false`.
        assert!(cli_from(&["--cleaning"]).effective_cleaning_enabled(&config));

        // (b) Negative flag overrides a config `true`.
        config.cleaning.enabled = Some(true);
        assert!(!cli_from(&["--no-cleaning"]).effective_cleaning_enabled(&config));

        // (d) The pair conflicts.
        let error = Cli::try_parse_from(["parakit", "--cleaning", "--no-cleaning"])
            .expect_err("--cleaning and --no-cleaning must conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn effective_cleaning_profile_prefers_cli_then_config_then_safe() {
        let mut config = ConfigFile::default();

        // Safe is the shipped default: broad semantic editing, in particular
        // deleting `like`/`you know`/`I mean`, must stay opt-in.
        assert_eq!(
            cli_from(&[]).effective_cleaning_profile(&config),
            CleaningProfile::Safe
        );

        config.cleaning.profile = Some(CleaningProfile::Aggressive);
        assert_eq!(
            cli_from(&[]).effective_cleaning_profile(&config),
            CleaningProfile::Aggressive
        );

        assert_eq!(
            cli_from(&["--cleaning-profile", "safe"]).effective_cleaning_profile(&config),
            CleaningProfile::Safe
        );
    }

    #[test]
    fn keep_trailing_period_flag_pair_overrides_config_in_both_directions() {
        let mut config = ConfigFile::default();

        // (c) No flags: falls through to config, then built-in default
        // (dropped). Messaging-style dictation drops the trailing period
        // unless the user positively asks to keep it.
        assert!(cli_from(&[]).effective_drops_trailing_period(&config));
        config.cleaning.keep_trailing_period = Some(true);
        assert!(!cli_from(&[]).effective_drops_trailing_period(&config));

        // (a) Positive flag overrides a config `false` (would otherwise drop it).
        config.cleaning.keep_trailing_period = Some(false);
        assert!(!cli_from(&["--keep-trailing-period"]).effective_drops_trailing_period(&config));

        // (b) Negative flag overrides a config `true` (would otherwise keep it).
        config.cleaning.keep_trailing_period = Some(true);
        assert!(cli_from(&["--no-keep-trailing-period"]).effective_drops_trailing_period(&config));

        // (d) The pair conflicts.
        let error = Cli::try_parse_from([
            "parakit",
            "--keep-trailing-period",
            "--no-keep-trailing-period",
        ])
        .expect_err("--keep-trailing-period and --no-keep-trailing-period must conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn keep_transcript_clipboard_flag_pair_overrides_config_in_both_directions() {
        let mut config = ConfigFile::default();

        // (c) No flags: falls through to config, then built-in default (false).
        assert!(!cli_from(&[]).effective_keep_transcript_clipboard(&config));
        config.daemon.keep_transcript_clipboard = Some(true);
        assert!(cli_from(&[]).effective_keep_transcript_clipboard(&config));

        // (a) Positive flag overrides a config `false`.
        config.daemon.keep_transcript_clipboard = Some(false);
        assert!(
            cli_from(&["--keep-transcript-clipboard"]).effective_keep_transcript_clipboard(&config)
        );

        // (b) Negative flag overrides a config `true`.
        config.daemon.keep_transcript_clipboard = Some(true);
        assert!(!cli_from(&["--no-keep-transcript-clipboard"])
            .effective_keep_transcript_clipboard(&config));

        // (d) The pair conflicts.
        let error = Cli::try_parse_from([
            "parakit",
            "--keep-transcript-clipboard",
            "--no-keep-transcript-clipboard",
        ])
        .expect_err("--keep-transcript-clipboard and --no-keep-transcript-clipboard must conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn effective_verbose_uses_or_semantics() {
        let mut config = ConfigFile::default();
        assert!(!cli_from(&[]).effective_verbose(&config));

        config.daemon.verbose = Some(true);
        assert!(cli_from(&[]).effective_verbose(&config));

        config.daemon.verbose = Some(false);
        assert!(cli_from(&["--verbose"]).effective_verbose(&config));
    }

    #[test]
    fn effective_verbose_config_true_does_not_override_cli_quiet() {
        // `--quiet` and `--verbose` conflict at the CLI level, but a config
        // `verbose = true` cannot participate in that check. Effective
        // verbosity must still keep every logging layer quiet.
        let mut config = ConfigFile::default();
        config.daemon.verbose = Some(true);
        let cli = cli_from(&["--quiet"]);
        assert!(cli.quiet);
        assert!(!cli.effective_verbose(&config));
    }

    #[test]
    fn effective_disabled_rules_merges_cli_and_config_without_duplicates() {
        let mut config = ConfigFile::default();
        config.cleaning.disabled_rules = vec![
            "fix-trailing-period".to_string(),
            "filled-pauses".to_string(),
        ];

        let cli = cli_from(&[
            "--disable-rule",
            "filled-pauses",
            "--disable-rule",
            "lead-discourse-comma",
        ]);
        let mut merged = cli.effective_disabled_rules(&config);
        merged.sort();
        let mut expected = vec![
            "filled-pauses".to_string(),
            "fix-trailing-period".to_string(),
            "lead-discourse-comma".to_string(),
        ];
        expected.sort();
        assert_eq!(merged, expected);
    }

    #[test]
    fn effective_transcript_history_prefers_config_then_default() {
        let mut config = ConfigFile::default();
        assert_eq!(
            cli_from(&[]).effective_transcript_history(&config),
            DEFAULT_TRANSCRIPT_HISTORY
        );

        config.daemon.transcript_history = Some(0);
        assert_eq!(cli_from(&[]).effective_transcript_history(&config), 0);

        config.daemon.transcript_history = Some(25);
        assert_eq!(cli_from(&[]).effective_transcript_history(&config), 25);
    }
}
