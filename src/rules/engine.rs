//! Compiled cleaning engine.
//!
//! This module owns the rule/activation type system and the [`Cleaner`]
//! pipeline that compiles and applies rules in order, including its stable
//! [`ruleset_id`](Cleaner::ruleset_id) fingerprint. Built-in rule
//! definitions live in [`crate::rules::defaults`]; user-defined rules live
//! in [`crate::rules::user`]. This module compiles both into a single
//! ordered pipeline, splicing user rules at their configured position
//! relative to the built-in rule list.

use anyhow::{bail, Context, Result};
use fancy_regex::{Regex as FancyRegex, RegexBuilder as FancyRegexBuilder};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashSet;

use super::defaults::DEFAULT_RULES;
use super::user::{compile_user_regex, validate_user_rules, RulePosition, UserRule};
use super::{CleanResult, CleaningProfile, RuleHit, CLEANER_VERSION, DEFAULT_NUMBER_THRESHOLD};

/// Backtrack step budget for every `fancy-regex` pass. Bounds worst-case
/// matching time for the small backreference-based subset of rules that use
/// the fancy-regex engine instead of the linear-time `regex` crate.
pub(crate) const FANCY_BACKTRACK_LIMIT: usize = 100_000;

/// Name of the first built-in rule in the final whitespace/punctuation
/// cleanup group. [`RulePosition::Standard`] user rules are spliced
/// immediately before this rule; [`RulePosition::First`] and
/// [`RulePosition::Last`] wrap the entire enabled built-in rule list
/// instead. The boundary is anchored to the canonical built-in order, so
/// disabling the boundary rule does not move `Standard` rules behind later
/// cleanup and capitalization passes.
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
/// pattern, a bounded `fancy-regex` pattern, a procedural transform function,
/// or the configurable spoken-number transform.
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
    SpokenNumbers,
}

