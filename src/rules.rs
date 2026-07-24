//! Text-cleaning rules applied to raw ASR output before insertion.
//!
//! Rules are baked into this file, not loaded from a config. Adding or editing
//! a rule = editing this file. This is intentional: it makes the rule set easy
//! for an LM to maintain and produces a single self-contained binary.
//!
//! ## How rules are applied
//!
//! Rules apply in the order they appear in `DEFAULT_RULES`. Each rule's
//! output is the next rule's input. Order matters — for example, leading
//! filler removal ("So, ") happens before whitespace collapse, otherwise the
//! comma-space leftovers wouldn't get cleaned.
//!
//! ## How to add a rule
//!
//! Append a `Rule` entry to `DEFAULT_RULES`. Use Rust regex syntax
//! (effectively PCRE-lite, no lookbehind). Use `(?i)` for case-insensitive,
//! `\b` for word boundaries, `$1` etc. for capture references.
//!
//! ```text
//! Rule {
//!     name: "my-rule",
//!     description: "What this rule does",
//!     pattern: r"(?i)\bmyword\b",
//!     replacement: "yourword",
//! },
//! ```
//!
//! ## How to disable a rule at runtime
//!
//! Pass `--disable-rule <name>` (repeatable) on the CLI, or `--no-cleaning`
//! to skip everything.

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashSet;

/// A single text-cleaning rule.
struct Rule {
    name: &'static str,
    description: &'static str,
    pattern: &'static str,
    replacement: &'static str,
}

/// Compiled at startup.
#[derive(Debug)]
struct CompiledRule {
    re: Regex,
    replacement: Cow<'static, str>,
}

/// Where a [`UserRule`] is spliced relative to [`DEFAULT_RULES`].
///
/// `Standard` (the default) runs alongside the bulk of the built-in rules,
/// before the final whitespace/punctuation cleanup group. `First` and `Last`
/// wrap the entire built-in rule list, which is useful for rules that must
/// see raw ASR output before any built-in rule touches it, or that must run
/// after all built-in cleanup (including whitespace/punctuation) has settled.
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
    /// cleanup.
    Last,
}

impl RulePosition {
    /// Stable label, matching the value accepted in `config.toml`.
    ///
    /// # Returns
    ///
    /// `first`, `standard`, or `last`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Standard => "standard",
            Self::Last => "last",
        }
    }
}

/// A user-supplied text-cleaning rule loaded from `config.toml`.
///
/// Compiled the same way as a built-in [`Rule`], but user-supplied and thus
/// validated at load time: the pattern must be a valid regex, and the name
/// must not collide with a built-in rule name or another user rule name.
#[derive(Debug, Clone, Deserialize)]
pub struct UserRule {
    /// Unique rule name. Must not collide with a built-in rule name or
    /// another user rule name.
    pub name: String,
    /// Optional human-readable description, shown by `--list-rules`.
    pub description: Option<String>,
    /// Rust `regex` crate pattern (same dialect as built-in rules).
    pub pattern: String,
    /// Replacement string. Supports `$1`, `$2`, etc. capture references.
    pub replacement: String,
    /// Where this rule is spliced relative to the built-in rule list.
    #[serde(default)]
    pub position: RulePosition,
}

/// Name of the first built-in rule in the final whitespace/punctuation
/// cleanup group. `RulePosition::Standard` user rules are inserted
/// immediately before this rule; `RulePosition::First`/`RulePosition::Last`
/// wrap the entire built-in rule list instead.
const CLEANUP_BOUNDARY_RULE_NAME: &str = "fix-space-before-punct";

/// Driver. Build once, call `clean()` per transcription.
#[derive(Debug)]
pub struct Cleaner {
    rules: Vec<CompiledRule>,
}

