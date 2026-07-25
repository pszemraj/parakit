//! Compiled cleaning engine.
//!
//! This module owns the rule/activation type system and the [`Cleaner`]
//! pipeline that compiles and applies rules in order, including its stable
//! [`ruleset_id`](Cleaner::ruleset_id) fingerprint. Built-in rule
//! definitions live in [`crate::rules::defaults`]; user-defined rules live
//! in [`crate::rules::user`]. This module compiles both into a single
//! ordered pipeline, splicing user rules at their configured position
//! relative to the built-in rule list.

use anyhow::{Context, Result};
use fancy_regex::{Regex as FancyRegex, RegexBuilder as FancyRegexBuilder};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

use super::defaults::DEFAULT_RULES;
use super::user::{compile_user_regex, validate_user_rules, RulePosition, UserRule};
use super::{CleanResult, CleaningProfile, RuleHit, CLEANER_VERSION};

/// Backtrack step budget for every `fancy-regex` pass. Bounds worst-case
/// matching time for the small backreference-based subset of rules that use
/// the fancy-regex engine instead of the linear-time `regex` crate.
pub(crate) const FANCY_BACKTRACK_LIMIT: usize = 100_000;

/// Name of the first built-in rule in the final whitespace/punctuation
/// cleanup group. [`RulePosition::Standard`] user rules are spliced
/// immediately before this rule; [`RulePosition::First`] and
/// [`RulePosition::Last`] wrap the entire enabled built-in rule list
/// instead. The boundary is computed over the *enabled* built-in list: if no
/// enabled rule has this name (disabled, or filtered out by the active
/// profile), `Standard` user rules are appended at the end of the built-in
/// list instead.
pub(crate) const CLEANUP_BOUNDARY_RULE_NAME: &str = "fix-space-before-punct";

/// Built-in cleanup activation tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Activation {
    /// Always active in both cleaning profiles.
    Safe,
    /// Active only under [`CleaningProfile::Aggressive`].
    Aggressive,
    /// Active only when the caller opts into trailing-period removal.
    DropTrailingPeriod,
}

impl Activation {
    /// Label shown in the `--list-rules` tier column.
    ///
    /// # Returns
    ///
    /// `"safe"`, `"aggressive"`, or `"messaging-default"`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Aggressive => "aggressive",
            Self::DropTrailingPeriod => "messaging-default",
        }
    }

    /// Whether a rule with this activation tier is active for the given
    /// profile and trailing-period setting.
    ///
    /// # Arguments
    ///
    /// * `profile` - Selected [`CleaningProfile`].
    /// * `drop_trailing_period` - Whether messaging-style terminal-period
    ///   removal is enabled.
    ///
    /// # Returns
    ///
    /// `true` for `Safe` unconditionally, for `Aggressive` only when
    /// `profile` is [`CleaningProfile::Aggressive`], and for
    /// `DropTrailingPeriod` only when `drop_trailing_period` is `true`.
    pub(crate) const fn enabled(
        self,
        profile: CleaningProfile,
        drop_trailing_period: bool,
    ) -> bool {
        match self {
            Self::Safe => true,
            Self::Aggressive => matches!(profile, CleaningProfile::Aggressive),
            Self::DropTrailingPeriod => drop_trailing_period,
        }
    }
}

/// Which engine compiles and applies a built-in rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EngineKind {
    Regex,
    FancyRegex,
    Procedural,
}

impl EngineKind {
    /// Label shown in the `--list-rules` engine column.
    ///
    /// # Returns
    ///
    /// `"regex"`, `"fancy-regex"`, or `"procedural"`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Regex => "regex",
            Self::FancyRegex => "fancy-regex",
            Self::Procedural => "procedural",
        }
    }
}

/// How a built-in [`Rule`] is compiled and applied: a linear-time `regex`
/// pattern, a bounded `fancy-regex` pattern, or a procedural transform
/// function.
#[derive(Clone, Copy)]
pub(crate) enum RuleKind {
    Regex {
        pattern: &'static str,
        replacement: &'static str,
    },
    FancyRegex {
        pattern: &'static str,
        replacement: &'static str,
    },
    Procedural(fn(&str) -> TransformResult),
}

impl RuleKind {
    /// # Returns
    ///
    /// The [`EngineKind`] that compiles and applies this rule.
    pub(crate) const fn engine(self) -> EngineKind {
        match self {
            Self::Regex { .. } => EngineKind::Regex,
            Self::FancyRegex { .. } => EngineKind::FancyRegex,
            Self::Procedural(_) => EngineKind::Procedural,
        }
    }
}

/// A single built-in cleaning rule definition.
#[derive(Clone, Copy)]
pub(crate) struct Rule {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) activation: Activation,
    pub(crate) kind: RuleKind,
}

