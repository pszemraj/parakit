//! User-defined cleaning rules loaded from `config.toml`.
//!
//! A [`UserRule`] is validated once at load time (see
//! [`validate_user_rules`]) and, when enabled, compiled and spliced into the
//! pipeline assembled by [`crate::rules::engine`] at its configured
//! [`RulePosition`]. Unlike built-in rules, user rules are never
//! profile-gated: they are explicitly user-authored, so they are always
//! active regardless of [`crate::rules::CleaningProfile`]. They still
//! respect `disabled_rules`.

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::Deserialize;
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
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize)]
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
/// Compiled the same way as a built-in rule (the `regex` crate; user rules
/// never use `fancy-regex`), but user-supplied and thus validated at load
/// time: the pattern must be a valid regex, and the name must not collide
/// with a built-in rule name or another user rule name.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserRule {
    /// Unique rule name. Must not have surrounding whitespace or collide
    /// with a built-in rule name or another user rule name.
    pub name: String,
    /// Optional human-readable description, shown by `parakit rules list`.
    pub description: Option<String>,
    /// Rust `regex` crate pattern (same dialect as built-in `regex`-engine
    /// rules).
    pub pattern: String,
    /// Replacement string. Supports `$1`, `$2`, etc. capture references.
    pub replacement: String,
    /// Where this rule is spliced relative to the built-in rule list.
    #[serde(default)]
    pub position: RulePosition,
}

/// Validate that every user rule has a canonical, non-empty name and a
/// non-empty pattern, that no user rule collides with a built-in name, and
/// that no two user rules share a name. A canonical name has no leading or
/// trailing whitespace, so validation, `disabled_rules` lookup, and rule-list
/// output all use exactly the same identifier. Regex *validity* (whether the
/// pattern compiles) is checked when a rule is compiled
/// ([`compile_user_regex`]), not here, so this stays cheap to call
/// unconditionally.
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
        if rule.pattern.is_empty() {
            // This is a required-config-value check, not zero-width regex
            // analysis. Other valid expressions may intentionally match at a
            // boundary and are accepted by `compile_user_regex` below.
            return Err(anyhow!("user rule '{}' has an empty pattern", rule.name));
        }
    }
    Ok(())
}

/// Compile a user rule's `pattern` into a `regex::Regex`.
///
/// Does not check `disabled_rules`; callers must filter disabled rules out
/// before calling this so a disabled user rule's invalid regex never fails
/// a build (see `push_user_rules` in [`crate::rules::engine`]).
///
/// # Returns
///
/// The compiled [`Regex`] for `rule.pattern`.
///
/// # Errors
///
/// Returns an error naming the rule when its pattern is an invalid regex.
pub(crate) fn compile_user_regex(rule: &UserRule) -> Result<Regex> {
    Regex::new(&rule.pattern)
        .map_err(|err| anyhow!("user rule '{}' has invalid regex: {err}", rule.name))
}