impl Cleaner {
    /// Compile all default rules whose name is not in `disabled`, spliced
    /// with `user_rules` at their configured [`RulePosition`].
    ///
    /// # Returns
    ///
    /// A cleaner containing the default-enabled and user-enabled rules that
    /// were not disabled, in application order.
    ///
    /// # Errors
    ///
    /// Returns an error if any enabled rule (built-in or user) contains an
    /// invalid regex pattern, if a user rule name collides with a built-in
    /// rule name, or if two user rules share the same name.
    fn new(disabled: &HashSet<String>, user_rules: &[UserRule]) -> Result<Self> {
        validate_user_rules(user_rules)?;

        let cleanup_idx = DEFAULT_RULES
            .iter()
            .position(|r| r.name == CLEANUP_BOUNDARY_RULE_NAME)
            .unwrap_or(DEFAULT_RULES.len());

        let mut rules = Vec::with_capacity(DEFAULT_RULES.len() + user_rules.len());
        push_user_rules(&mut rules, user_rules, RulePosition::First, disabled)?;
        push_default_rules(&mut rules, &DEFAULT_RULES[..cleanup_idx], disabled)?;
        push_user_rules(&mut rules, user_rules, RulePosition::Standard, disabled)?;
        push_default_rules(&mut rules, &DEFAULT_RULES[cleanup_idx..], disabled)?;
        push_user_rules(&mut rules, user_rules, RulePosition::Last, disabled)?;

        Ok(Self { rules })
    }

    /// Apply all enabled rules in order. Idempotent for stable input.
    ///
    /// # Returns
    ///
    /// The cleaned transcript.
    pub fn clean(&self, input: &str) -> String {
        let mut s = input.to_string();
        for r in &self.rules {
            let replaced = r.re.replace_all(&s, r.replacement.as_ref());
            if matches!(replaced, std::borrow::Cow::Owned(_)) {
                s = replaced.into_owned();
            }
        }
        capitalize_sentence_starts(&s)
    }

    /// Number of active rules after disable filtering.
    ///
    /// # Returns
    ///
    /// The number of active cleanup rules.
    pub fn active_rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// Validate that no user rule collides with a built-in name and that no two
/// user rules share a name. Regex validity is checked when a rule is
/// compiled (`push_user_rules`), not here, so this stays cheap to call
/// unconditionally.
///
/// # Errors
///
/// Returns an error naming the offending rule when a user rule's name
/// collides with a built-in rule name, or when two user rules share a name.
fn validate_user_rules(user_rules: &[UserRule]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(user_rules.len());
    for ur in user_rules {
        if DEFAULT_RULES.iter().any(|r| r.name == ur.name) {
            return Err(anyhow!(
                "user rule '{}' has the same name as a built-in rule; rename it",
                ur.name
            ));
        }
        if !seen.insert(ur.name.as_str()) {
            return Err(anyhow!("duplicate user rule name '{}'", ur.name));
        }
    }
    Ok(())
}

/// Compile and append enabled entries from `defs` (a slice of
/// [`DEFAULT_RULES`]) to `rules`, skipping names present in `disabled`.
///
/// # Errors
///
/// Returns an error naming the rule when its pattern is an invalid regex.
fn push_default_rules(
    rules: &mut Vec<CompiledRule>,
    defs: &[Rule],
    disabled: &HashSet<String>,
) -> Result<()> {
    for r in defs {
        if disabled.contains(r.name) {
            continue;
        }
        let re = Regex::new(r.pattern)
            .with_context(|| format!("rule '{}' has invalid regex", r.name))?;
        rules.push(CompiledRule {
            re,
            replacement: Cow::Borrowed(r.replacement),
        });
    }
    Ok(())
}

/// Compile and append enabled `user_rules` entries at `position` to `rules`,
/// skipping names present in `disabled`.
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
    for ur in user_rules.iter().filter(|ur| ur.position == position) {
        if disabled.contains(&ur.name) {
            continue;
        }
        let re = Regex::new(&ur.pattern)
            .map_err(|err| anyhow!("user rule '{}' has invalid regex: {err}", ur.name))?;
        rules.push(CompiledRule {
            re,
            replacement: Cow::Owned(ur.replacement.clone()),
        });
    }
    Ok(())
}

