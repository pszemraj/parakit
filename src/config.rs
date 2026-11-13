//! User config file (`config.toml`).
//!
//! parakit reads an optional TOML config file for daemon defaults, cleaning
//! preferences, transcription logging, the Linux hotkey backend, and
//! user-defined cleaning rules. Every key is optional; a missing file or a
//! missing key falls back to built-in defaults.
//!
//! Precedence is CLI flags > config file > built-in defaults. Section methods
//! resolve config/default values; the CLI's `effective_*` methods add overrides.
//!
//! A broken config file must never break commands that do not need it
//! (`fetch`, `cache`, `status`, `stop`, `copy-last`, `history`, `test-paste`);
//! see the load-order comment in `app.rs::run`. Within
//! `config`, `path`/`init`/`edit` stay available on a broken file, but
//! `show` (the default subcommand) loads and validates the file and so
//! fails with a diagnostic naming the config path.

use anyhow::{Context, Result};
use parakit::inference::DeviceMode;
use parakit::model::{ModelPreset, ModelQuantization};
use parakit::qwen::QwenOptions;
use parakit::rules::{CleaningProfile, UserRule};
use serde::Deserialize;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use crate::daemon::hotkey::HotkeyBackend;
use crate::daemon::inject::PasteMode;

/// Environment variable that overrides the config file path.
pub(crate) const CONFIG_PATH_ENV: &str = "PARAKIT_CONFIG_PATH";

/// Parsed `config.toml` contents.
///
/// Every field is optional at every level: an absent file, an absent
/// section, or an absent key all fall back to built-in defaults. Unknown
/// keys are rejected so misspellings cannot silently change behavior.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConfigFile {
    pub(crate) daemon: DaemonConfig,
    pub(crate) cleaning: CleaningConfig,
    pub(crate) logging: LoggingConfig,
    pub(crate) hotkey: HotkeyConfig,
    pub(crate) rules: RulesConfig,
    pub(crate) qwen: QwenConfig,
}

/// `[daemon]` section: daemon runtime defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DaemonConfig {
    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    pub(crate) model: Option<PathBuf>,
    /// Named model preset, mutually exclusive with `model`.
    pub(crate) model_preset: Option<ModelPreset>,
    /// Preset precision; startup acquires Q8_0 or generates cached Q6_K.
    pub(crate) quantize: Option<ModelQuantization>,
    /// Runtime compute device.
    pub(crate) device: Option<DeviceMode>,
    /// CPU inference threads.
    pub(crate) threads: Option<NonZeroUsize>,
    /// Release the session after this many idle minutes; zero disables offload.
    pub(crate) model_idle_minutes: Option<u64>,
    /// Batch insertion style.
    pub(crate) paste_mode: Option<PasteMode>,
    /// Linux insertion transport, independent of terminal content policy.
    #[cfg(target_os = "linux")]
    pub(crate) insertion_backend: Option<crate::daemon::desktop::linux_insertion::InsertionBackend>,
    /// Accepted for portable dotfiles and ignored on other platforms.
    #[cfg(not(target_os = "linux"))]
    #[serde(rename = "insertion_backend")]
    pub(crate) _insertion_backend: Option<String>,
    /// Leave dictated text on the clipboard after paste instead of
    /// restoring previous clipboard contents.
    pub(crate) keep_transcript_clipboard: Option<bool>,
    /// Play the audio cues (start / success / error tones).
    pub(crate) sounds: Option<bool>,
    /// Verbose diagnostics: paths, backend details, and timing lines.
    pub(crate) verbose: Option<bool>,
    /// Count of transcripts retained for `copy-last`, `history`, and Qwen context.
    /// Zero disables retention. History is in memory only and cleared on stop.
    pub(crate) transcript_history: Option<usize>,
}

