//! Text-cleaning rules applied to raw ASR output before injection.
//!
//! Rules are baked into this file, not loaded from a config. Adding or editing
//! a rule = editing this file. This is intentional: it makes the rule set easy
//! for an LM to maintain and produces a single self-contained binary.
//!
//! ## How rules are applied
//!
//! Rules apply in the order they appear in [`DEFAULT_RULES`]. Each rule's
//! output is the next rule's input. Order matters — for example, leading
//! filler removal ("So, ") happens before whitespace collapse, otherwise the
//! comma-space leftovers wouldn't get cleaned.
//!
//! ## How to add a rule
//!
//! Append a [`Rule`] entry to [`DEFAULT_RULES`]. Use Rust regex syntax
//! (effectively PCRE-lite, no lookbehind). Use `(?i)` for case-insensitive,
//! `\b` for word boundaries, `$1` etc. for capture references.
//!
//! ```text
//! Rule {
//!     name: "my-rule",
//!     description: "What this rule does",
//!     pattern: r"(?i)\bmyword\b",
//!     replacement: "yourword",
//!     default_enabled: true,
//! },
//! ```
//!
//! ## How to disable a rule at runtime
//!
//! Pass `--disable-rule <name>` (repeatable) on the CLI, or `--no-cleaning`
//! to skip everything.

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::collections::HashSet;

/// A single text-cleaning rule.
pub struct Rule {
    pub name: &'static str,
    pub description: &'static str,
    pub pattern: &'static str,
    pub replacement: &'static str,
    pub default_enabled: bool,
}

/// Compiled at startup.
struct CompiledRule {
    re: Regex,
    replacement: &'static str,
}

/// Driver. Build once, call `clean()` per transcription.
pub struct Cleaner {
    rules: Vec<CompiledRule>,
}

impl Cleaner {
    /// Compile all default rules whose name is not in `disabled`.
    pub fn new(disabled: &HashSet<String>) -> Result<Self> {
        let mut rules = Vec::with_capacity(DEFAULT_RULES.len());
        for r in DEFAULT_RULES {
            if !r.default_enabled || disabled.contains(r.name) {
                continue;
            }
            let re = Regex::new(r.pattern)
                .with_context(|| format!("rule '{}' has invalid regex", r.name))?;
            rules.push(CompiledRule {
                re,
                replacement: r.replacement,
            });
        }
        Ok(Self { rules })
    }

    /// Apply all enabled rules in order. Idempotent for stable input.
    pub fn clean(&self, input: &str) -> String {
        let mut s = input.to_string();
        for r in &self.rules {
            let replaced = r.re.replace_all(&s, r.replacement);
            if matches!(replaced, std::borrow::Cow::Owned(_)) {
                s = replaced.into_owned();
            }
        }
        capitalize_sentence_starts(&s)
    }