/// Build the standard cleaner configuration used by CLIs.
///
/// # Arguments
///
/// * `no_cleaning` - Disable cleaning after validating rule names.
/// * `disabled_rules` - Rule names supplied by repeated `--disable-rule`
///   and/or `cleaning.disabled_rules` in `config.toml`.
/// * `user_rules` - User-defined rules loaded from `config.toml`.
///
/// # Returns
///
/// `None` when cleaning is disabled, otherwise a compiled cleaner.
///
/// # Errors
///
/// Returns an error for unknown rule names, invalid rule regexes, a user
/// rule name colliding with a built-in name, or duplicate user rule names.
/// Config-path context, if any, is the caller's responsibility to add.
pub fn build_cleaner(
    no_cleaning: bool,
    disabled_rules: &[String],
    user_rules: &[UserRule],
) -> Result<Option<Cleaner>> {
    for name in disabled_rules {
        assert_rule_name_exists(name, user_rules)?;
    }
    validate_user_rules(user_rules)?;
    if no_cleaning {
        return Ok(None);
    }
    let disabled: HashSet<String> = disabled_rules.iter().cloned().collect();
    Cleaner::new(&disabled, user_rules).map(Some)
}

fn capitalize_sentence_starts(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = true;

    for c in s.chars() {
        if capitalize_next {
            if c.is_alphabetic() {
                out.extend(c.to_uppercase());
                capitalize_next = false;
                continue;
            }
            out.push(c);
            if is_sentence_boundary(c) || c.is_whitespace() || is_opening_punct(c) {
                continue;
            }
            continue;
        }

        out.push(c);
        if is_sentence_boundary(c) {
            capitalize_next = true;
        }
    }

    out
}