/// `[qwen]` settings apply only to Qwen3-ASR sessions.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct QwenConfig {
    /// Source language code/name; unset or `auto` detects the language.
    pub(crate) language: Option<String>,
    /// Generated-token budget per native chunk, 1 through 4096.
    pub(crate) max_new_tokens: Option<usize>,
    /// Use recent dictations for the original push-to-talk shortcut.
    pub(crate) context_on_default_hotkey: Option<bool>,
    /// Maximum prior dictations used as context; zero disables context.
    pub(crate) context_dictations: Option<usize>,
    /// Positive maximum age of prior context, in minutes.
    pub(crate) context_max_age_minutes: Option<u64>,
}

impl QwenConfig {
    /// Resolve configured controls over the shared Qwen defaults.
    ///
    /// # Returns
    /// The configured Qwen options, including effective defaults.
    pub(crate) fn options(&self) -> QwenOptions {
        let mut options = QwenOptions::default();
        if let Some(language) = &self.language {
            options.language = Some(language.clone());
        }
        if let Some(max_new_tokens) = self.max_new_tokens {
            options.max_new_tokens = max_new_tokens;
        }
        options
    }

    /// # Returns
    /// Whether the original shortcut requests context; disabled by default.
    pub(crate) fn context_on_default_hotkey(&self) -> bool {
        self.context_on_default_hotkey.unwrap_or(false)
    }

    /// # Returns
    /// Maximum prior dictations to use, defaulting to three.
    pub(crate) fn context_dictations(&self) -> usize {
        self.context_dictations.unwrap_or(3)
    }

    /// # Returns
    /// Whether retained history can supply prior-dictation context.
    pub(crate) fn context_enabled(&self, history_limit: usize) -> bool {
        self.context_dictations() != 0 && history_limit != 0
    }

    /// # Returns
    /// Whether the extra context shortcut adds a mode: context is available and
    /// the original shortcut does not already request it.
    pub(crate) fn context_shortcut_enabled(&self, history_limit: usize) -> bool {
        self.context_enabled(history_limit) && !self.context_on_default_hotkey()
    }

    /// # Returns
    /// Maximum context age in minutes, defaulting to ten.
    pub(crate) fn context_max_age_minutes(&self) -> u64 {
        self.context_max_age_minutes.unwrap_or(10)
    }
}

impl DaemonConfig {
    /// Resolve Linux native insertion preference, defaulting to automatic selection.
    ///
    /// # Returns
    /// The configured preference or automatic selection.
    #[cfg(target_os = "linux")]
    pub(crate) fn effective_insertion_backend(
        &self,
    ) -> crate::daemon::desktop::linux_insertion::InsertionBackend {
        self.insertion_backend.unwrap_or_default()
    }
    /// # Returns
    /// The configured compute device, or automatic selection.
    pub(crate) fn effective_device(&self) -> DeviceMode {
        self.device.unwrap_or_default()
    }

    /// # Returns
    /// The configured paste mode, or the platform default.
    pub(crate) fn effective_paste_mode(&self) -> PasteMode {
        self.paste_mode.unwrap_or_else(default_paste_mode)
    }

    /// # Returns
    /// Whether transcripts remain on the clipboard; disabled by default.
    pub(crate) fn keep_transcript_clipboard(&self) -> bool {
        self.keep_transcript_clipboard.unwrap_or(false)
    }

    /// # Returns
    /// Whether audio cues are enabled; enabled by default.
    pub(crate) fn sounds_enabled(&self) -> bool {
        self.sounds.unwrap_or(true)
    }

    /// # Returns
    /// Whether verbose diagnostics are enabled; disabled by default.
    pub(crate) fn verbose(&self) -> bool {
        self.verbose.unwrap_or(false)
    }

    /// # Returns
    /// The configured idle timeout, including zero to disable offload.
    pub(crate) fn effective_model_idle_minutes(&self) -> u64 {
        self.model_idle_minutes
            .unwrap_or(crate::daemon::model_lifecycle::DEFAULT_MODEL_IDLE_MINUTES)
    }

    /// # Returns
    /// The configured transcript history depth; zero disables history.
    pub(crate) fn effective_transcript_history(&self) -> usize {
        self.transcript_history
            .unwrap_or(crate::daemon::ipc::DEFAULT_TRANSCRIPT_HISTORY)
    }
}