/// Output of one rule/pass application.
pub(crate) struct TransformResult {
    pub(crate) text: String,
    pub(crate) matches: usize,
}

#[derive(Debug)]
enum CompiledTransform {
    Regex {
        re: Regex,
        replacement: Cow<'static, str>,
    },
    FancyRegex {
        re: FancyRegex,
        replacement: &'static str,
    },
    Procedural(fn(&str) -> TransformResult),
}

impl CompiledTransform {
    fn engine(&self) -> EngineKind {
        match self {
            Self::Regex { .. } => EngineKind::Regex,
            Self::FancyRegex { .. } => EngineKind::FancyRegex,
            Self::Procedural(_) => EngineKind::Procedural,
        }
    }

    /// Pattern/replacement text used to fingerprint this transform in
    /// [`compute_ruleset_id`]. `None` for procedural passes, which have no
    /// externally visible pattern text.
    fn identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::Regex { re, replacement } => Some((re.as_str(), replacement.as_ref())),
            Self::FancyRegex { re, replacement } => Some((re.as_str(), replacement)),
            Self::Procedural(_) => None,
        }
    }
}

#[derive(Debug)]
struct CompiledRule {
    name: String,
    /// `Some` only for spliced user rules; carries the configured position
    /// so [`compute_ruleset_id`] can distinguish two rule sets that differ
    /// only in where a same-named, same-patterned rule is spliced.
    position: Option<RulePosition>,
    transform: CompiledTransform,
}

impl CompiledRule {
    fn apply(&self, input: &str) -> Result<TransformResult> {
        match &self.transform {
            CompiledTransform::Regex { re, replacement } => {
                let matches = re.find_iter(input).count();
                if matches == 0 {
                    return Ok(TransformResult {
                        text: input.to_string(),
                        matches: 0,
                    });
                }
                Ok(TransformResult {
                    text: re.replace_all(input, replacement.as_ref()).into_owned(),
                    matches,
                })
            }
            CompiledTransform::FancyRegex { re, replacement } => {
                let mut matches = 0;
                for found in re.find_iter(input) {
                    found.with_context(|| {
                        format!("rule '{}' failed during fancy-regex matching", self.name)
                    })?;
                    matches += 1;
                }
                if matches == 0 {
                    return Ok(TransformResult {
                        text: input.to_string(),
                        matches: 0,
                    });
                }
                let text = re
                    .try_replacen(input, 0, *replacement)
                    .with_context(|| {
                        format!("rule '{}' failed during fancy-regex replacement", self.name)
                    })?
                    .into_owned();
                Ok(TransformResult { text, matches })
            }
            CompiledTransform::Procedural(transform) => Ok(transform(input)),
        }
    }
}

/// Compiled transcript-cleaning pipeline.
///
/// Built by [`build_cleaner`](super::build_cleaner) (or, in tests, by
/// [`Cleaner::new`] directly). Compiles the enabled built-in rules plus any
/// enabled user rules, spliced at their configured [`RulePosition`], into a
/// single ordered pipeline.
#[derive(Debug)]
pub struct Cleaner {
    rules: Vec<CompiledRule>,
    profile: CleaningProfile,
    drop_trailing_period: bool,
    ruleset_id: String,
}

impl Cleaner {
    /// Compile the enabled built-in and user rules into an ordered
    /// pipeline, using the production `fancy-regex` backtrack limit.
    ///
    /// # Arguments
    ///
    /// * `profile` - Selected [`CleaningProfile`].
    /// * `drop_trailing_period` - Enable the messaging-style terminal-period
    ///   removal rule.
    /// * `disabled` - Rule names to exclude, built-in or user-defined.
    /// * `user_rules` - User-defined rules to splice into the built-in list.
    ///
    /// # Returns
    ///
    /// A compiled [`Cleaner`] ready to clean transcripts.
    ///
    /// # Errors
    ///
    /// Returns an error if user rule validation fails (see
    /// [`validate_user_rules`]), if any enabled built-in pattern is an
    /// invalid `regex` or `fancy-regex` expression, or if any enabled user
    /// rule pattern is an invalid `regex` expression.
    pub(crate) fn new(
        profile: CleaningProfile,
        drop_trailing_period: bool,
        disabled: &HashSet<String>,
        user_rules: &[UserRule],
    ) -> Result<Self> {
        Self::assemble(
            profile,
            drop_trailing_period,
            disabled,
            user_rules,
            FANCY_BACKTRACK_LIMIT,
        )
    }

