//! User config file (`config.toml`).
//!
//! parakit reads an optional TOML config file for daemon defaults, cleaning
//! preferences, transcription logging, the Linux hotkey backend, and
//! user-defined cleaning rules. Every key is optional; a missing file or a
//! missing key falls back to built-in defaults.
//!
//! Precedence is CLI flags > config file > built-in defaults. See
//! `Cli::effective_*` in `cli.rs` for the merge logic.
//!
//! A broken config file must never break commands that do not need it
//! (`fetch`, `cache`, `status`, `stop`, `copy-last`, `history`, `test-paste`);
//! see the load-order comment in `app.rs::run`. Within
//! `config`, `path`/`init`/`edit` stay available on a broken file, but
//! `show` (the default subcommand) loads and validates the file and so
//! fails with a diagnostic naming the config path.

use anyhow::{Context, Result};
use parakit::inference::DeviceMode;
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
}

/// `[daemon]` section: daemon runtime defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DaemonConfig {
    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    pub(crate) model: Option<PathBuf>,
    /// Runtime compute device.
    pub(crate) device: Option<DeviceMode>,
    /// CPU inference threads.
    pub(crate) threads: Option<NonZeroUsize>,
    /// Batch insertion style.
    pub(crate) paste_mode: Option<PasteMode>,
    /// Leave dictated text on the clipboard after paste instead of
    /// restoring previous clipboard contents.
    pub(crate) keep_transcript_clipboard: Option<bool>,
    /// Play the audio cues (start / success / error tones).
    pub(crate) sounds: Option<bool>,
    /// Verbose diagnostics: paths, backend details, and timing lines.
    pub(crate) verbose: Option<bool>,
    /// Count of transcripts kept in daemon memory for `copy-last` and
    /// `history`. `0` disables both. History lives only in daemon memory;
    /// nothing is written to disk, and it is cleared when the daemon stops.
    pub(crate) transcript_history: Option<usize>,
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
    /// CLI `--keep-trailing-period` forces this on.
    pub(crate) keep_trailing_period: Option<bool>,
    /// Minimum isolated numeric value converted to digits. Unset resolves
    /// to the built-in default of 4: isolated values below 4 are left
    /// exactly as the ASR model produced them. `0` is the explicit opt-out
    /// that converts every recognized number.
    pub(crate) number_threshold: Option<f64>,
    /// Rule names to disable. Merged with CLI `--disable-rule` flags.
    pub(crate) disabled_rules: Vec<String>,
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
# Every key below is optional. Uncomment and edit the keys you want to
# override; leave the rest commented out to keep the built-in default.

[daemon]
# Path to a GGUF model file. Overrides the cached Q8_0 model.
# model = "/path/to/model.gguf"

# Runtime compute device: "auto", "cpu", or "gpu".
# device = "auto"

# CPU inference threads. Defaults to a conservative detected count.
# threads = 4

# Batch insertion style: "terminal", "standard", or "direct".
# paste_mode = "standard"

# Leave dictated text on the clipboard after paste instead of restoring
# the previous clipboard contents.
# keep_transcript_clipboard = false

# Play the audio cues (start / success / error tones).
# sounds = true

# Verbose diagnostics: paths, backend details, and timing lines. A CLI
# --quiet flag always wins over this setting.
# verbose = false

# Number of transcripts kept in daemon memory for `copy-last` and `history`.
# 0 disables both. History lives only in daemon memory, is never written to
# disk, and is cleared when the daemon stops.
# transcript_history = 10

[cleaning]
# Enable the text-cleaning pipeline.
# enabled = true

# Cleanup behavior tier: "safe" or "aggressive". Safe performs mechanical
# cleanup and high-confidence normalization only. Aggressive additionally
# deletes discourse markers such as filler "like", "you know", and "I mean",
# which can change meaning. Overridden by CLI --cleaning-profile.
# profile = "safe"

# Keep the single terminal period. Cleanup drops it by default because
# parakit is used mostly for messaging-style dictation. CLI
# --keep-trailing-period forces this on.
# keep_trailing_period = false

# Minimum isolated numeric value rendered as digits. Leave unset to use the
# built-in default of 4: isolated values below the threshold are left
# exactly as the ASR model produced them, never forced to words or digits.
# Set to 0 to instead convert every recognized number. For example, 5 keeps
# isolated zero through four as-is and converts five and larger values.
# number_threshold = 5

# Rule names to disable. Merged with any CLI --disable-rule flags. Run
# `parakit rules list` to see all built-in and user rule names.
# disabled_rules = ["fix-trailing-period"]

[logging]
# Directory for JSONL transcription logs. One file is written per local day.
# Unset disables transcription logging.
# dir = "/home/user/.parakit/logs"

[hotkey]
# Linux hotkey backend: "auto", "desktop", "x11-global-hotkey",
# "x11-listen", or "evdev-proxy-experimental". Linux only.
# backend = "auto"

