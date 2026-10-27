//! User-defined cleaning rules loaded from `config.toml`.
//!
//! A [`UserRule`] owns its compiled regex. Deserialization compiles the
//! configured pattern, and [`validate_user_rules`] checks names and required
//! values. Enabled rules are spliced into the pipeline assembled by
//! [`crate::rules::engine`] at their configured [`RulePosition`]. Unlike
//! built-in rules, user rules are never
//! profile-gated: they are explicitly user-authored, so they are always
//! active regardless of [`crate::rules::CleaningProfile`]. They still
//! respect `disabled_rules`.

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::defaults::DEFAULT_RULES;

/// Where a [`UserRule`] is spliced relative to the built-in rule list.
///
/// `Standard` (the default) runs alongside the bulk of the built-in rules,
/// before the final whitespace/punctuation cleanup group. `First` and `Last`
/// wrap the entire built-in rule list, which is useful for rules that must
/// see raw ASR output before any built-in rule touches it, or that must run
/// after all built-in cleanup (including whitespace/punctuation and sentence
/// capitalization) has settled.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RulePosition {
    /// Run before every built-in rule.
    First,
    /// Run with the bulk of the built-in rules, before the final
    /// whitespace/punctuation cleanup group. This is the default.
    #[default]
    Standard,
    /// Run after every built-in rule, including whitespace/punctuation
    /// cleanup and sentence capitalization.
    Last,
}

impl RulePosition {
    /// Stable label, matching the value accepted in `config.toml`. Also
    /// used to fold a user rule's position into
    /// [`Cleaner::ruleset_id`](crate::rules::Cleaner::ruleset_id).
    ///
    /// # Returns
    ///
    /// `first`, `standard`, or `last`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Standard => "standard",
            Self::Last => "last",
        }
    }
}

/// A user-supplied text-cleaning rule loaded from `config.toml`.
///
/// The pattern compiles during deserialization and is shared with each
/// cleaner built from this rule. User rules use the `regex` crate, never
/// `fancy-regex`. Names must not collide with built-in or other user rules.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "ConfiguredUserRule")]
pub struct UserRule {
    /// Unique rule name. Must not have surrounding whitespace or collide
    /// with a built-in rule name or another user rule name.
    pub name: String,
    /// Optional human-readable description, shown by `parakit rules list`.
    pub description: Option<String>,
    /// Compiled Rust regex; [`Regex::as_str`] returns its configured pattern.
    pub pattern: Regex,
    /// Replacement string in the `regex` crate's expansion syntax: `$0`, `$1`, `$name`, `${name}`, and `$$` for a literal dollar sign.
    /// The name after a `$` is the longest run of letters, digits, and underscores, so `$1b` means a group named "1b"; write `${1}b` for group 1 followed by "b".
    /// A reference to a group that does not exist expands to empty text without an error.
    pub replacement: String,
    /// Where this rule is spliced relative to the built-in rule list.
    pub position: RulePosition,
}

/// Serialized fields converted into a rule with an owned, compiled pattern.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredUserRule {
    name: String,
    description: Option<String>,
    pattern: String,
    replacement: String,
    #[serde(default)]
    position: RulePosition,
}

impl TryFrom<ConfiguredUserRule> for UserRule {
    type Error = anyhow::Error;

    /// Compile the configured pattern while retaining its rule name in errors.
    ///
    /// # Errors
    /// Returns a named diagnostic when the regex is invalid.
    fn try_from(rule: ConfiguredUserRule) -> Result<Self> {
        let pattern = Regex::new(&rule.pattern)
            .map_err(|error| anyhow!("user rule '{}' has invalid regex: {error}", rule.name))?;
        Ok(Self {
            name: rule.name,
            description: rule.description,
            pattern,
            replacement: rule.replacement,
            position: rule.position,
        })
    }
}

/// Validate that every user rule has a canonical, non-empty name and a
/// non-empty pattern, that no user rule collides with a built-in name, and
/// that no two user rules share a name. A canonical name has no leading or
/// trailing whitespace, so validation, `disabled_rules` lookup, and rule-list
/// output all use exactly the same identifier. Pattern validity follows
/// from the compiled [`Regex`] owned by every rule.
///
/// # Returns
///
/// `Ok(())` when every user rule passes validation.
///
/// # Errors
///
/// Returns an error naming the offending rule when: a rule's `name` is
/// empty or whitespace-only; a rule's `name` has leading or trailing
/// whitespace; a rule's `name` collides with a built-in rule name; two user
/// rules share a `name`; or a rule's `pattern` is the empty string. Rejecting
/// the literal empty value catches an incomplete rule definition; it is not a
/// general ban on valid regexes that can produce zero-width matches (for
/// example `()` or `\b`). A whitespace-only pattern remains legal and matches
/// a literal space.
pub(crate) fn validate_user_rules(user_rules: &[UserRule]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(user_rules.len());
    for (idx, rule) in user_rules.iter().enumerate() {
        let trimmed_name = rule.name.trim();
        if trimmed_name.is_empty() {
            return Err(anyhow!("user rule #{} has an empty name", idx + 1));
        }
        if trimmed_name != rule.name {
            return Err(anyhow!(
                "user rule '{}' has leading or trailing whitespace in its name",
                rule.name
            ));
        }
        if DEFAULT_RULES.iter().any(|def| def.name == rule.name) {
            return Err(anyhow!(
                "user rule '{}' has the same name as a built-in rule; rename it",
                rule.name
            ));
        }
        if !seen.insert(rule.name.as_str()) {
            return Err(anyhow!("duplicate user rule name '{}'", rule.name));
        }
        if rule.pattern.as_str().is_empty() {
            // This is a required-config-value check, not zero-width regex
            // analysis. Valid expressions that match only a boundary remain
            // accepted patterns.
            return Err(anyhow!("user rule '{}' has an empty pattern", rule.name));
        }
    }
    Ok(())
}
