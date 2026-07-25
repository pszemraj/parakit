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
//! (`fetch`, `cache`, `config`, `status`, `stop`, `paste-last`, `copy-last`,
//! `history`, `test-paste`); see the load-order comment in `app.rs::run`.

use anyhow::{Context, Result};
use parakit::data_log::LogFormat;
use parakit::inference::DeviceMode;
use parakit::rules::UserRule;
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
/// keys are ignored (not `deny_unknown_fields`) so older parakit versions
/// tolerate config files written for newer ones.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ConfigFile {
    pub(crate) daemon: DaemonConfig,
    pub(crate) cleaning: CleaningConfig,
    pub(crate) logging: LoggingConfig,
    #[cfg(target_os = "linux")]
    pub(crate) hotkey: HotkeyConfig,
    pub(crate) rules: RulesConfig,
}

/// `[daemon]` section: daemon runtime defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
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
    /// Count of transcripts kept in daemon memory for `paste-last`,
    /// `copy-last`, and `history`. `0` disables all three. History lives
    /// only in daemon memory; nothing is written to disk, and it is
    /// cleared when the daemon stops.
    pub(crate) transcript_history: Option<usize>,
}

/// `[cleaning]` section: text-cleaning pipeline defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CleaningConfig {
    /// Enable the text-cleaning pipeline.
    pub(crate) enabled: Option<bool>,
    /// Rule names to disable. Merged with CLI `--disable-rule` flags.
    pub(crate) disabled_rules: Vec<String>,
}

/// `[logging]` section: transcription log defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LoggingConfig {
    /// Directory for transcription logs. One file is written per local day.
    pub(crate) dir: Option<PathBuf>,
    /// Transcription log format. Used only when `dir` is set.
    pub(crate) format: Option<LogFormat>,
}

/// `[hotkey]` section: Linux hotkey backend default.
#[cfg(target_os = "linux")]
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct HotkeyConfig {
    /// Linux hotkey backend.
    pub(crate) backend: Option<HotkeyBackend>,
}

/// `[rules]` section: user-defined cleaning rules.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
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

# Number of transcripts kept in daemon memory for `paste-last`, `copy-last`,
# and `history`. 0 disables all three. History lives only in daemon memory,
# is never written to disk, and is cleared when the daemon stops.
# transcript_history = 10

[cleaning]
# Enable the text-cleaning pipeline.
# enabled = true

# Rule names to disable. Merged with any CLI --disable-rule flags. Run
# `parakit --list-rules` to see all built-in and user rule names.
# disabled_rules = ["fix-trailing-period"]

[logging]
# Directory for transcription logs. One JSONL or TSV file is written per
# local day. Unset disables transcription logging.
# dir = "/home/user/.parakit/logs"

# Transcription log format: "jsonl" or "tsv". Used only when `dir` is set.
# format = "jsonl"

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
    if let Some(raw) = std::env::var_os(CONFIG_PATH_ENV) {
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
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        if !path.as_os_str().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config"))
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
/// user-defined rule fails validation (empty name, empty pattern, invalid
/// regex, a name colliding with a built-in rule, or a duplicate user rule
/// name), or `cleaning.disabled_rules` names a rule that does not exist.
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

    validate_config(&config).with_context(|| format!("invalid config in {}", path.display()))?;

    Ok(config)
}