impl RuleKind {
    /// # Returns
    ///
    /// The [`EngineKind`] that compiles and applies this rule.
    pub(crate) const fn engine(self) -> EngineKind {
        match self {
            Self::Regex { .. } => EngineKind::Regex,
            Self::FancyRegex { .. } => EngineKind::FancyRegex,
            Self::Procedural(_) | Self::SpokenNumbers => EngineKind::Procedural,
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
    SpokenNumbers {
        threshold: f64,
    },
}

impl CompiledTransform {
    fn engine(&self) -> EngineKind {
        match self {
            Self::Regex { .. } => EngineKind::Regex,
            Self::FancyRegex { .. } => EngineKind::FancyRegex,
            Self::Procedural(_) | Self::SpokenNumbers { .. } => EngineKind::Procedural,
        }
    }

    /// Pattern/replacement text used to fingerprint this transform in
    /// [`compute_ruleset_id`]. `None` for procedural passes, which have no
    /// externally visible pattern text.
    fn identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::Regex { re, replacement } => Some((re.as_str(), replacement.as_ref())),
            Self::FancyRegex { re, replacement } => Some((re.as_str(), replacement)),
            Self::Procedural(_) | Self::SpokenNumbers { .. } => None,
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
            CompiledTransform::SpokenNumbers { threshold } => {
                Ok(super::passes::normalize_spoken_numbers(input, *threshold))
            }
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
    number_threshold: f64,
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
    /// * `number_threshold` - Minimum isolated number converted to digits;
    ///   `None` resolves to [`DEFAULT_NUMBER_THRESHOLD`], and `Some(0.0)` is
    ///   the explicit opt-out that converts every recognized number.
    /// * `disabled` - Rule names to exclude, built-in or user-defined.
    /// * `user_rules` - User-defined rules to splice into the built-in list.
    ///
    /// # Returns
    ///
    /// A compiled [`Cleaner`] ready to clean transcripts.
    ///
    /// # Errors
    ///
    /// Returns an error if `number_threshold` is negative or non-finite, if
    /// user rule validation fails (see [`validate_user_rules`]), if any
    /// enabled built-in pattern is an invalid `regex` or `fancy-regex`
    /// expression, or if any enabled user rule pattern is an invalid `regex`
    /// expression.
    pub(crate) fn new(
        profile: CleaningProfile,
        drop_trailing_period: bool,
        number_threshold: Option<f64>,
        disabled: &HashSet<String>,
        user_rules: &[UserRule],
    ) -> Result<Self> {
        validate_number_threshold(number_threshold)?;
        validate_user_rules(user_rules)?;
        Self::assemble(
            profile,
            drop_trailing_period,
            number_threshold,
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
        validate_user_rules(user_rules)?;
        Self::assemble(
            profile,
            drop_trailing_period,
            None,
            disabled,
            user_rules,
            backtrack_limit,
        )
    }

    fn assemble(
        profile: CleaningProfile,
        drop_trailing_period: bool,
        number_threshold: Option<f64>,
        disabled: &HashSet<String>,
        user_rules: &[UserRule],
        backtrack_limit: usize,
    ) -> Result<Self> {
        // `None` (the key left unset) resolves to `DEFAULT_NUMBER_THRESHOLD`.
        // An explicit `Some(0.0)` is preserved verbatim as the opt-out that
        // converts every recognized number, so it now produces a different
        // ruleset id than an unset threshold. Validation has already
        // rejected negative and non-finite values.
        let number_threshold = number_threshold.unwrap_or(DEFAULT_NUMBER_THRESHOLD);
        let mut enabled_defaults = Vec::with_capacity(DEFAULT_RULES.len());
        let mut cleanup_idx = None;
        for def in DEFAULT_RULES {
            if def.name == CLEANUP_BOUNDARY_RULE_NAME {
                cleanup_idx = Some(enabled_defaults.len());
            }
            if def.activation.enabled(profile, drop_trailing_period) && !disabled.contains(def.name)
            {
                enabled_defaults.push(def);
            }
        }
        let cleanup_idx = cleanup_idx.unwrap_or(enabled_defaults.len());

        let mut rules = Vec::with_capacity(enabled_defaults.len() + user_rules.len());
        push_user_rules(&mut rules, user_rules, RulePosition::First, disabled)?;
        for def in &enabled_defaults[..cleanup_idx] {
            rules.push(compile_default_rule(
                def,
                backtrack_limit,
                number_threshold,
            )?);
        }
        push_user_rules(&mut rules, user_rules, RulePosition::Standard, disabled)?;
        for def in &enabled_defaults[cleanup_idx..] {
            rules.push(compile_default_rule(
                def,
                backtrack_limit,
                number_threshold,
            )?);
        }
        push_user_rules(&mut rules, user_rules, RulePosition::Last, disabled)?;

        let ruleset_id = compute_ruleset_id(profile, drop_trailing_period, &rules);
        Ok(Self {
            rules,
            profile,
            drop_trailing_period,
            number_threshold,
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

    /// Effective minimum isolated number converted to digits.
    ///
    /// # Returns
    ///
    /// [`DEFAULT_NUMBER_THRESHOLD`] when the configured threshold was
    /// unset, otherwise the configured value verbatim (including an
    /// explicit `0.0`, which converts every recognized number).
    #[must_use]
    pub const fn number_threshold(&self) -> f64 {
        self.number_threshold
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

fn compile_default_rule(
    def: &'static Rule,
    backtrack_limit: usize,
    number_threshold: f64,
) -> Result<CompiledRule> {
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
        RuleKind::SpokenNumbers => CompiledTransform::SpokenNumbers {
            threshold: number_threshold,
        },
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
        if let CompiledTransform::SpokenNumbers { threshold } = &rule.transform {
            // Procedural functions have no pattern text to fingerprint. Hash
            // this configurable input so behaviorally different number
            // policies can never share a ruleset identifier.
            hasher.update([0]);
            hasher.update(b"number-threshold");
            hasher.update(threshold.to_bits().to_le_bytes());
        }
        if let Some(position) = rule.position {
            hasher.update([0]);
            hasher.update(position.as_str().as_bytes());
        }
    }

    let digest = hasher.finalize();
    let short_hash = crate::checksum::hex_digest(&digest[..8]);
    format!("v{CLEANER_VERSION}-{}-{short_hash}", profile.as_str())
}

/// Validate the optional isolated-number conversion threshold.
///
/// # Arguments
///
/// * `threshold` - Minimum isolated numeric value rendered as digits, or
///   `None` to resolve to [`DEFAULT_NUMBER_THRESHOLD`].
///
/// # Returns
///
/// `Ok(())` when the threshold is absent, zero, or a finite positive value.
///
/// # Errors
///
/// Returns an error when the threshold is negative, NaN, or infinite.
pub(crate) fn validate_number_threshold(threshold: Option<f64>) -> Result<()> {
    if let Some(value) = threshold {
        if !value.is_finite() || value < 0.0 {
            bail!(
                "number threshold must be a finite value greater than or equal to 0 (got {value})"
            );
        }
    }
    Ok(())
}
