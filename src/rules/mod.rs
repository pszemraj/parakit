//! Text-cleaning rules applied to raw ASR output before insertion.
//!
//! The default `safe` profile performs mechanical cleanup and high-confidence
//! normalization. Semantic edits such as deleting discourse markers or filler
//! uses of `like` are isolated in the opt-in `aggressive` profile.
//!
//! Most built-in rules use Rust's linear-time `regex` engine. A small,
//! bounded subset uses `fancy-regex` where backreferences materially reduce
//! duplication. More contextual transformations, including sentence
//! capitalization and spaced acronym normalization, are implemented as
//! procedural passes. On top of the built-in rule set, `config.toml` may
//! declare additional [`UserRule`]s. User rules are never profile-gated
//! (they are explicitly user-authored, so they are always active) but still
//! respect `disabled_rules`, and are spliced at a configurable
//! [`RulePosition`] relative to the built-in rules.
//!
//! Built-in rules are baked into this crate, not loaded from a config.
//! Adding or editing a built-in rule means editing
//! [`defaults::DEFAULT_RULES`].
//!
//! ## Module layout
//!
//! * [`engine`] - the compiled [`Cleaner`] pipeline and rule/activation types.
//! * [`defaults`] - the built-in `DEFAULT_RULES` table.
//! * [`passes`] - procedural transforms (acronyms, numbers, capitalization).
//! * [`user`] - user-defined rules loaded from `config.toml`.
//!
//! ## How to disable a rule at runtime
//!
//! Pass `--disable-rule <name>` (repeatable) on the CLI, or set
//! `cleaning.disabled_rules` in `config.toml`; pass `--no-cleaning` to skip
//! everything. `<name>` may name either a built-in or a user rule.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;

mod defaults;
mod engine;
mod passes;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
mod user;

pub use engine::Cleaner;
pub use user::{RulePosition, UserRule};

/// Schema/behavior version recorded in transcription logs.
pub const CLEANER_VERSION: u32 = 6;

/// Default minimum isolated number converted to digits when
/// `cleaning.number_threshold` is left unset.
///
/// An isolated number at or above this value is rendered as digits; a value
/// strictly below it is left exactly as the ASR model produced it (never
/// forced to words, never forced to digits). Configuring an explicit
/// `Some(0.0)` is the opt-out that converts every recognized number.
pub const DEFAULT_NUMBER_THRESHOLD: f64 = 4.0;

/// Built-in cleanup behavior tiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleaningProfile {
    /// Mechanical cleanup and high-confidence normalization only.
    Safe,
    /// Safe cleanup plus stylistic filler and discourse deletion.
    Aggressive,
}

impl CleaningProfile {
    /// Stable lowercase label used by the CLI, config, and logs.
    ///
    /// # Returns
    ///
    /// `"safe"` or `"aggressive"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Aggressive => "aggressive",
        }
    }
}

impl fmt::Display for CleaningProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CleaningProfile {
    type Err = anyhow::Error;

    /// # Errors
    ///
    /// Returns an error when `value` is neither `safe` nor `aggressive`
    /// (case-insensitive, surrounding whitespace ignored).
    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "safe" => Ok(Self::Safe),
            "aggressive" => Ok(Self::Aggressive),
            other => Err(anyhow!(
                "unknown cleaning profile '{other}'. Expected 'safe' or 'aggressive'"
            )),
        }
    }
}

/// One rule activation recorded for a cleaned transcript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuleHit {
    /// Rule name. Owned rather than `&'static str` because user-defined
    /// rules (see [`UserRule`]) have a name that is only known at runtime.
    pub name: String,
    /// Number of non-overlapping matches or procedural edits.
    pub matches: usize,
}

/// Cleaned text plus rule-level attribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanResult {
    /// Final cleaned transcript.
    pub text: String,
    /// Ordered list of transformations that changed the transcript.
    pub rules_fired: Vec<RuleHit>,
    /// Set when a bounded `fancy-regex` pass failed and [`Cleaner::clean`]
    /// fell back to the original, untransformed `text`. Always `None` from
    /// [`Cleaner::try_clean`], which propagates the error instead of
    /// recording it here.
    pub failure: Option<String>,
}

/// Build the cleaner selected by CLI/runtime options.
///
/// Rule name and user-rule validation always run before the `no_cleaning`
/// check, so a broken `config.toml` (an unknown disabled rule, an invalid
/// user rule) fails the same way whether or not cleaning itself ends up
/// disabled. This is load-bearing: `config::validate_config` calls this with
/// `no_cleaning = false` to validate a config file eagerly at load time,
/// independent of whatever `--no-cleaning` the caller might pass later.
///
/// # Arguments
///
/// * `no_cleaning` - Disable cleaning after validating rule names and user
///   rules.
/// * `profile` - Selected [`CleaningProfile`].
/// * `drop_trailing_period` - Enable the messaging-style terminal-period
///   removal rule.
/// * `number_threshold` - Minimum isolated number converted to digits.
///   `None` resolves to [`DEFAULT_NUMBER_THRESHOLD`]; `Some(0.0)` is the
///   explicit opt-out that converts every recognized number.
/// * `disabled_rules` - Rule names supplied by repeated `--disable-rule`
///   and/or `cleaning.disabled_rules` in `config.toml`. May name a built-in
///   or a user rule.
/// * `user_rules` - User-defined rules loaded from `config.toml`.
///
/// # Returns
///
/// `None` when cleaning is disabled, otherwise a compiled cleaner.
///
/// # Errors
///
/// Returns an error for a negative or non-finite `number_threshold`, an unknown
/// disabled rule name, an invalid built-in pattern, an empty or non-canonical
/// user rule name, an empty user rule pattern, a user rule name colliding with
/// a built-in name, a duplicate user rule name, or an invalid user rule regex.
/// A user rule that is itself named in `disabled_rules` is exempt from the
/// invalid-regex check: it is filtered out before its pattern is ever compiled,
/// which is the documented way to "park" a `[[rules.user]]` entry whose pattern
/// does not compile yet.
pub fn build_cleaner(
    no_cleaning: bool,
    profile: CleaningProfile,
    drop_trailing_period: bool,
    number_threshold: Option<f64>,
    disabled_rules: &[String],
    user_rules: &[UserRule],
) -> Result<Option<Cleaner>> {
    for name in disabled_rules {
        assert_rule_name_exists(name, user_rules)?;
    }
    if no_cleaning {
        engine::validate_number_threshold(number_threshold)?;
        user::validate_user_rules(user_rules)?;
        return Ok(None);
    }

    let disabled: HashSet<String> = disabled_rules.iter().cloned().collect();
    engine::Cleaner::new(
        profile,
        drop_trailing_period,
        number_threshold,
        &disabled,
        user_rules,
    )
    .map(Some)
}

