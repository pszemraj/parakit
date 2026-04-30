//! Text-cleaning rules applied to raw ASR output before insertion.
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
    ///
    /// # Returns
    ///
    /// A cleaner containing the default-enabled rules that were not disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if any enabled rule contains an invalid regex pattern.
    pub fn new(disabled: &HashSet<String>) -> Result<Self> {
        let mut rules = Vec::with_capacity(DEFAULT_RULES.len());
        for r in DEFAULT_RULES {
            if disabled.contains(r.name) {
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
    ///
    /// # Returns
    ///
    /// The cleaned transcript.
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

    /// Number of active rules after disable filtering.
    ///
    /// # Returns
    ///
    /// The number of active cleanup rules.
    pub fn active_rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// Build the standard cleaner configuration used by CLIs.
///
/// # Arguments
///
/// * `no_cleaning` - Disable cleaning after validating rule names.
/// * `disabled_rules` - Rule names supplied by repeated `--disable-rule`.
///
/// # Returns
///
/// `None` when cleaning is disabled, otherwise a compiled cleaner.
///
/// # Errors
///
/// Returns an error for unknown rule names or invalid rule regexes.
pub fn build_cleaner(no_cleaning: bool, disabled_rules: &[String]) -> Result<Option<Cleaner>> {
    for name in disabled_rules {
        assert_rule_name_exists(name)?;
    }
    if no_cleaning {
        return Ok(None);
    }
    let disabled: HashSet<String> = disabled_rules.iter().cloned().collect();
    Cleaner::new(&disabled).map(Some)
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
///
/// # Returns
///
/// `Ok(())` when the rule name is present.
///
/// # Errors
///
/// Returns an error describing the unknown name when no matching rule exists.
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
    println!("{:<32}  description", "name");
    println!("{}", "-".repeat(80));
    for r in DEFAULT_RULES {
        println!("{:<32}  {}", r.name, r.description);
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

/// Built-in transcript cleanup rules in application order.
pub const DEFAULT_RULES: &[Rule] = &[
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
    Rule {
        name: "partial-stutter-should",
        description: "Strip partial-word stutter before 'should'",
        pattern: r#"(?i)\b(?:s|so|sh|sho)(?:[- ]+(?:s|so|sh|sho)){1,3}[- ]+(should)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-think",
        description: "Strip partial-word stutter before 'think/thinking/this/that'",
        pattern: r#"(?i)\b(?:t|th|thi)(?:[- ]+(?:t|th|thi)){1,3}[- ]+(think(?:ing)?|thing|this|that|these|those)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-because",
        description: "Strip partial-word stutter before 'because'",
        pattern: r#"(?i)\b(?:b|be|bec)(?:[- ]+(?:b|be|bec)){1,3}[- ]+(because)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-definitely",
        description: "Strip partial-word stutter before 'definitely'",
        pattern: r#"(?i)\b(?:d|de|def)(?:[- ]+(?:d|de|def)){1,3}[- ]+(definitely)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-make",
        description: "Strip partial-word stutter before 'make'",
        pattern: r#"(?i)\b(?:m|ma|mak)(?:[- ]+(?:m|ma|mak)){1,3}[- ]+(make)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-sure",
        description: "Strip partial-word stutter before 'sure'",
        pattern: r#"(?i)\b(?:s|su|sur)(?:[- ]+(?:s|su|sur)){1,3}[- ]+(sure)\b"#,
        replacement: "$1",
    },
    Rule {
        name: "partial-stutter-change",
        description: "Strip partial-word stutter before 'change/changing/changed'",
        pattern: r#"(?i)\b(?:c|ch)(?:[- ]+(?:c|ch)){1,3}[- ]+(chang(?:e|ed|es|ing))\b"#,
        replacement: "$1",
    },
    // -------------------------------------------------------------------------
    // Casual contractions
    // -------------------------------------------------------------------------
    Rule {
        name: "cause-to-because-lead",
        description: "'cause at sentence start → 'Because'",
        pattern: r#"(?i)(^|[.!?\n]\s*)((?:["(\[]\s*)*)['\u{2019}]cause\b"#,
        replacement: "${1}${2}Because",
    },
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
        Cleaner::new(&HashSet::new()).expect("default rules must compile")
    }

    fn assert_clean_cases(cases: &[(&str, &str)]) {
        let c = cleaner_with_all_defaults();
        for (input, expected) in cases {
            assert_eq!(c.clean(input), *expected, "input: {input}");
        }
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
        assert!(assert_rule_name_exists("filler-um-uh").is_ok());
        assert!(assert_rule_name_exists("does-not-exist").is_err());
    }
}
