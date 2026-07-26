//! Built-in transcript-cleaning rule table.
//!
//! Order is significant and MUST NOT be casually reshuffled: aggressive
//! discourse edits run first so the final mechanical passes can repair
//! punctuation and spacing left behind, and duplicate/stutter rules MUST
//! stay before `spaced-acronyms` so `I I think` becomes `I think`, not
//! `II think`.
//!
//! ## How to add a built-in rule
//!
//! Append a `Rule` entry to [`DEFAULT_RULES`] using one of the
//! `regex_rule!`/`fancy_rule!`/`procedural_rule!` macros below. Use Rust
//! `regex` crate syntax (effectively PCRE-lite, no lookbehind/lookahead)
//! unless the rule specifically needs `fancy-regex` backreferences, in which
//! case use `fancy_rule!` and keep the pattern bounded (see
//! `crate::rules::engine::FANCY_BACKTRACK_LIMIT`).

use super::engine::{Activation, Rule, RuleKind};
use super::passes::{
    capitalize_sentence_starts, normalize_magnitude_suffixes, normalize_numeric_identifier_groups,
    normalize_numeric_point_suffixes, normalize_spaced_acronyms, normalize_spoken_numbers,
    normalize_spoken_versions,
};

macro_rules! regex_rule {
    ($name:expr, $description:expr, $activation:expr, $pattern:expr, $replacement:expr) => {
        Rule {
            name: $name,
            description: $description,
            activation: $activation,
            kind: RuleKind::Regex {
                pattern: $pattern,
                replacement: $replacement,
            },
        }
    };
}

macro_rules! fancy_rule {
    ($name:literal, $description:literal, $activation:expr, $pattern:literal, $replacement:literal) => {
        Rule {
            name: $name,
            description: $description,
            activation: $activation,
            kind: RuleKind::FancyRegex {
                pattern: $pattern,
                replacement: $replacement,
            },
        }
    };
}

macro_rules! procedural_rule {
    ($name:literal, $description:literal, $activation:expr, $transform:path) => {
        Rule {
            name: $name,
            description: $description,
            activation: $activation,
            kind: RuleKind::Procedural($transform),
        }
    };
}

macro_rules! partial_stutter_rule {
    ($name:literal, $label:literal, $prefixes:literal, $target:literal) => {
        regex_rule!(
            concat!("partial-stutter-", $name),
            concat!("Strip partial-word stutter before '", $label, "'"),
            Activation::Safe,
            concat!(
                r#"(?i)\b(?:"#,
                $prefixes,
                r#")(?:[- ]+(?:"#,
                $prefixes,
                r#")){1,3}[- ]+("#,
                $target,
                r#")\b"#
            ),
            "$1"
        )
    };
}