/// # Returns
/// Terminal paste on Linux, standard paste on other platforms.
pub(crate) fn default_paste_mode() -> PasteMode {
    if cfg!(target_os = "linux") {
        PasteMode::Terminal
    } else {
        PasteMode::Standard
    }
}

/// `[cleaning]` section: text-cleaning pipeline defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CleaningConfig {
    /// Enable the text-cleaning pipeline.
    pub(crate) enabled: Option<bool>,
    /// Cleanup behavior tier: `safe` (default) or `aggressive`. Overridden by
    /// CLI `--cleaning-profile`.
    pub(crate) profile: Option<CleaningProfile>,
    /// Keep the single terminal period that cleanup removes by default.
    /// CLI `--keep-trailing-period` forces this on, and `--no-keep-trailing-period` forces it off.
    pub(crate) keep_trailing_period: Option<bool>,
    /// Conversion threshold shared by the numeric rules.
    /// Every spoken cardinal and ordinal is compared with it, including larger values and lists: below it they retain the ASR wording, and at or above it they become digits.
    /// A run of adjacent spoken counts containing any value below the threshold retains its complete wording, including a final size suffix. Otherwise its counts render separately.
    /// Unset resolves to the built-in default of 4, so zero through three and first through third stay as produced.
    /// `0` converts every eligible value, while plural magnitudes, fractions, bare scales, and the time unit "second" retain their wording.
    pub(crate) number_threshold: Option<f64>,
    /// Additional literal words or phrases protected from whole-word repetition collapse.
    pub(crate) repetition_exceptions: Vec<String>,
    /// Rule names to disable. Merged with CLI `--disable-rule` flags.
    pub(crate) disabled_rules: Vec<String>,
}

impl CleaningConfig {
    /// # Returns
    /// Whether transcript cleaning is enabled; enabled by default.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// # Returns
    /// The configured cleanup tier, or the safe profile.
    pub(crate) fn effective_profile(&self) -> CleaningProfile {
        self.profile.unwrap_or(CleaningProfile::Safe)
    }

    /// # Returns
    /// Whether to keep the terminal period; dropped by default.
    pub(crate) fn keep_trailing_period(&self) -> bool {
        self.keep_trailing_period.unwrap_or(false)
    }

    /// Merge explicit rule overrides with configured disabled rules.
    ///
    /// # Arguments
    ///
    /// * `overrides` - Explicit rule names, retained in their original order.
    ///
    /// # Returns
    ///
    /// The overrides followed by configured names not already present.
    pub(crate) fn merged_disabled_rules(&self, overrides: &[String]) -> Vec<String> {
        let mut merged = overrides.to_vec();
        for name in &self.disabled_rules {
            if !merged.contains(name) {
                merged.push(name.clone());
            }
        }
        merged
    }
}

/// `[logging]` section: transcription log defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LoggingConfig {
    /// Directory for JSONL transcription logs. One file is written per local day.
    pub(crate) dir: Option<PathBuf>,
}

/// `[hotkey]` section: Linux hotkey backend default.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct HotkeyConfig {
    /// Linux hotkey backend.
    #[cfg(target_os = "linux")]
    pub(crate) backend: Option<HotkeyBackend>,
    /// Accepted for dotfiles portability but ignored off Linux.
    #[cfg(not(target_os = "linux"))]
    #[serde(rename = "backend")]
    pub(crate) _backend: Option<String>,
}

#[cfg(target_os = "linux")]
impl HotkeyConfig {
    /// # Returns
    /// The configured hotkey backend, or automatic selection.
    pub(crate) fn effective_backend(&self) -> HotkeyBackend {
        self.backend.unwrap_or(HotkeyBackend::Auto)
    }
}

/// `[rules]` section: user-defined cleaning rules.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RulesConfig {
    /// User-defined rules, applied alongside the built-in rules. See
    /// `parakit::rules` for the compiled application order.
    pub(crate) user: Vec<UserRule>,
}