/// Validate config-level invariants that are cheap to check eagerly at
/// load time, ahead of daemon bootstrap.
///
/// Reuses [`parakit::rules::build_cleaner`] with the configured
/// `cleaning.disabled_rules` (not an empty slice), so load-time errors —
/// including an unknown name in `disabled_rules` — are worded identically
/// to the errors `--test-rules` or daemon startup would report for the same
/// rule set.
///
/// # Errors
///
/// Returns an error naming the offending user rule for an empty name, an
/// empty pattern, an invalid regex, a name colliding with a built-in rule,
/// or a duplicate user rule name; or naming an unknown rule listed in
/// `cleaning.disabled_rules`.
fn validate_config(config: &ConfigFile) -> Result<()> {
    parakit::rules::build_cleaner(false, &config.cleaning.disabled_rules, &config.rules.user)?;
    Ok(())
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
    fn missing_file_returns_defaults() {
        let dir = crate::test_support::fixture_root("parakit-config-test", "missing");
        let path = dir.join("does-not-exist.toml");
        let config = load_from_path(&path).expect("missing config file should default");
        assert!(config.daemon.model.is_none());
        assert!(config.daemon.device.is_none());
        assert!(config.rules.user.is_empty());
    }

    #[test]
    fn empty_file_parses_as_defaults() {
        let path = write_fixture("empty", "");
        let config = load_from_path(&path).expect("empty config file should parse");
        assert!(config.daemon.device.is_none());
        assert!(config.cleaning.disabled_rules.is_empty());
        assert!(config.cleaning.enabled.is_none());
    }

    #[test]
    fn template_parses_as_valid_toml_with_all_defaults() {
        // The template is entirely commented out, so it must parse to the
        // same defaults as an empty file. This is primarily a check that
        // the template text itself is syntactically valid TOML.
        let path = write_fixture("template", TEMPLATE);
        let config = load_from_path(&path).expect("template should be valid TOML");
        assert!(config.daemon.model.is_none());
        assert!(config.rules.user.is_empty());
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
disabled_rules = ["fix-trailing-period", "filler-um-uh"]

[logging]
dir = "/logs"
format = "tsv"

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
        assert_eq!(
            config.cleaning.disabled_rules,
            vec![
                "fix-trailing-period".to_string(),
                "filler-um-uh".to_string()
            ]
        );

        assert_eq!(config.logging.dir, Some(PathBuf::from("/logs")));
        assert_eq!(config.logging.format, Some(LogFormat::Tsv));

        assert_eq!(config.rules.user.len(), 1);
        assert_eq!(config.rules.user[0].name, "custom-hello");
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
    fn bad_user_rule_regex_error_includes_file_path() {
        let toml = r#"
[[rules.user]]
name = "bad"
pattern = "(unclosed"
replacement = "x"
"#;
        let path = write_fixture("bad-regex", toml);
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("invalid regex"), "message: {msg}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
    }

    #[test]
    fn empty_user_rule_name_is_rejected_at_load() {
        let toml = r#"
[[rules.user]]
name = ""
pattern = "(?i)hi"
replacement = "hello"
"#;
        let path = write_fixture("empty-user-rule-name", toml);
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("empty name"), "message: {msg}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
    }

    #[test]
    fn empty_user_rule_pattern_is_rejected_at_load() {
        let toml = r#"
[[rules.user]]
name = "custom-empty-pattern"
pattern = ""
replacement = "x"
"#;
        let path = write_fixture("empty-user-rule-pattern", toml);
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("custom-empty-pattern"), "message: {msg}");
        assert!(msg.contains("empty pattern"), "message: {msg}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
    }

    #[test]
    fn user_rule_collision_with_builtin_is_rejected_at_load() {
        let toml = r#"
[[rules.user]]
name = "filler-um-uh"
pattern = "(?i)nope"
replacement = "x"
"#;
        let path = write_fixture("collision", toml);
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("filler-um-uh"), "message: {msg}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
    }

    #[test]
    fn disabled_rules_typo_is_rejected_at_load() {
        let toml = r#"
[cleaning]
disabled_rules = ["fixed-trailing-perod"]
"#;
        let path = write_fixture("disabled-rules-typo", toml);
        let err = load_from_path(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no rule named"), "message: {msg}");
        assert!(msg.contains("fixed-trailing-perod"), "message: {msg}");
        assert!(msg.contains(&path.display().to_string()), "message: {msg}");
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
    fn disabling_a_user_rule_with_invalid_regex_does_not_fail_load() {
        // A disabled user rule's pattern is never compiled (see
        // `parakit::rules::push_user_rules`), so an invalid regex on a rule
        // that is also listed in `cleaning.disabled_rules` must not fail
        // config load. This is the load-bearing interaction between the
        // `disabled_rules` name check added to `validate_config` and the
        // documented "park a broken rule by disabling it" workflow.
        let toml = r#"
[cleaning]
disabled_rules = ["broken"]

[[rules.user]]
name = "broken"
pattern = "(unclosed"
replacement = "x"
"#;
        let path = write_fixture("disabled-user-rule-bad-regex", toml);
        let config = load_from_path(&path)
            .expect("disabled user rule with invalid regex must not fail validation");
        assert_eq!(config.rules.user[0].pattern, "(unclosed");
    }

    #[test]
    fn config_path_env_override_wins() {
        let dir = crate::test_support::fixture_root("parakit-config-test", "env-override");
        let path = dir.join("custom-config.toml");
        std::env::set_var(CONFIG_PATH_ENV, &path);
        let resolved = config_path();
        std::env::remove_var(CONFIG_PATH_ENV);
        assert_eq!(resolved.expect("env override should resolve"), path);
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

    #[test]
    fn log_format_serde_matches_cli_flag_strings() {
        // `LogFormat` hand-rolls `FromStr` instead of implementing
        // `clap::ValueEnum` (it also accepts a "json" alias on the CLI that
        // has no serde equivalent), so it is checked directly against the
        // canonical `--log-format` strings rather than through
        // `assert_serde_matches_clap`.
        assert_eq!(
            serde_json::to_string(&LogFormat::Jsonl).unwrap(),
            "\"jsonl\""
        );
        assert_eq!("jsonl".parse::<LogFormat>().unwrap(), LogFormat::Jsonl);
        assert_eq!(serde_json::to_string(&LogFormat::Tsv).unwrap(), "\"tsv\"");
        assert_eq!("tsv".parse::<LogFormat>().unwrap(), LogFormat::Tsv);
    }
}