/// Built-in rule table, in application order.
///
/// Order is significant, as described in the module docs above: aggressive
/// discourse edits run first so the final mechanical passes can repair
/// punctuation and spacing left behind, and duplicate/stutter rules stay
/// before `spaced-acronyms` so `I I think` becomes `I think`, not
/// `II think`.
pub(crate) const DEFAULT_RULES: &[Rule] = &[
    // Aggressive leading discourse markers.
    regex_rule!(
        "lead-so-comma",
        "Remove leading 'So,' at a sentence start",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:so,\s+)+"#,
        "$1$2"
    ),
    regex_rule!(
        "lead-so-pronoun",
        "Remove leading 'So ' before a common continuation",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)so\s+(i|we|you|he|she|they|it|this|there|then|and|but|the|a|an|my|our|your)\b"#,
        "$1$2$3"
    ),
    regex_rule!(
        "lead-well-comma",
        "Remove leading 'Well,' at a sentence start",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:well,\s+)+"#,
        "$1$2"
    ),
    regex_rule!(
        "lead-well-pronoun",
        "Remove leading 'Well ' before a common continuation",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)well\s+(i|we|you|he|she|they|it|this|that|there|then|and|but|maybe|actually)\b"#,
        "$1$2$3"
    ),
    regex_rule!(
        "lead-like-comma",
        "Remove leading 'Like,' at a sentence start",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:like,\s+)+"#,
        "$1$2"
    ),
    regex_rule!(
        "lead-like-pronoun",
        "Remove leading filler 'Like ' before a common continuation",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)like\s+(i|we|you|he|she|they|it|this|that|there|the|a|an|my|our|your|some|any)\b"#,
        "$1$2$3"
    ),
    regex_rule!(
        "lead-you-know",
        "Remove only comma-delimited leading 'You know,'",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)you know(?: what i mean)?,\s+"#,
        "$1$2"
    ),
    regex_rule!(
        "lead-i-mean",
        "Remove comma-delimited leading 'I mean,'",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:i mean,\s+)+"#,
        "$1$2"
    ),
    // Aggressive mid-sentence discourse markers. Bare semantic forms are kept.
    regex_rule!(
        "mid-not-like-you-know",
        "Collapse comma-delimited 'not like, you know,' to 'not'",
        Activation::Aggressive,
        r#"(?i)\bnot\s+like,\s*(?:you know(?: what i mean)?|i mean),\s*"#,
        "not "
    ),
    regex_rule!(
        "mid-like-you-know",
        "Remove comma-delimited 'like, you know,' or 'like, I mean,'",
        Activation::Aggressive,
        r#"(?i)\blike,\s*(?:you know(?: what i mean)?|i mean),\s*"#,
        ""
    ),
    regex_rule!(
        "mid-as-in-like",
        "Replace 'as in like X' with 'as in X'",
        Activation::Aggressive,
        r#"(?i)\bas\s+in\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "as in $1"
    ),
    regex_rule!(
        "mid-its-actually-like",
        "Delete filler 'like' from 'it is actually like X' forms",
        Activation::Aggressive,
        r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2 $3"
    ),
    regex_rule!(
        "mid-is-actually-like",
        "Delete filler 'like' after a copula and qualifier",
        Activation::Aggressive,
        r#"(?i)\b(is|was|are|were|am|be|been|being)\s+(actually|basically|literally)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2 $3"
    ),
    regex_rule!(
        "mid-you-know",
        "Remove only fully comma-delimited ', you know,' parentheticals",
        Activation::Aggressive,
        r#"(?i),\s*you know(?: what i mean)?,\s*"#,
        " "
    ),
    regex_rule!(
        "mid-i-mean",
        "Remove fully comma-delimited ', I mean,' parentheticals",
        Activation::Aggressive,
        r#"(?i),\s*i mean,\s*"#,
        " "
    ),
    regex_rule!(
        "mid-like-comma",
        "Remove fully comma-delimited ', like,' parentheticals",
        Activation::Aggressive,
        r#"(?i),\s*like,\s*"#,
        " "
    ),
    regex_rule!(
        "mid-i-dont-know",
        "Remove fully comma-delimited ', I don't know,' parentheticals",
        Activation::Aggressive,
        r#"(?i),\s*i don'?t know,\s*"#,
        " "
    ),
    regex_rule!(
        "mid-like-noun",
        "Delete ', like ' before the next token",
        Activation::Aggressive,
        r#"(?i),\s*like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        " $1"
    ),
    regex_rule!(
        "mid-is-like",
        "Delete 'like' after a copula",
        Activation::Aggressive,
        r#"(?i)\b(is|was|are|were|am|be|been|being)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2"
    ),
    regex_rule!(
        "mid-its-like",
        "Delete 'like' after it/that/there/here is",
        Activation::Aggressive,
        r#"(?i)\b(it'?s|that'?s|there'?s|here'?s)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2"
    ),
    regex_rule!(
        "mid-conj-like",
        "Delete 'like' after and/but/or/so",
        Activation::Aggressive,
        r#"(?i)\b(and|but|or|so)\s+like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2"
    ),
    // High-confidence filled pauses.
    regex_rule!(
        "filler-um-uh",
        "Remove standalone um/uh variants and adjacent commas",
        Activation::Safe,
        r#"(?i),?\s*\b(?:u[hm]+)\b\s*,?"#,
        " "
    ),
    regex_rule!(
        "filler-erm",
        "Remove lowercase erm variants without matching the acronym ER",
        Activation::Safe,
        r#",?\s*\b(?:er+m+)\b\s*,?"#,
        " "
    ),
    // Consolidated duplicate-token handling. The safe list deliberately omits
    // valid/ambiguous repetitions such as that-that and emphatic no-no.
    fancy_rule!(
        "stutter-safe-words",
        "Collapse repeated high-confidence function words with a backreference",
        Activation::Safe,
        r#"(?i)\b(i|the|a|an|and|but|or|so|to|of|in|on|we|you|he|she|they|it|this|my|is|was|are|were|did|will|has|have)\b(?:\s+\1\b)+"#,
        "$1"
    ),
    fancy_rule!(
        "stutter-ambiguous-words",
        "Collapse repeated that/no/can/had/do only in aggressive mode",
        Activation::Aggressive,
        r#"(?i)\b(that|no|can|had|do)\b(?:\s+\1\b)+"#,
        "$1"
    ),
    fancy_rule!(
        "single-letter-stutter",
        "Strip repeated single-letter starts before a matching word",
        Activation::Safe,
        r#"(?i)\b([a-z])(?:[- ]+\1){1,3}[- ]+(\1[a-z][a-z'-]*)\b"#,
        "$2"
    ),
    partial_stutter_rule!("should", "should", "s|so|sh|sho", "should"),
    partial_stutter_rule!(
        "think",
        "think/thinking/thing/this/that",
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
    // General orthographic invariants. No technical-vocabulary allowlist is
    // involved: any run of two or more standalone uppercase letters collapses.
    procedural_rule!(
        "spaced-acronyms",
        "Collapse runs of standalone uppercase letters separated by one ASCII space",
        Activation::Safe,
        normalize_spaced_acronyms
    ),
    // Natural-language number parsing is delegated to text2num. The adjacent
    // passes only format structural version and identifier notation.
    procedural_rule!(
        "spoken-version-numbers",
        "Render multi-point spoken versions using text2num for every component",
        Activation::Safe,
        normalize_spoken_versions
    ),
    procedural_rule!(
        "numeric-point-suffixes",
        "Append spoken point-components to an existing numeric token using text2num",
        Activation::Safe,
        normalize_numeric_point_suffixes
    ),
    procedural_rule!(
        "spoken-numbers",
        "Convert every recognized English number expression with text2num",
        Activation::Safe,
        normalize_spoken_numbers
    ),
    procedural_rule!(
        "numeric-identifier-groups",
        "Join split digit groups in structurally recognizable uppercase identifiers",
        Activation::Safe,
        normalize_numeric_identifier_groups
    ),
    procedural_rule!(
        "numeric-magnitude-suffixes",
        "Join numeric groups to uppercase K/M/B magnitude suffixes",
        Activation::Safe,
        normalize_magnitude_suffixes
    ),
    regex_rule!(
        "compact-version-prefix",
        "Join a standalone V prefix to a dotted version",
        Activation::Safe,
        r#"\b[Vv]\s+(\d+(?:\.\d+)+)\b"#,
        "v$1"
    ),
    regex_rule!(
        "compact-short-identifier",
        "Join one unambiguous uppercase letter or any two-letter prefix to a number",
        Activation::Safe,
        r#"\b([B-HJ-Z]|[A-Z]{2})\s+(\d+(?:\.\d+)*)\b"#,
        "$1$2"
    ),
    // High-confidence casual-form normalization.
    regex_rule!(
        "cause-to-because-mid",
        "Normalize attached apostrophe-cause to because",
        Activation::Safe,
        r#"(?i)([A-Za-z])['\u{2019}]cause\b"#,
        "$1 because"
    ),
    regex_rule!(
        "cause-to-because-bare",
        "Normalize standalone apostrophe-cause to because",
        Activation::Safe,
        r#"(?i)['\u{2019}]cause\b"#,
        "because"
    ),
    regex_rule!(
        "casual-em-til-round",
        "Separate attached casual apostrophe forms",
        Activation::Safe,
        r#"(?i)([A-Za-z])['\u{2019}](em|til|round|bout|cept|nother)\b"#,
        "$1 '$2"
    ),
    regex_rule!(
        "casual-gonna",
        "Expand 'gonna' and 'gunna' to 'going to'",
        Activation::Safe,
        r#"(?i)\bg[ou]nna\b"#,
        "going to"
    ),
    regex_rule!(
        "casual-wanna",
        "Expand 'wanna' and 'wana' to 'want to'",
        Activation::Safe,
        r#"(?i)\bwan{1,2}a\b"#,
        "want to"
    ),
    regex_rule!(
        "casual-kinda",
        "Expand 'kinda' to 'kind of'",
        Activation::Safe,
        r#"(?i)\bkinda\b"#,
        "kind of"
    ),
    regex_rule!(
        "casual-gimme",
        "Expand 'gimme' to 'give me'",
        Activation::Safe,
        r#"(?i)\bgimme\b"#,
        "give me"
    ),
    // Mechanical cleanup and boundary-aware capitalization run last.
    regex_rule!(
        "fix-space-before-punct",
        "Remove whitespace before comma/period/semicolon/colon/exclamation/question",
        Activation::Safe,
        r#"\s+([,.;:!?])"#,
        "$1"
    ),
    regex_rule!(
        "fix-collapse-spaces",
        "Collapse runs of spaces and tabs",
        Activation::Safe,
        r#"[ \t]{2,}"#,
        " "
    ),
    regex_rule!(
        "fix-trim",
        "Trim leading and trailing whitespace",
        Activation::Safe,
        r#"^\s+|\s+$"#,
        ""
    ),
    regex_rule!(
        "fix-leading-comma",
        "Remove a comma left at the start of output",
        Activation::Safe,
        r#"^\s*,\s*"#,
        ""
    ),
    procedural_rule!(
        "capitalize-sentence-starts",
        "Capitalize true sentence starts while protecting dotted tokens",
        Activation::Safe,
        capitalize_sentence_starts
    ),
    regex_rule!(
        "fix-trailing-period",
        "Drop one terminal period for messaging-style output unless explicitly kept",
        Activation::DropTrailingPeriod,
        r#"\.$"#,
        ""
    ),
];