fn is_sentence_boundary(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

fn is_opening_punct(c: char) -> bool {
    matches!(c, '"' | '\'' | '(' | '[' | '{' | '<')
}

/// Validate a single rule name exists in `DEFAULT_RULES` or `user_rules`.
/// Used by the CLI to fail fast on `--disable-rule typoname`.
///
/// # Arguments
///
/// * `name` - Rule name to look up, as typed on the CLI.
/// * `user_rules` - User-defined rules that extend the built-in set.
///
/// # Returns
///
/// `Ok(())` when the rule name is present.
///
/// # Errors
///
/// Returns an error describing the unknown name when no matching rule exists.
pub fn assert_rule_name_exists(name: &str, user_rules: &[UserRule]) -> Result<()> {
    if DEFAULT_RULES.iter().any(|r| r.name == name) || user_rules.iter().any(|r| r.name == name) {
        Ok(())
    } else {
        Err(anyhow!(
            "no rule named '{}'. Run with --list-rules to see all rules.",
            name
        ))
    }
}

/// Print all rules to stdout (used by `--list-rules`). User rules, if any,
/// print in a separate section below the built-in rules.
pub fn print_rule_list(user_rules: &[UserRule]) {
    println!("{:<32}  description", "name");
    println!("{}", "-".repeat(80));
    for r in DEFAULT_RULES {
        println!("{:<32}  {}", r.name, r.description);
    }
    if !user_rules.is_empty() {
        println!();
        println!("{:<32}  description (user)", "name");
        println!("{}", "-".repeat(80));
        for ur in user_rules {
            println!(
                "{:<32}  {}",
                ur.name,
                ur.description.as_deref().unwrap_or("")
            );
        }
    }
}

// =============================================================================
// DEFAULT_RULES
// =============================================================================
//
// Order is significant. Categories are grouped for readability; within a
// category, more specific patterns come first.
//
// Conventions:
//   - All rules are case-insensitive ((?i)) unless they specifically need to
//     preserve capitalization.
//   - Leading-position rules use the prefix
//       (^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)
//     which captures: start-of-string OR end-of-sentence whitespace, followed
//     by any opening quotes/brackets. Replacements re-emit those captures.
//   - The final whitespace/punctuation cleanup MUST run last.
// =============================================================================

macro_rules! stutter_rule {
    ($name:literal, $word:literal) => {
        Rule {
            name: concat!("stutter-", $name),
            description: concat!(
                "Collapse repeated '",
                $word,
                " ",
                $word,
                "' → '",
                $word,
                "'"
            ),
            pattern: concat!(r#"(?i)\b("#, $word, r#")(?:\s+"#, $word, r#")+\b"#),
            replacement: "$1",
        }
    };
}

macro_rules! partial_stutter_rule {
    ($name:literal, $label:literal, $prefixes:literal, $target:literal) => {
        Rule {
            name: concat!("partial-stutter-", $name),
            description: concat!("Strip partial-word stutter before '", $label, "'"),
            pattern: concat!(
                r#"(?i)\b(?:"#,
                $prefixes,
                r#")(?:[- ]+(?:"#,
                $prefixes,
                r#")){1,3}[- ]+("#,
                $target,
                r#")\b"#
            ),
            replacement: "$1",
        }
    };
}

/// Built-in transcript cleanup rules in application order.
const DEFAULT_RULES: &[Rule] = &[
    // -------------------------------------------------------------------------
    // Filler words at sentence start ("So, I think..." → "I think...")
    // -------------------------------------------------------------------------
    Rule {
        name: "lead-so-comma",
        description: "Remove leading 'So,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:so,\s+)+"#,
        replacement: "$1$2",
    },
    Rule {
        name: "lead-so-pronoun",
        description: "Remove leading 'So ' before a pronoun/conjunction (no comma)",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)so\s+(i|we|you|he|she|they|it|this|there|then|and|but|the|a|an|my|our|your)\b"#,
        replacement: "$1$2$3",
    },
    Rule {
        name: "lead-well-comma",
        description: "Remove leading 'Well,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:well,\s+)+"#,
        replacement: "$1$2",
    },
    Rule {
        name: "lead-well-pronoun",
        description: "Remove leading 'Well ' before a pronoun/conjunction",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)well\s+(i|we|you|he|she|they|it|this|that|there|then|and|but|maybe|actually)\b"#,
        replacement: "$1$2$3",
    },
    Rule {
        name: "lead-like-comma",
        description: "Remove leading 'Like,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:like,\s+)+"#,
        replacement: "$1$2",
    },
    Rule {
        name: "lead-like-pronoun",
        description: "Remove leading 'Like ' before a pronoun/article (filler use)",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)like\s+(i|we|you|he|she|they|it|this|that|there|the|a|an|my|our|your|some|any)\b"#,
        replacement: "$1$2$3",
    },
    Rule {
        name: "lead-you-know",
        description: "Remove leading 'You know,' / 'You know what I mean,'",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:you know(?: what i mean)?[, ]+)+"#,
        replacement: "$1$2",
    },
    Rule {
        name: "lead-i-mean",
        description: "Remove leading 'I mean,'",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:i mean,\s+)+"#,
        replacement: "$1$2",
    },
    // -------------------------------------------------------------------------
    // Mid-sentence filler (", you know,", ", like,", ", I mean,")
    // -------------------------------------------------------------------------
    Rule {
        name: "mid-not-like-you-know",
        description: "Replace 'not like, you know,' / 'not like, I mean,' with 'not'",
        pattern: r#"(?i)\bnot\s+like\s*,?\s*(?:you know(?: what i mean)?|i mean)\s*,?\s*"#,
        replacement: "not ",
    },
    Rule {
        name: "mid-like-you-know",
        description: "Strip 'like, you know,' / 'like, I mean,' filler phrases",
        pattern: r#"(?i)\blike\s*,?\s*(?:you know(?: what i mean)?|i mean)\s*,?\s*"#,
        replacement: "",
    },
    Rule {
        name: "mid-as-in-like",
        description: "Replace 'as in like X' with 'as in X'",
        pattern: r#"(?i)\bas\s+in\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "as in $1",
    },
    Rule {
        name: "mid-its-actually-like",
        description: "Replace \"it's actually like X\" with \"it's actually X\"",
        pattern: r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2 $3",
    },
    Rule {
        name: "mid-is-actually-like",
        description: "Replace 'is actually like X' with 'is actually X'",
        pattern: r#"(?i)\b(is|was|are|were|am|be|been|being)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2 $3",
    },
    Rule {
        name: "mid-you-know",
        description: "Strip mid-sentence ', you know,' / ', you know what I mean,'",
        pattern: r#"(?i),\s*you know(?: what i mean)?[, ]+"#,
        replacement: " ",
    },
    Rule {
        name: "mid-i-mean",
        description: "Strip mid-sentence ', I mean,'",
        pattern: r#"(?i),\s*i mean,\s*"#,
        replacement: " ",
    },
    Rule {
        name: "mid-like-comma",
        description: "Strip mid-sentence ', like,'",
        pattern: r#"(?i),\s*like,\s*"#,
        replacement: " ",
    },
    Rule {
        name: "mid-i-dont-know",
        description: "Strip mid-sentence ', I don't know,'",
        pattern: r#"(?i),\s*i don'?t know,\s*"#,
        replacement: " ",
    },
    Rule {
        name: "mid-like-noun",
        description: "Replace ', like X' with ' X' when X is a content word",
        pattern: r#"(?i),\s*like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: " $1",
    },
    Rule {
        name: "mid-is-like",
        description: "Replace 'is like X' / 'was like X' (filler) with 'is X'",
        pattern: r#"(?i)\b(is|was|are|were|am|be|been|being)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
    },
    Rule {
        name: "mid-its-like",
        description: "Replace \"it's like X\" / \"that's like X\" with \"it's X\"",
        pattern: r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
    },
    Rule {
        name: "mid-conj-like",
        description: "Replace 'and/but/or/so like X' with 'and/but/or/so X'",
        pattern: r#"(?i)\b(and|but|or|so)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
    },
    // -------------------------------------------------------------------------
    // Filler interjections: um, uh, erm
    // -------------------------------------------------------------------------
    Rule {
        name: "filler-um-uh",
        description: "Remove 'um', 'uh', 'erm' (with surrounding commas/space)",
        pattern: r#"(?i),?\s*\b(?:u[hm]+|er+m*)\b\s*,?"#,
        replacement: " ",
    },
    // -------------------------------------------------------------------------
    // Repeated word stutters ("the the the" → "the", "I I I" → "I")
    // Note: "I" is special-cased to keep the capital.
    // -------------------------------------------------------------------------
    Rule {
        name: "stutter-i",
        description: "Collapse repeated 'I I I' → 'I'",
        pattern: r#"(?i)\b(i)(?:\s+i)+\b"#,
        replacement: "I",
    },
    stutter_rule!("the", "the"),
    stutter_rule!("a", "a"),
    stutter_rule!("an", "an"),
    stutter_rule!("and", "and"),
    stutter_rule!("but", "but"),
    stutter_rule!("or", "or"),
    stutter_rule!("so", "so"),
    stutter_rule!("to", "to"),
    stutter_rule!("of", "of"),
    stutter_rule!("in", "in"),
    stutter_rule!("on", "on"),
    stutter_rule!("we", "we"),
    stutter_rule!("you", "you"),
    stutter_rule!("he", "he"),
    stutter_rule!("she", "she"),
    stutter_rule!("they", "they"),
    stutter_rule!("it", "it"),
    stutter_rule!("this", "this"),
    stutter_rule!("that", "that"),
    stutter_rule!("my", "my"),
    stutter_rule!("is", "is"),
    stutter_rule!("was", "was"),
    stutter_rule!("are", "are"),
    stutter_rule!("were", "were"),
    stutter_rule!("do", "do"),
    stutter_rule!("did", "did"),
    stutter_rule!("can", "can"),
    stutter_rule!("will", "will"),
    stutter_rule!("has", "has"),
    stutter_rule!("had", "had"),
    stutter_rule!("no", "no"),
    stutter_rule!("have", "have"),
    // -------------------------------------------------------------------------
    // Single-letter / partial-word stutters ("sh sh sh should" → "should")
    // These are common in real speech and ASR captures them literally.
    // -------------------------------------------------------------------------
    Rule {
        name: "single-letter-stutter",
        description: "Strip repeated single-letter starts before a word beginning with that letter",
        pattern: concat!(
            r#"(?i)\b(?:t\s+){2,}(t\w*)\b"#,
            r#"|(?:s\s+){2,}(s\w*)\b"#,
            r#"|(?:f\s+){2,}(f\w*)\b"#,
            r#"|(?:w\s+){2,}(w\w*)\b"#,
            r#"|(?:i\s+){2,}(i\w*)\b"#,
            r#"|(?:a\s+){2,}(a\w*)\b"#,
            r#"|(?:p\s+){2,}(p\w*)\b"#,
            r#"|(?:m\s+){2,}(m\w*)\b"#,
            r#"|(?:c\s+){2,}(c\w*)\b"#,
            r#"|(?:d\s+){2,}(d\w*)\b"#,
            r#"|(?:b\s+){2,}(b\w*)\b"#,
            r#"|(?:h\s+){2,}(h\w*)\b"#,
            r#"|(?:n\s+){2,}(n\w*)\b"#,
            r#"|(?:o\s+){2,}(o\w*)\b"#,
            r#"|(?:l\s+){2,}(l\w*)\b"#,
            r#"|(?:r\s+){2,}(r\w*)\b"#,
            r#"|(?:g\s+){2,}(g\w*)\b"#,
            r#"|(?:y\s+){2,}(y\w*)\b"#,
        ),
        replacement: "$1$2$3$4$5$6$7$8$9$10$11$12$13$14$15$16$17$18",
    },
    partial_stutter_rule!("should", "should", "s|so|sh|sho", "should"),
    partial_stutter_rule!(
        "think",
        "think/thinking/this/that",
        "t|th|thi",
        "think(?:ing)?|thing|this|that|these|those"
    ),
    partial_stutter_rule!("because", "because", "b|be|bec", "because"),
    partial_stutter_rule!("definitely", "definitely", "d|de|def", "definitely"),
    partial_stutter_rule!("make", "make", "m|ma|mak", "make"),
    partial_stutter_rule!("sure", "sure", "s|su|sur", "sure"),
    partial_stutter_rule!(
        "change",
        "change/changing/changed",
        "c|ch",
        "chang(?:e|ed|es|ing)"
    ),
    // -------------------------------------------------------------------------
    // Casual contractions
    // -------------------------------------------------------------------------
    Rule {
        name: "cause-to-because-mid",
        description: "Mid-sentence 'cause → because (after a word)",
        pattern: r#"(?i)([A-Za-z])['\u{2019}]cause\b"#,
        replacement: "$1 because",
    },
    Rule {
        name: "cause-to-because-bare",
        description: "Bare 'cause anywhere → because",
        pattern: r#"(?i)['\u{2019}]cause\b"#,
        replacement: "because",
    },
    Rule {
        name: "casual-em-til-round",
        description: "'em / 'til / 'round / 'bout etc. — keep contraction with proper apostrophe",
        pattern: r#"(?i)([A-Za-z])['\u{2019}](em|til|round|bout|cept|nother)\b"#,
        replacement: "$1 '$2",
    },
    // -------------------------------------------------------------------------
    // Final whitespace and punctuation cleanup. MUST run last.
    // -------------------------------------------------------------------------
    Rule {
        name: "fix-space-before-punct",
        description: "Remove space before punctuation (',' '.' ';' ':' '!' '?')",
        pattern: r#"\s+([,.;:!?])"#,
        replacement: "$1",
    },
    Rule {
        name: "fix-collapse-spaces",
        description: "Collapse runs of spaces/tabs into one space",
        pattern: r#"[ \t]{2,}"#,
        replacement: " ",
    },
    Rule {
        name: "fix-trim",
        description: "Trim leading and trailing whitespace",
        pattern: r#"^\s+|\s+$"#,
        replacement: "",
    },
    Rule {
        name: "fix-leading-comma",
        description: "Remove a comma left at the very start of the output",
        pattern: r#"^\s*,\s*"#,
        replacement: "",
    },
    Rule {
        name: "fix-trailing-period",
        description: "Drop a single trailing period for dictation-friendly short utterances",
        pattern: r#"\.$"#,
        replacement: "",
    },
];

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaner_with_all_defaults() -> Cleaner {
        Cleaner::new(&HashSet::new(), &[]).expect("default rules must compile")
    }

    fn user_rule(name: &str, pattern: &str, replacement: &str, position: RulePosition) -> UserRule {
        UserRule {
            name: name.to_string(),
            description: None,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            position,
        }
    }

    fn assert_clean_cases(cases: &[(&str, &str)]) {
        let c = cleaner_with_all_defaults();
        for (input, expected) in cases {
            assert_eq!(c.clean(input), *expected, "input: {input}");
        }
    }

    #[test]
    fn lead_so_removed() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("So, I think this works."), "I think this works");
        assert_eq!(c.clean("So I think this works."), "I think this works");
    }

    #[test]
    fn um_uh_removed() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("I, um, think this works"), "I think this works");
        assert_eq!(c.clean("uh, hello there"), "Hello there");
    }

    #[test]
    fn repeated_words_collapsed() {
        assert_clean_cases(&[
            ("the the the cat", "The cat"),
            ("I I I think", "I think"),
            ("we we ran", "We ran"),
            ("did did happen", "Did happen"),
            ("no no no problem", "No problem"),
            ("has has changed", "Has changed"),
        ]);
    }

    #[test]
    fn partial_stutter_should() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("we sh sh should go"), "We should go");
        assert_eq!(c.clean("we sh-sh-should go"), "We should go");
    }

    #[test]
    fn cause_becomes_because() {
        assert_clean_cases(&[
            ("'cause it was late", "Because it was late"),
            ("that's'cause it works", "That's because it works"),
            ("I left 'cause it was late", "I left because it was late"),
        ]);
    }

    #[test]
    fn whitespace_cleanup() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("hello   world ,  foo ."), "Hello world, foo");
    }

    #[test]
    fn disabled_rules_skip() {
        let mut disabled = HashSet::new();
        disabled.insert("filler-um-uh".to_string());
        let c = Cleaner::new(&disabled, &[]).unwrap();
        // um/uh should survive
        assert_eq!(c.clean("hello, um, world"), "Hello, um, world");
    }

    #[test]
    fn user_rule_first_position_runs_before_builtins() {
        // A `First` user rule expands "xyz" to "So, hello" *before* any
        // built-in rule runs, so the built-in `lead-so-comma` rule (which
        // only fires at sentence start) still strips the "So," it produced.
        let rules = vec![user_rule(
            "expand-xyz",
            r"(?i)^xyz$",
            "So, hello",
            RulePosition::First,
        )];
        let c = Cleaner::new(&HashSet::new(), &rules).unwrap();
        assert_eq!(c.clean("xyz"), "Hello");
    }

    #[test]
    fn user_rule_standard_position_runs_before_cleanup_group() {
        // A `Standard` user rule introduces messy whitespace and a stray
        // space before a comma; it must run before the built-in
        // `fix-collapse-spaces` / `fix-space-before-punct` cleanup rules for
        // the output to come out clean.
        let rules = vec![user_rule(
            "expand-brb",
            r"(?i)\bbrb\b",
            "be right   back ,",
            RulePosition::Standard,
        )];
        let c = Cleaner::new(&HashSet::new(), &rules).unwrap();
        assert_eq!(c.clean("brb"), "Be right back,");
    }

    #[test]
    fn user_rule_last_position_runs_after_builtins() {
        // A `Last` user rule appends a trailing period *after* the built-in
        // `fix-trailing-period` rule has already run, so the period this
        // rule adds is not stripped.
        let rules = vec![user_rule(
            "add-trailing-period",
            r"(?i)^done$",
            "done.",
            RulePosition::Last,
        )];
        let c = Cleaner::new(&HashSet::new(), &rules).unwrap();
        assert_eq!(c.clean("done"), "Done.");
    }

    #[test]
    fn user_rule_name_colliding_with_builtin_is_an_error() {
        let rules = vec![user_rule(
            "filler-um-uh",
            r"(?i)nope",
            "x",
            RulePosition::Standard,
        )];
        let err = Cleaner::new(&HashSet::new(), &rules).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("filler-um-uh"), "message: {msg}");
        assert!(msg.contains("rename"), "message: {msg}");
    }

    #[test]
    fn duplicate_user_rule_names_are_an_error() {
        let rules = vec![
            user_rule("custom-a", r"(?i)a", "A", RulePosition::Standard),
            user_rule("custom-a", r"(?i)b", "B", RulePosition::Standard),
        ];
        let err = Cleaner::new(&HashSet::new(), &rules).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "message: {msg}");
        assert!(msg.contains("custom-a"), "message: {msg}");
    }

    #[test]
    fn invalid_user_rule_regex_names_the_rule() {
        let rules = vec![user_rule(
            "bad-regex",
            "(unclosed",
            "x",
            RulePosition::Standard,
        )];
        let err = Cleaner::new(&HashSet::new(), &rules).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("user rule 'bad-regex' has invalid regex"),
            "message: {msg}"
        );
    }

    #[test]
    fn disabled_user_rule_skips_compilation_and_application() {
        let mut disabled = HashSet::new();
        disabled.insert("custom-hello".to_string());
        let rules = vec![user_rule(
            "custom-hello",
            r"(?i)hello",
            "HI",
            RulePosition::Standard,
        )];
        let c = Cleaner::new(&disabled, &rules).unwrap();
        assert_eq!(c.clean("hello world"), "Hello world");
    }

    #[test]
    fn sentence_starts_are_capitalized_after_cleaning() {
        let c = cleaner_with_all_defaults();
        assert_eq!(
            c.clean("So, the cat ran. then it slept. \"then it woke.\""),
            "The cat ran. Then it slept. \"Then it woke.\""
        );
    }

    #[test]
    fn capitalization_is_idempotent_for_existing_sentence_case() {
        let c = cleaner_with_all_defaults();
        let input = "Already capitalized. Still capitalized!";
        assert_eq!(c.clean(input), input);
    }

    #[test]
    fn capitalization_handles_leading_whitespace() {
        assert_eq!(
            capitalize_sentence_starts("   hello there"),
            "   Hello there"
        );
    }

    #[test]
    fn single_letter_stutter() {
        assert_clean_cases(&[
            ("t t t think", "Think"),
            ("I w w w want this", "I want this"),
            ("s s s sure", "Sure"),
        ]);
    }

    #[test]
    fn like_filler_combos() {
        assert_clean_cases(&[
            ("it's like, you know, hard", "It's hard"),
            ("not like, you know,", "Not"),
            ("as in like tuple", "As in tuple"),
            ("it's actually like hard", "It's actually hard"),
            ("is basically like broken", "Is basically broken"),
        ]);
    }

    #[test]
    fn assert_rule_name_exists_works() {
        assert!(assert_rule_name_exists("filler-um-uh", &[]).is_ok());
        assert!(assert_rule_name_exists("does-not-exist", &[]).is_err());
        let rules = vec![user_rule(
            "custom-hello",
            r"(?i)hello",
            "HI",
            RulePosition::Standard,
        )];
        assert!(assert_rule_name_exists("custom-hello", &rules).is_ok());
    }
}