/// Validate a rule name used by `--disable-rule` or `cleaning.disabled_rules`.
///
/// # Arguments
///
/// * `name` - Rule name to look up, as typed on the CLI or in config.
/// * `user_rules` - User-defined rules that extend the built-in set.
///
/// # Returns
///
/// `Ok(())` when the rule name is present, built-in or user-defined.
///
/// # Errors
///
/// Returns an error describing the unknown name when no matching rule
/// exists.
pub fn assert_rule_name_exists(name: &str, user_rules: &[UserRule]) -> Result<()> {
    if defaults::DEFAULT_RULES.iter().any(|rule| rule.name == name)
        || user_rules.iter().any(|rule| rule.name == name)
    {
        Ok(())
    } else {
        Err(anyhow!(
            "no rule named '{}'. Run with --list-rules to see all rules.",
            name
        ))
    }
}

/// Print all built-in and user passes and whether they are active for the
/// selected options.
///
/// The built-in table (name/engine/tier/enabled/description) is printed
/// first. If `user_rules` is non-empty, a separate `(user)` section with
/// each user rule's name, enabled state, and description is appended below
/// it.
///
/// # Arguments
///
/// * `profile` - Selected [`CleaningProfile`], used to compute the
///   `enabled` column for built-in rules.
/// * `drop_trailing_period` - Whether messaging-style terminal-period
///   removal is enabled.
/// * `disabled_rules` - Rule names disabled by `--disable-rule` and/or
///   `cleaning.disabled_rules`.
/// * `user_rules` - User-defined rules, printed in a separate section below
///   the built-in table when non-empty.
///
/// # Returns
///
/// `Ok(())` after printing the table(s) to stdout.
///
/// # Errors
///
/// Returns an error when a user rule has an invalid name or empty pattern, or
/// when a name in `disabled_rules` matches neither a built-in nor a user rule.
pub fn print_rule_list(
    profile: CleaningProfile,
    drop_trailing_period: bool,
    disabled_rules: &[String],
    user_rules: &[UserRule],
) -> Result<()> {
    user::validate_user_rules(user_rules)?;
    for name in disabled_rules {
        assert_rule_name_exists(name, user_rules)?;
    }
    let disabled: HashSet<&str> = disabled_rules.iter().map(String::as_str).collect();
    print!(
        "{}",
        render_rule_list(profile, drop_trailing_period, &disabled, user_rules)
    );
    Ok(())
}

/// Render the rule listing after names have been validated.
///
/// Split from [`print_rule_list`] so column content can be regression-tested
/// without capturing process-global stdout.
fn render_rule_list(
    profile: CleaningProfile,
    drop_trailing_period: bool,
    disabled: &HashSet<&str>,
    user_rules: &[UserRule],
) -> String {
    let mut output = String::new();
    // The tier column is sized for the longest label, `messaging-default`.
    writeln!(
        output,
        "{:<31}  {:<12}  {:<17}  {:<7}  description",
        "name", "engine", "tier", "enabled"
    )
    .expect("writing to a String cannot fail");
    writeln!(output, "{}", "-".repeat(114)).expect("writing to a String cannot fail");
    for rule in defaults::DEFAULT_RULES {
        let enabled =
            rule.activation.enabled(profile, drop_trailing_period) && !disabled.contains(rule.name);
        writeln!(
            output,
            "{:<31}  {:<12}  {:<17}  {:<7}  {}",
            rule.name,
            rule.kind.engine().label(),
            rule.activation.label(),
            if enabled { "yes" } else { "no" },
            rule.description,
        )
        .expect("writing to a String cannot fail");
    }

    if !user_rules.is_empty() {
        writeln!(output).expect("writing to a String cannot fail");
        writeln!(
            output,
            "{:<32}  {:<7}  description (user)",
            "name", "enabled"
        )
        .expect("writing to a String cannot fail");
        writeln!(output, "{}", "-".repeat(80)).expect("writing to a String cannot fail");
        for rule in user_rules {
            writeln!(
                output,
                "{:<32}  {:<7}  {}",
                rule.name,
                if disabled.contains(rule.name.as_str()) {
                    "no"
                } else {
                    "yes"
                },
                rule.description.as_deref().unwrap_or(""),
            )
            .expect("writing to a String cannot fail");
        }
    }
    output
}