    /// Number of active rules (after disable filtering).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
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

/// Validate a single rule name exists in `DEFAULT_RULES`. Used by the CLI to
/// fail fast on `--disable-rule typoname`.
pub fn assert_rule_name_exists(name: &str) -> Result<()> {
    if DEFAULT_RULES.iter().any(|r| r.name == name) {
        Ok(())
    } else {
        Err(anyhow!(
            "no rule named '{}'. Run with --list-rules to see all rules.",
            name
        ))
    }
}

/// Print all rules to stdout (used by `--list-rules`).
pub fn print_rule_list() {
    println!("{:<32}  {:<8}  description", "name", "default");
    println!("{}", "-".repeat(80));
    for r in DEFAULT_RULES {
        println!(
            "{:<32}  {:<8}  {}",
            r.name,
            if r.default_enabled { "on" } else { "off" },
            r.description
        );
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

pub const DEFAULT_RULES: &[Rule] = &[
    // -------------------------------------------------------------------------
    // Filler words at sentence start ("So, I think..." → "I think...")
    // -------------------------------------------------------------------------
    Rule {
        name: "lead-so-comma",
        description: "Remove leading 'So,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:so,\s+)+"#,
        replacement: "$1$2",
        default_enabled: true,
    },
    Rule {
        name: "lead-so-pronoun",
        description: "Remove leading 'So ' before a pronoun/conjunction (no comma)",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)so\s+(i|we|you|he|she|they|it|this|there|then|and|but|the|a|an|my|our|your)\b"#,
        replacement: "$1$2$3",
        default_enabled: true,
    },
    Rule {
        name: "lead-well-comma",
        description: "Remove leading 'Well,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:well,\s+)+"#,
        replacement: "$1$2",
        default_enabled: true,
    },
    Rule {
        name: "lead-well-pronoun",
        description: "Remove leading 'Well ' before a pronoun/conjunction",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)well\s+(i|we|you|he|she|they|it|this|that|there|then|and|but|maybe|actually)\b"#,
        replacement: "$1$2$3",
        default_enabled: true,
    },
    Rule {
        name: "lead-like-comma",
        description: "Remove leading 'Like,' at sentence start",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:like,\s+)+"#,
        replacement: "$1$2",
        default_enabled: true,
    },
    Rule {
        name: "lead-like-pronoun",
        description: "Remove leading 'Like ' before a pronoun/article (filler use)",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)like\s+(i|we|you|he|she|they|it|this|that|there|the|a|an|my|our|your|some|any)\b"#,
        replacement: "$1$2$3",
        default_enabled: true,
    },
    Rule {
        name: "lead-you-know",
        description: "Remove leading 'You know,' / 'You know what I mean,'",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:you know(?: what i mean)?[, ]+)+"#,
        replacement: "$1$2",
        default_enabled: true,
    },
    Rule {
        name: "lead-i-mean",
        description: "Remove leading 'I mean,'",
        pattern: r#"(?i)(^\s*|[.!?\n]\s*)((?:["'(\[]\s*)*)(?:i mean,\s+)+"#,
        replacement: "$1$2",
        default_enabled: true,
    },
    // -------------------------------------------------------------------------
    // Mid-sentence filler (", you know,", ", like,", ", I mean,")
    // -------------------------------------------------------------------------
    Rule {
        name: "mid-not-like-you-know",
        description: "Replace 'not like, you know,' / 'not like, I mean,' with 'not'",
        pattern: r#"(?i)\bnot\s+like\s*,?\s*(?:you know(?: what i mean)?|i mean)\s*,?\s*"#,
        replacement: "not ",
        default_enabled: true,
    },
    Rule {
        name: "mid-like-you-know",
        description: "Strip 'like, you know,' / 'like, I mean,' filler phrases",
        pattern: r#"(?i)\blike\s*,?\s*(?:you know(?: what i mean)?|i mean)\s*,?\s*"#,
        replacement: "",
        default_enabled: true,
    },
    Rule {
        name: "mid-as-in-like",
        description: "Replace 'as in like X' with 'as in X'",
        pattern: r#"(?i)\bas\s+in\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "as in $1",
        default_enabled: true,
    },
    Rule {
        name: "mid-its-actually-like",
        description: "Replace \"it's actually like X\" with \"it's actually X\"",
        pattern: r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2 $3",
        default_enabled: true,
    },
    Rule {
        name: "mid-is-actually-like",
        description: "Replace 'is actually like X' with 'is actually X'",
        pattern: r#"(?i)\b(is|was|are|were|am|be|been|being)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2 $3",
        default_enabled: true,
    },
    Rule {
        name: "mid-you-know",
        description: "Strip mid-sentence ', you know,' / ', you know what I mean,'",
        pattern: r#"(?i),\s*you know(?: what i mean)?[, ]+"#,
        replacement: " ",
        default_enabled: true,
    },
    Rule {
        name: "mid-i-mean",
        description: "Strip mid-sentence ', I mean,'",
        pattern: r#"(?i),\s*i mean,\s*"#,
        replacement: " ",
        default_enabled: true,
    },
    Rule {
        name: "mid-like-comma",
        description: "Strip mid-sentence ', like,'",
        pattern: r#"(?i),\s*like,\s*"#,
        replacement: " ",
        default_enabled: true,
    },
    Rule {
        name: "mid-i-dont-know",
        description: "Strip mid-sentence ', I don't know,'",
        pattern: r#"(?i),\s*i don'?t know,\s*"#,
        replacement: " ",
        default_enabled: true,
    },
    Rule {
        name: "mid-like-noun",
        description: "Replace ', like X' with ' X' when X is a content word",
        pattern: r#"(?i),\s*like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: " $1",
        default_enabled: true,
    },
    Rule {
        name: "mid-is-like",
        description: "Replace 'is like X' / 'was like X' (filler) with 'is X'",
        pattern: r#"(?i)\b(is|was|are|were|am|be|been|being)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
        default_enabled: true,
    },
    Rule {
        name: "mid-its-like",
        description: "Replace \"it's like X\" / \"that's like X\" with \"it's X\"",
        pattern: r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
        default_enabled: true,
    },
    Rule {
        name: "mid-conj-like",
        description: "Replace 'and/but/or/so like X' with 'and/but/or/so X'",
        pattern: r#"(?i)\b(and|but|or|so)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        replacement: "$1 $2",
        default_enabled: true,
    },
    // -------------------------------------------------------------------------
    // Filler interjections: um, uh, erm
    // -------------------------------------------------------------------------
    Rule {
        name: "filler-um-uh",
        description: "Remove 'um', 'uh', 'erm' (with surrounding commas/space)",
        pattern: r#"(?i),?\s*\b(?:u[hm]+|er+m*)\b\s*,?"#,
        replacement: " ",
        default_enabled: true,
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
        default_enabled: true,
    },
    Rule {
        name: "stutter-the",
        description: "Collapse repeated 'the the' → 'the'",
        pattern: r#"(?i)\b(the)(?:\s+the)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-a",
        description: "Collapse repeated 'a a' → 'a'",
        pattern: r#"(?i)\b(a)(?:\s+a)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-an",
        description: "Collapse repeated 'an an' → 'an'",
        pattern: r#"(?i)\b(an)(?:\s+an)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-and",
        description: "Collapse repeated 'and and' → 'and'",
        pattern: r#"(?i)\b(and)(?:\s+and)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-but",
        description: "Collapse repeated 'but but' → 'but'",
        pattern: r#"(?i)\b(but)(?:\s+but)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-or",
        description: "Collapse repeated 'or or' → 'or'",
        pattern: r#"(?i)\b(or)(?:\s+or)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-so",
        description: "Collapse repeated 'so so' → 'so'",
        pattern: r#"(?i)\b(so)(?:\s+so)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-to",
        description: "Collapse repeated 'to to' → 'to'",
        pattern: r#"(?i)\b(to)(?:\s+to)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-of",
        description: "Collapse repeated 'of of' → 'of'",
        pattern: r#"(?i)\b(of)(?:\s+of)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-in",
        description: "Collapse repeated 'in in' → 'in'",
        pattern: r#"(?i)\b(in)(?:\s+in)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-on",
        description: "Collapse repeated 'on on' → 'on'",
        pattern: r#"(?i)\b(on)(?:\s+on)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-we",
        description: "Collapse repeated 'we we' → 'we'",
        pattern: r#"(?i)\b(we)(?:\s+we)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-you",
        description: "Collapse repeated 'you you' → 'you'",
        pattern: r#"(?i)\b(you)(?:\s+you)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-he",
        description: "Collapse repeated 'he he' → 'he'",
        pattern: r#"(?i)\b(he)(?:\s+he)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-she",
        description: "Collapse repeated 'she she' → 'she'",
        pattern: r#"(?i)\b(she)(?:\s+she)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-they",
        description: "Collapse repeated 'they they' → 'they'",
        pattern: r#"(?i)\b(they)(?:\s+they)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-it",
        description: "Collapse repeated 'it it' → 'it'",
        pattern: r#"(?i)\b(it)(?:\s+it)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-this",
        description: "Collapse repeated 'this this' → 'this'",
        pattern: r#"(?i)\b(this)(?:\s+this)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-that",
        description: "Collapse repeated 'that that' → 'that'",
        pattern: r#"(?i)\b(that)(?:\s+that)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-my",
        description: "Collapse repeated 'my my' → 'my'",
        pattern: r#"(?i)\b(my)(?:\s+my)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-is",
        description: "Collapse repeated 'is is' → 'is'",
        pattern: r#"(?i)\b(is)(?:\s+is)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-was",
        description: "Collapse repeated 'was was' → 'was'",
        pattern: r#"(?i)\b(was)(?:\s+was)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-are",
        description: "Collapse repeated 'are are' → 'are'",
        pattern: r#"(?i)\b(are)(?:\s+are)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-were",
        description: "Collapse repeated 'were were' → 'were'",
        pattern: r#"(?i)\b(were)(?:\s+were)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-do",
        description: "Collapse repeated 'do do' → 'do'",
        pattern: r#"(?i)\b(do)(?:\s+do)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-did",
        description: "Collapse repeated 'did did' → 'did'",
        pattern: r#"(?i)\b(did)(?:\s+did)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-can",
        description: "Collapse repeated 'can can' → 'can'",
        pattern: r#"(?i)\b(can)(?:\s+can)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-will",
        description: "Collapse repeated 'will will' → 'will'",
        pattern: r#"(?i)\b(will)(?:\s+will)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-has",
        description: "Collapse repeated 'has has' → 'has'",
        pattern: r#"(?i)\b(has)(?:\s+has)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-had",
        description: "Collapse repeated 'had had' → 'had'",
        pattern: r#"(?i)\b(had)(?:\s+had)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-no",
        description: "Collapse repeated 'no no' → 'no'",
        pattern: r#"(?i)\b(no)(?:\s+no)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "stutter-have",
        description: "Collapse repeated 'have have' → 'have'",
        pattern: r#"(?i)\b(have)(?:\s+have)+\b"#,
        replacement: "$1",
        default_enabled: true,
    },
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
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-should",
        description: "Strip partial-word stutter before 'should'",
        pattern: r#"(?i)\b(?:s|so|sh|sho)(?:[- ]+(?:s|so|sh|sho)){1,3}[- ]+(should)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-think",
        description: "Strip partial-word stutter before 'think/thinking/this/that'",
        pattern: r#"(?i)\b(?:t|th|thi)(?:[- ]+(?:t|th|thi)){1,3}[- ]+(think(?:ing)?|thing|this|that|these|those)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-because",
        description: "Strip partial-word stutter before 'because'",
        pattern: r#"(?i)\b(?:b|be|bec)(?:[- ]+(?:b|be|bec)){1,3}[- ]+(because)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-definitely",
        description: "Strip partial-word stutter before 'definitely'",
        pattern: r#"(?i)\b(?:d|de|def)(?:[- ]+(?:d|de|def)){1,3}[- ]+(definitely)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-make",
        description: "Strip partial-word stutter before 'make'",
        pattern: r#"(?i)\b(?:m|ma|mak)(?:[- ]+(?:m|ma|mak)){1,3}[- ]+(make)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-sure",
        description: "Strip partial-word stutter before 'sure'",
        pattern: r#"(?i)\b(?:s|su|sur)(?:[- ]+(?:s|su|sur)){1,3}[- ]+(sure)\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "partial-stutter-change",
        description: "Strip partial-word stutter before 'change/changing/changed'",
        pattern: r#"(?i)\b(?:c|ch)(?:[- ]+(?:c|ch)){1,3}[- ]+(chang(?:e|ed|es|ing))\b"#,
        replacement: "$1",
        default_enabled: true,
    },
    // -------------------------------------------------------------------------
    // Casual contractions
    // -------------------------------------------------------------------------
    Rule {
        name: "cause-to-because-lead",
        description: "'cause at sentence start → 'Because'",
        pattern: r#"(?i)(^|[.!?\n]\s*)((?:["(\[]\s*)*)['\u{2019}]cause\b"#,
        replacement: "${1}${2}Because",
        default_enabled: true,
    },
    Rule {
        name: "cause-to-because-mid",
        description: "Mid-sentence 'cause → because (after a word)",
        pattern: r#"(?i)([A-Za-z])['\u{2019}]cause\b"#,
        replacement: "$1 because",
        default_enabled: true,
    },
    Rule {
        name: "cause-to-because-bare",
        description: "Bare 'cause anywhere → because",
        pattern: r#"(?i)['\u{2019}]cause\b"#,
        replacement: "because",
        default_enabled: true,
    },
    Rule {
        name: "casual-em-til-round",
        description: "'em / 'til / 'round / 'bout etc. — keep contraction with proper apostrophe",
        pattern: r#"(?i)([A-Za-z])['\u{2019}](em|til|round|bout|cept|nother)\b"#,
        replacement: "$1 '$2",
        default_enabled: true,
    },
    // -------------------------------------------------------------------------
    // Final whitespace and punctuation cleanup. MUST run last.
    // -------------------------------------------------------------------------
    Rule {
        name: "fix-space-before-punct",
        description: "Remove space before punctuation (',' '.' ';' ':' '!' '?')",
        pattern: r#"\s+([,.;:!?])"#,
        replacement: "$1",
        default_enabled: true,
    },
    Rule {
        name: "fix-collapse-spaces",
        description: "Collapse runs of spaces/tabs into one space",
        pattern: r#"[ \t]{2,}"#,
        replacement: " ",
        default_enabled: true,
    },
    Rule {
        name: "fix-trim",
        description: "Trim leading and trailing whitespace",
        pattern: r#"^\s+|\s+$"#,
        replacement: "",
        default_enabled: true,
    },
    Rule {
        name: "fix-leading-comma",
        description: "Remove a comma left at the very start of the output",
        pattern: r#"^\s*,\s*"#,
        replacement: "",
        default_enabled: true,
    },
    Rule {
        name: "fix-trailing-period",
        description: "Drop a single trailing period for dictation-friendly short utterances",
        pattern: r#"\.$"#,
        replacement: "",
        default_enabled: true,
    },
];

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaner_with_all_defaults() -> Cleaner {
        Cleaner::new(&HashSet::new()).expect("default rules must compile")
    }

    #[test]
    fn all_default_rules_compile() {
        cleaner_with_all_defaults();
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
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("the the the cat"), "The cat");
        assert_eq!(c.clean("I I I think"), "I think");
        assert_eq!(c.clean("we we ran"), "We ran");
    }

    #[test]
    fn partial_stutter_should() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("we sh sh should go"), "We should go");
        assert_eq!(c.clean("we sh-sh-should go"), "We should go");
    }

    #[test]
    fn cause_becomes_because() {
        let c = cleaner_with_all_defaults();
        assert_eq!(
            c.clean("I left 'cause it was late"),
            "I left because it was late"
        );
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
        let c = Cleaner::new(&disabled).unwrap();
        // um/uh should survive
        assert_eq!(c.clean("hello, um, world"), "Hello, um, world");
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
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("t t t think"), "Think");
        assert_eq!(c.clean("I w w w want this"), "I want this");
        assert_eq!(c.clean("s s s sure"), "Sure");
    }

    #[test]
    fn like_filler_combos() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("it's like, you know, hard"), "It's hard");
        assert_eq!(c.clean("not like, you know,"), "Not");
        assert_eq!(c.clean("as in like tuple"), "As in tuple");
        assert_eq!(c.clean("it's actually like hard"), "It's actually hard");
        assert_eq!(c.clean("is basically like broken"), "Is basically broken");
    }

    #[test]
    fn additional_stutters() {
        let c = cleaner_with_all_defaults();
        assert_eq!(c.clean("did did happen"), "Did happen");
        assert_eq!(c.clean("no no no problem"), "No problem");
        assert_eq!(c.clean("has has changed"), "Has changed");
    }

    #[test]
    fn assert_rule_name_exists_works() {
        assert!(assert_rule_name_exists("filler-um-uh").is_ok());
        assert!(assert_rule_name_exists("does-not-exist").is_err());
    }
}