/// Commented template covering every config key. Not parsed at startup;
/// written to disk by `parakit config init`.
pub(crate) const TEMPLATE: &str = r#"# parakit config.toml
#
# Precedence: CLI flags > this file > built-in defaults.
# Every key below is optional.
# Uncomment and edit the keys you want to override, and leave the rest commented out to keep the built-in default.

[daemon]
# Path to a GGUF model file.
# Overrides the cached Q8_0 model.
# model = "/path/to/model.gguf"

# Model preset, mutually exclusive with model. Precision defaults to Q8_0.
# Choices: parakeet-v2, parakeet-v3, qwen3-asr-0.6b, qwen3-asr-1.7b, parakeet-ultra, parakeet-redux.
# Unset: parakeet-ultra. Qwen presets are experimental; Redux is for testing only.
# model_preset = "qwen3-asr-1.7b"

# Preset precision: q8_0 (default) or locally generated q6_k.
# Missing models are prepared automatically at startup. Conflicts with model.
# quantize = "q6_k"

# Runtime compute device: "auto", "cpu", or "gpu".
# device = "auto"

# CPU inference threads, 1 to 2147483647.
# Unset: half the logical CPUs the process may use, with a minimum of 2, or 1 on a single-CPU machine.
# threads = 4

# Unload the model after this many idle minutes.
# 0 keeps it resident.
# The next dictation automatically reloads it while audio is captured.
# model_idle_minutes = 10

# Batch insertion style: "terminal", "standard", or "direct".
# Unset: "terminal" on Linux, "standard" on macOS and Windows.
# paste_mode = "standard"

# Linux insertion transport: "auto", "ibus", or "x11".
# insertion_backend = "auto"

# Leave dictated text on the clipboard after paste instead of restoring the previous clipboard contents.
# keep_transcript_clipboard = false

# Play the audio cues (start / success / error tones).
# sounds = true

# Print diagnostics: model path, microphone and backend details, timings, clipboard diagnostics, and the native inference library's own log messages.
# Applies to `parakit start`, `parakit doctor`, and `parakit rules`.
# Other commands take only the --verbose flag.
# The global --verbose flag turns diagnostics on whatever this key says, and the global --quiet flag always wins.
# verbose = false

# Number of transcripts kept in daemon memory for `copy-last` and `history`.
# Valid range: 0 through 100.
# 0 disables both.
# History lives only in daemon memory and is cleared when the daemon stops.
# This key never writes it to disk, though logging.dir can log the same transcripts.
# transcript_history = 10

[qwen]
# Source language code or English name; auto detects the supported language.
# Applies only to Qwen3-ASR. CLI --qwen-language overrides it.
# language = "auto"

# Maximum generated tokens per native chunk, 1 through 4096.
# Applies only to Qwen3-ASR. CLI --qwen-max-new-tokens overrides it.
# max_new_tokens = 512

# Use recent dictations with the original Ctrl+Space shortcut; the extra context shortcut is then inactive.
# context_on_default_hotkey = false

# Prior dictations to use as context, limited by daemon.transcript_history; 0 disables context.
# context_dictations = 3

# Maximum age of prior context in minutes; must be positive.
# context_max_age_minutes = 10

[cleaning]
# Enable the text-cleaning pipeline.
# Set false for raw multilingual dictation without English or user cleaning rules.
# enabled = true

# Cleanup behavior tier: "safe" or "aggressive".
# Safe performs mechanical cleanup and high-confidence normalization only.
# Aggressive additionally deletes discourse markers such as filler "like", "you know", and "I mean", which can change meaning, and collapses repeats of that, no, can, had, and do.
# Overridden by CLI --cleaning-profile.
# profile = "safe"

# Keep the single terminal period.
# Cleanup drops it by default because parakit is used mostly for messaging-style dictation.
# CLI --keep-trailing-period forces this on, and --no-keep-trailing-period forces it off.
# keep_trailing_period = false