    /// Test-only constructor that accepts a deliberately tiny `fancy-regex`
    /// backtrack limit, so the fail-open path in [`Cleaner::clean`] can be
    /// exercised deterministically instead of relying on pathological input
    /// to exhaust the much larger production limit.
    ///
    /// # Arguments
    ///
    /// * `profile` - Selected [`CleaningProfile`].
    /// * `drop_trailing_period` - Enable the messaging-style terminal-period
    ///   removal rule.
    /// * `disabled` - Rule names to exclude, built-in or user-defined.
    /// * `user_rules` - User-defined rules to splice into the built-in list.
    /// * `backtrack_limit` - `fancy-regex` backtrack step budget to compile
    ///   with, in place of [`FANCY_BACKTRACK_LIMIT`].
    ///
    /// # Returns
    ///
    /// A compiled [`Cleaner`] ready to clean transcripts.
    ///
    /// # Errors
    ///
    /// Same as [`Cleaner::new`].
    #[cfg(test)]
    pub(crate) fn new_with_backtrack_limit(
        profile: CleaningProfile,
        drop_trailing_period: bool,
        disabled: &HashSet<String>,
        user_rules: &[UserRule],
        backtrack_limit: usize,
    ) -> Result<Self> {
        Self::assemble(
            profile,
            drop_trailing_period,
            disabled,
            user_rules,
            backtrack_limit,
        )
    }

    fn assemble(
        profile: CleaningProfile,
        drop_trailing_period: bool,
        disabled: &HashSet<String>,
        user_rules: &[UserRule],
        backtrack_limit: usize,
    ) -> Result<Self> {
        validate_user_rules(user_rules)?;

        let enabled_defaults: Vec<&'static Rule> = DEFAULT_RULES
            .iter()
            .filter(|def| {
                def.activation.enabled(profile, drop_trailing_period)
                    && !disabled.contains(def.name)
            })
            .collect();
        let cleanup_idx = enabled_defaults
            .iter()
            .position(|def| def.name == CLEANUP_BOUNDARY_RULE_NAME)
            .unwrap_or(enabled_defaults.len());

        let mut rules = Vec::with_capacity(enabled_defaults.len() + user_rules.len());
        push_user_rules(&mut rules, user_rules, RulePosition::First, disabled)?;
        for def in &enabled_defaults[..cleanup_idx] {
            rules.push(compile_default_rule(def, backtrack_limit)?);
        }
        push_user_rules(&mut rules, user_rules, RulePosition::Standard, disabled)?;
        for def in &enabled_defaults[cleanup_idx..] {
            rules.push(compile_default_rule(def, backtrack_limit)?);
        }
        push_user_rules(&mut rules, user_rules, RulePosition::Last, disabled)?;

        let ruleset_id = compute_ruleset_id(profile, drop_trailing_period, &rules);
        Ok(Self {
            rules,
            profile,
            drop_trailing_period,
            ruleset_id,
        })
    }

    /// Apply every enabled pass in order and retain rule-level attribution.
    ///
    /// # Returns
    ///
    /// A [`CleanResult`] with the cleaned `text`, the ordered `rules_fired`
    /// that changed the transcript, and `failure` always `None` (a failure
    /// aborts this method with `Err` instead of being recorded here).
    ///
    /// # Errors
    ///
    /// Returns an error if a bounded `fancy-regex` pass hits its runtime
    /// backtrack limit. Callers on a hot path (e.g. the daemon worker
    /// thread) should prefer [`Cleaner::clean`], which fails open to the
    /// original transcript instead of propagating this error.
    pub fn try_clean(&self, input: &str) -> Result<CleanResult> {
        let mut text = input.to_string();
        let mut rules_fired = Vec::new();

        for rule in &self.rules {
            let transformed = rule.apply(&text)?;
            if transformed.matches == 0 || transformed.text == text {
                continue;
            }
            rules_fired.push(RuleHit {
                name: rule.name.clone(),
                matches: transformed.matches,
            });
            text = transformed.text;
        }

        Ok(CleanResult {
            text,
            rules_fired,
            failure: None,
        })
    }

    /// Apply every enabled pass, failing open to the original transcript.
    ///
    /// If a bounded `fancy-regex` pass hits its runtime backtrack limit,
    /// this returns the original `input` unchanged (never a
    /// partially-transformed string) with [`CleanResult::failure`] set to a
    /// description of the error. This method never panics; it is the
    /// preferred entry point for production paths such as the daemon
    /// worker thread, where a panic would take down the worker.
    ///
    /// # Returns
    ///
    /// A [`CleanResult`]. On success it matches [`Cleaner::try_clean`]'s
    /// result. On a pass failure, `text` is the original `input`,
    /// `rules_fired` is empty, and `failure` holds the error description.
    ///
    /// # Errors
    ///
    /// This method is infallible: it returns `CleanResult`, not `Result`,
    /// and never propagates an `Err`. A cleaning-pass failure is caught
    /// internally and surfaced through [`CleanResult::failure`] instead of
    /// as a `Result::Err`.
    #[must_use]
    pub fn clean(&self, input: &str) -> CleanResult {
        match self.try_clean(input) {
            Ok(result) => result,
            Err(err) => CleanResult {
                text: input.to_string(),
                rules_fired: Vec::new(),
                failure: Some(format!("{err:#}")),
            },
        }
    }