# User-defined text-cleaning rules, applied alongside the built-in rules.
# Repeat the [[rules.user]] table for each additional rule; uncomment the
# [[rules.user]] header itself, or the keys below land in [hotkey]. See
# docs/cleaning-rules.md for the full user-rule format and validation
# rules (names must not collide with a built-in rule or another user rule,
# and `pattern` must be a valid Rust `regex` crate pattern).
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
    if let Some(raw) = override_path {
        if raw.is_empty() {
            anyhow::bail!("{CONFIG_PATH_ENV} is set but empty");
        }
        return Ok(PathBuf::from(raw));
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
/// The parsed config, or built-in defaults when the file does not exist.
///
/// # Errors
///
/// Returns an error if the config path cannot be resolved, the file exists
/// but cannot be read, the file is not valid TOML for [`ConfigFile`], a
/// user-defined rule fails validation (empty or non-canonical name, empty
/// pattern, invalid regex, a name colliding with a built-in rule, or a
/// duplicate user rule name), or `cleaning.disabled_rules` names a rule that
/// does not exist.
/// Parse and validation errors are annotated with the config file path.
pub(crate) fn load() -> Result<ConfigFile> {
    load_from_path(&config_path()?)
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

    parakit::rules::validate_configured_rules(
        config.cleaning.number_threshold,
        &config.cleaning.disabled_rules,
        &config.rules.user,
    )
    .with_context(|| format!("invalid config in {}", path.display()))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(name: &str, contents: &str) -> PathBuf {
        let dir = crate::test_support::fixture_root("parakit-config-test", name);
        let path = dir.join("config.toml");
        let mut file = std::fs::File::create(&path).expect("create fixture config file");
        file.write_all(contents.as_bytes())
            .expect("write fixture config file");
        path
    }

    #[test]
    fn config_defaults_from_missing_empty_or_template_file() {
        // Every row asserts the same superset of default fields: the union
        // of what missing_file_returns_defaults, empty_file_parses_as_defaults,
        // and template_parses_as_valid_toml_with_all_defaults each checked
        // individually before this table replaced them. Deliberate
        // uniform-coverage increase.
        //
        // (label, fixture directory slug, file contents; `None` means the
        // file is absent entirely — never written.)
        let cases: [(&str, &str, Option<&str>); 3] = [
            ("missing file", "missing", None),
            ("empty file", "empty", Some("")),
            (
                "template (entirely commented out, must still parse as valid TOML)",
                "template",
                Some(TEMPLATE),
            ),
        ];

        for (label, slug, contents) in cases {
            let path = match contents {
                None => {
                    let dir = crate::test_support::fixture_root("parakit-config-test", slug);
                    dir.join("does-not-exist.toml")
                }
                Some(contents) => write_fixture(slug, contents),
            };
            let config = load_from_path(&path)
                .unwrap_or_else(|e| panic!("{label}: config file should parse: {e:#}"));

            assert!(config.daemon.model.is_none(), "{label}: daemon.model");
            assert!(config.daemon.device.is_none(), "{label}: daemon.device");
            assert!(config.rules.user.is_empty(), "{label}: rules.user");
            assert!(
                config.cleaning.disabled_rules.is_empty(),
                "{label}: cleaning.disabled_rules"
            );
            assert!(
                config.cleaning.enabled.is_none(),
                "{label}: cleaning.enabled"
            );
            assert!(
                config.cleaning.number_threshold.is_none(),
                "{label}: cleaning.number_threshold"
            );
        }
    }

    #[test]
    fn fully_populated_config_parses_every_field() {
        let toml = r#"
[daemon]
model = "/models/custom.gguf"
device = "gpu"
threads = 4
paste_mode = "direct"
keep_transcript_clipboard = true
sounds = false
verbose = true
transcript_history = 25

[cleaning]
enabled = false
profile = "aggressive"
keep_trailing_period = true
number_threshold = 5
disabled_rules = ["fix-trailing-period", "filled-pauses"]

[logging]
dir = "/logs"

[[rules.user]]
name = "custom-hello"
description = "greet"
pattern = "(?i)hi"
replacement = "hello"
position = "first"
"#;
        let path = write_fixture("full", toml);
        let config = load_from_path(&path).expect("fully populated config should parse");

        assert_eq!(
            config.daemon.model,
            Some(PathBuf::from("/models/custom.gguf"))
        );
        assert_eq!(config.daemon.device, Some(DeviceMode::Gpu));
        assert_eq!(config.daemon.threads, NonZeroUsize::new(4));
        assert_eq!(config.daemon.paste_mode, Some(PasteMode::Direct));
        assert_eq!(config.daemon.keep_transcript_clipboard, Some(true));
        assert_eq!(config.daemon.sounds, Some(false));
        assert_eq!(config.daemon.verbose, Some(true));
        assert_eq!(config.daemon.transcript_history, Some(25));

        assert_eq!(config.cleaning.enabled, Some(false));
        assert_eq!(config.cleaning.profile, Some(CleaningProfile::Aggressive));
        assert_eq!(config.cleaning.keep_trailing_period, Some(true));
        assert_eq!(config.cleaning.number_threshold, Some(5.0));
        assert_eq!(
            config.cleaning.disabled_rules,
            vec![
                "fix-trailing-period".to_string(),
                "filled-pauses".to_string()
            ]
        );

        assert_eq!(config.logging.dir, Some(PathBuf::from("/logs")));

        assert_eq!(config.rules.user.len(), 1);
        assert_eq!(config.rules.user[0].name, "custom-hello");
        assert_eq!(config.rules.user[0].description.as_deref(), Some("greet"));
        assert_eq!(config.rules.user[0].pattern, "(?i)hi");
        assert_eq!(config.rules.user[0].replacement, "hello");
        assert_eq!(
            config.rules.user[0].position,
            parakit::rules::RulePosition::First
        );
    }

    #[test]
    fn parse_error_includes_file_path() {
        let path = write_fixture("bad-toml", "not = [valid");
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
    }

    #[test]
    fn misspelled_user_rule_position_is_rejected() {
        let toml = r#"
[[rules.user]]
name = "custom-hello"
pattern = "(?i)hi"
replacement = "hello"
positon = "last"
"#;
        let path = write_fixture("misspelled-user-rule-position", toml);
        let err = load_from_path(&path).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("unknown field `positon`"), "{message}");
        assert!(message.contains(&path.display().to_string()), "{message}");
    }

    #[test]
    fn validation_error_includes_rule_failure_and_config_path() {
        let toml = r#"
[[rules.user]]
name = "bad"
pattern = "(unclosed"
replacement = "x"
"#;
        let path = write_fixture("bad-regex", toml);
        let err = load_from_path(&path).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("invalid regex"), "message: {message}");
        assert!(
            message.contains(&path.display().to_string()),
            "config path missing from {message}"
        );
    }

    #[test]
    fn disabled_rules_naming_a_user_rule_is_accepted_at_load() {
        let toml = r#"
[cleaning]
disabled_rules = ["custom-hello"]

[[rules.user]]
name = "custom-hello"
pattern = "(?i)hi"
replacement = "hello"
"#;
        let path = write_fixture("disabled-rules-user-rule", toml);
        let config = load_from_path(&path).expect("disabling a user rule by name should validate");
        assert_eq!(
            config.cleaning.disabled_rules,
            vec!["custom-hello".to_string()]
        );
    }

    #[test]
    fn disabling_a_user_rule_does_not_hide_an_invalid_regex() {
        let toml = r#"
[cleaning]
disabled_rules = ["broken"]

[[rules.user]]
name = "broken"
pattern = "(unclosed"
replacement = "x"
"#;
        let path = write_fixture("disabled-user-rule-bad-regex", toml);
        let err = load_from_path(&path).expect_err("every configured regex must be valid");
        assert!(format!("{err:#}").contains("invalid regex"));
    }

    #[test]
    fn unknown_config_keys_are_rejected() {
        for (name, toml, key) in [
            ("unknown-top-level", "typo = true", "typo"),
            ("unknown-section-key", "[daemon]\npositon = true", "positon"),
        ] {
            let path = write_fixture(name, toml);
            let err = load_from_path(&path).expect_err("unknown key must fail parsing");
            assert!(format!("{err:#}").contains(key));
        }
    }

    #[test]
    fn config_path_env_override_wins() {
        let dir = crate::test_support::fixture_root("parakit-config-test", "env-override");
        let path = dir.join("custom-config.toml");
        let resolved = config_path_with_override(Some(path.clone().into_os_string()));
        assert_eq!(resolved.expect("env override should resolve"), path);
    }

    #[test]
    fn empty_config_path_env_override_is_rejected_without_global_env_mutation() {
        let err = config_path_with_override(Some(std::ffi::OsString::new())).unwrap_err();
        assert!(
            err.to_string().contains("is set but empty"),
            "message: {err:#}"
        );
    }

    /// Assert that every variant of a `clap::ValueEnum` serializes (via
    /// serde) to exactly the string clap accepts on the command line. This
    /// is the invariant `ConfigFile` parsing depends on: a value copied
    /// verbatim from `--flag <value>` into `config.toml` must parse back to
    /// the same enum variant.
    fn assert_serde_matches_clap<T>()
    where
        T: clap::ValueEnum + serde::Serialize + std::fmt::Debug,
    {
        for variant in T::value_variants() {
            let clap_name = variant
                .to_possible_value()
                .expect("value_variants() entries must have a possible value")
                .get_name()
                .to_string();
            let json = serde_json::to_string(variant).expect("serde serialization must succeed");
            assert_eq!(
                json,
                format!("\"{clap_name}\""),
                "variant {variant:?}: serde form must match clap value string"
            );
        }
    }

    #[test]
    fn paste_mode_serde_matches_clap_value_strings() {
        assert_serde_matches_clap::<PasteMode>();
    }

    #[test]
    fn device_mode_serde_matches_clap_value_strings() {
        assert_serde_matches_clap::<DeviceMode>();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hotkey_backend_serde_matches_clap_value_strings() {
        assert_serde_matches_clap::<HotkeyBackend>();
    }
}