# Conversion threshold shared by the numeric rules, 0 or more.
# Every spoken cardinal and ordinal is compared with it, including values of 10 or more and values in lists.
# One below the threshold is left exactly as the ASR model produced it, never forced to words or digits, and one at or above it becomes digits.
# A run of adjacent spoken counts containing any value below the threshold retains its whole wording, including a final size suffix.
# Otherwise the counts render separately with their spaces retained: "fifty ninety" becomes "50 90".
# Leave unset to use the built-in default of 4, which leaves zero through three and first through third as produced.
# For example, 100 also leaves fifty as a word.
# Set to 0 to convert every eligible value, including each separate count in a mixed run.
# Even at 0, plural magnitudes, fractions, bare scales (including "a million"), and the time unit "second" retain their wording.
# Year/version structures have separate rules. Identifier compaction grants no threshold bypass for spoken counts and also works on existing digits.
# number_threshold = 5

# Additional literal words or phrases to preserve in the repeated-word pass.
# Matching ignores case; other cleaning rules still apply. No CLI flag.
# repetition_exceptions = ["something", "tomato", "paddle", "chinny chin chin"]

# Rule names to disable.
# Merged with any CLI --disable-rule flags.
# Run `parakit rules list` to see all built-in and user rule names.
# disabled_rules = ["fix-trailing-period"]

[logging]
# Directory for JSONL transcription logs.
# One file is written per local day.
# Unset disables transcription logging.
# dir = "/home/user/.parakit/logs"

[hotkey]
# Linux hotkey backend: "auto", "desktop", "ibus", "x11-global-hotkey", "x11-listen", or "evdev-proxy-experimental".
# Linux only.
# backend = "auto"

# User-defined text-cleaning rules, applied alongside the built-in rules.
# Repeat the [[rules.user]] table for each additional rule.
# Uncomment the [[rules.user]] header itself, or the keys below land in [hotkey].
# Names must not collide with a built-in rule or another user rule, and `pattern` must be a valid Rust `regex` crate pattern.
# See docs/config_reference.toml for every user-rule key, its validation, and the replacement syntax.
# [[rules.user]]
# name = "weights-and-biases-to-wandb"
# description = "Map 'weights and biases' to 'wandb'"
# pattern = "(?i)\\bweights and biases\\b"
# replacement = "wandb"
# position = "standard"  # "first", "standard" (default), or "last"
"#;

/// Return the resolved config file path.
///
/// # Returns
///
/// `$PARAKIT_CONFIG_PATH` when set, otherwise the platform config
/// directory joined with `parakit/config.toml`.
///
/// # Errors
///
/// Returns an error if `$PARAKIT_CONFIG_PATH` is set but empty, or if the
/// operating system does not expose a usable config or home directory.
pub(crate) fn config_path() -> Result<PathBuf> {
    config_path_with_override(std::env::var_os(CONFIG_PATH_ENV))
}