    /// Apply every enabled pass and return only the cleaned text.
    ///
    /// # Returns
    ///
    /// The `text` field of [`Cleaner::clean`]'s result.
    #[must_use]
    pub fn clean_text(&self, input: &str) -> String {
        self.clean(input).text
    }

    /// Number of enabled passes after profile, disable, and user-rule
    /// splicing.
    ///
    /// # Returns
    ///
    /// The number of compiled rules in the pipeline.
    #[must_use]
    pub fn active_rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Selected cleanup profile.
    ///
    /// # Returns
    ///
    /// The [`CleaningProfile`] this cleaner was built with.
    #[must_use]
    pub const fn profile(&self) -> CleaningProfile {
        self.profile
    }

    /// Whether terminal-period deletion is enabled for messaging-style
    /// output.
    ///
    /// # Returns
    ///
    /// The `drop_trailing_period` flag this cleaner was built with.
    #[must_use]
    pub const fn drops_trailing_period(&self) -> bool {
        self.drop_trailing_period
    }

    /// Stable identifier derived from the ordered enabled pass set,
    /// including spliced user rules.
    ///
    /// # Returns
    ///
    /// The ruleset fingerprint computed in [`compute_ruleset_id`] at
    /// construction time.
    #[must_use]
    pub fn ruleset_id(&self) -> &str {
        &self.ruleset_id
    }
}

fn compile_default_rule(def: &'static Rule, backtrack_limit: usize) -> Result<CompiledRule> {
    let transform = match def.kind {
        RuleKind::Regex {
            pattern,
            replacement,
        } => CompiledTransform::Regex {
            re: Regex::new(pattern)
                .with_context(|| format!("rule '{}' has invalid regex", def.name))?,
            replacement: Cow::Borrowed(replacement),
        },
        RuleKind::FancyRegex {
            pattern,
            replacement,
        } => CompiledTransform::FancyRegex {
            re: {
                let mut builder = FancyRegexBuilder::new(pattern);
                builder.backtrack_limit(backtrack_limit);
                builder
                    .build()
                    .with_context(|| format!("rule '{}' has invalid fancy regex", def.name))?
            },
            replacement,
        },
        RuleKind::Procedural(transform) => CompiledTransform::Procedural(transform),
    };
    Ok(CompiledRule {
        name: def.name.to_string(),
        position: None,
        transform,
    })
}

fn compile_user_rule(rule: &UserRule) -> Result<CompiledRule> {
    let re = compile_user_regex(rule)?;
    Ok(CompiledRule {
        name: rule.name.clone(),
        position: Some(rule.position),
        transform: CompiledTransform::Regex {
            re,
            replacement: Cow::Owned(rule.replacement.clone()),
        },
    })
}

/// Compile and append enabled `user_rules` entries at `position` to `rules`,
/// skipping names present in `disabled`. A disabled user rule's pattern is
/// never compiled, so it may be invalid without failing the build.
///
/// # Errors
///
/// Returns an error naming the rule when its pattern is an invalid regex.
fn push_user_rules(
    rules: &mut Vec<CompiledRule>,
    user_rules: &[UserRule],
    position: RulePosition,
    disabled: &HashSet<String>,
) -> Result<()> {
    for rule in user_rules.iter().filter(|rule| rule.position == position) {
        if disabled.contains(&rule.name) {
            continue;
        }
        rules.push(compile_user_rule(rule)?);
    }
    Ok(())
}

fn compute_ruleset_id(
    profile: CleaningProfile,
    drop_trailing_period: bool,
    rules: &[CompiledRule],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CLEANER_VERSION.to_le_bytes());
    hasher.update(profile.as_str().as_bytes());
    hasher.update([u8::from(drop_trailing_period)]);
    for rule in rules {
        hasher.update([0]);
        hasher.update(rule.name.as_bytes());
        hasher.update([0]);
        hasher.update(rule.transform.engine().label().as_bytes());
        if let Some((pattern, replacement)) = rule.transform.identity() {
            hasher.update([0]);
            hasher.update(pattern.as_bytes());
            hasher.update([0]);
            hasher.update(replacement.as_bytes());
        }
        if let Some(position) = rule.position {
            hasher.update([0]);
            hasher.update(position.as_str().as_bytes());
        }
    }

    let digest = hasher.finalize();
    let mut short_hash = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        write!(&mut short_hash, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("v{CLEANER_VERSION}-{}-{short_hash}", profile.as_str())
}