/// Resolve the config path from an already-read environment override.
///
/// Keeping the environment read at the public boundary lets tests exercise
/// override precedence without mutating process-global environment state.
fn config_path_with_override(override_path: Option<std::ffi::OsString>) -> Result<PathBuf> {
    if let Some(path) = parakit::model::non_empty_env_path(CONFIG_PATH_ENV, override_path)? {
        return Ok(path);
    }

    #[cfg(target_os = "windows")]
    {
        let dirs =
            directories::BaseDirs::new().context("could not determine user config directory")?;
        Ok(dirs.config_dir().join("parakit").join("config.toml"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(xdg_config_base()?.join("parakit").join("config.toml"))
    }
}

/// Return the XDG-style config base used by Unix-like parakit paths.
///
/// # Returns
///
/// `$XDG_CONFIG_HOME` when set and non-empty, otherwise `$HOME/.config`.
///
/// # Errors
///
/// Returns an error if no usable home directory is available.
#[cfg(not(target_os = "windows"))]
fn xdg_config_base() -> Result<PathBuf> {
    parakit::model::xdg_base("XDG_CONFIG_HOME", ".config")
}

/// Load the config file at the resolved [`config_path`].
///
/// # Returns
///
/// The parsed config, or built-in defaults when no file exists at the default location.
///
/// # Errors
///
/// Returns an error for an unresolved path, a missing explicit override,
/// unreadable or invalid TOML, or unsupported daemon, Qwen, or cleaning settings.
/// Parse and validation errors are annotated with the config file path.
pub(crate) fn load() -> Result<ConfigFile> {
    load_with_override(std::env::var_os(CONFIG_PATH_ENV))
}

/// Load the config from an already-read `$PARAKIT_CONFIG_PATH` value.
///
/// A missing file at the default location means "use the defaults", but an explicit override names a file the user expects to be read, so a missing one is almost always a typo.
fn load_with_override(override_path: Option<std::ffi::OsString>) -> Result<ConfigFile> {
    let explicit = override_path.is_some();
    let path = config_path_with_override(override_path)?;
    // A failed existence probe falls through, so the read reports the real error.
    if explicit && matches!(path.try_exists(), Ok(false)) {
        anyhow::bail!(
            "{CONFIG_PATH_ENV} names {}, which does not exist; create it with `parakit config init`, or unset {CONFIG_PATH_ENV} to use the default config location",
            path.display()
        );
    }
    load_from_path(&path)
}

/// Load and validate a config file at an explicit path. Split out from
/// [`load`] so tests can exercise parsing without touching process
/// environment state.
///
/// # Returns
///
/// The parsed and validated config, or [`ConfigFile::default`] when no
/// file exists at `path`.
///
/// # Errors
///
/// See [`load`].
pub(crate) fn load_from_path(path: &Path) -> Result<ConfigFile> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigFile::default());
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read config file {}", path.display()));
        }
    };

    let config: ConfigFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;

    if config
        .daemon
        .transcript_history
        .is_some_and(|limit| limit > crate::daemon::ipc::MAX_TRANSCRIPT_HISTORY)
    {
        anyhow::bail!(
            "invalid config in {}: daemon.transcript_history must be between 0 and {}",
            path.display(),
            crate::daemon::ipc::MAX_TRANSCRIPT_HISTORY
        );
    }

    parakit::model::resolve_model_selection(
        None,
        None,
        config.daemon.model.as_deref(),
        config.daemon.model_preset,
    )
    .with_context(|| format!("invalid config in {}", path.display()))?;
    if config.daemon.model.is_some() && config.daemon.quantize.is_some() {
        anyhow::bail!(
            "invalid config in {}: daemon.model and daemon.quantize cannot be set together",
            path.display()
        );
    }
    config
        .qwen
        .options()
        .validate()
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid config in {}", path.display()))?;
    let context_age = config.qwen.context_max_age_minutes();
    if context_age == 0 || context_age.checked_mul(60).is_none() {
        anyhow::bail!(
            "invalid config in {}: qwen.context_max_age_minutes must be positive and fit in seconds",
            path.display()
        );
    }

    // Checked here so the daemon never fetches or opens a model it would then refuse.
    if config
        .daemon
        .threads
        .is_some_and(|threads| threads.get() > parakit::inference::MAX_THREADS)
    {
        anyhow::bail!(
            "invalid config in {}: daemon.threads must be between 1 and {}",
            path.display(),
            parakit::inference::MAX_THREADS
        );
    }

    // An empty path is never intended: as logging.dir it writes logs into the daemon's working directory, and as daemon.model it fails only at startup with a blank path in the error.
    for (key, value) in [
        ("daemon.model", config.daemon.model.as_deref()),
        ("logging.dir", config.logging.dir.as_deref()),
    ] {
        if value.is_some_and(|value| value.as_os_str().is_empty()) {
            anyhow::bail!(
                "invalid config in {}: {key} is set but empty",
                path.display()
            );
        }
    }

    parakit::rules::validate_configured_rules(
        config.cleaning.number_threshold,
        &config.cleaning.repetition_exceptions,
        &config.cleaning.disabled_rules,
        &config.rules.user,
    )
    .with_context(|| format!("invalid config in {}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "config_reference_tests.rs"]
mod reference_tests;
