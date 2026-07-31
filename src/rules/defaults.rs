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
    capitalize_sentence_starts, drop_dangling_connective, normalize_magnitude_suffixes,
    normalize_numeric_identifier_groups, normalize_numeric_point_suffixes,
    normalize_spaced_acronyms, normalize_spoken_versions,
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
        "lead-discourse-comma",
        "Remove comma-delimited leading discourse markers",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:(?:so|well|like|i mean|you know(?: what i mean)?),\s+)+"#,
        "$1$2"
    ),
    regex_rule!(
        "lead-discourse-word",
        "Remove leading so/well/like before a common continuation",
        Activation::Aggressive,
        r#"(?i)(^\s*|[.!?\n]\s*)((?:[\"'(\[]\s*)*)(?:so|well|like)\s+(i|we|you|he|she|they|it|this|that|there|then|and|but|the|a|an|my|our|your|maybe|actually|some|any)\b"#,
        "$1$2$3"
    ),
    // Aggressive mid-sentence discourse markers. Bare semantic forms are kept.
    regex_rule!(
        "mid-like-discourse",
        "Remove comma-delimited like plus you-know/I-mean filler",
        Activation::Aggressive,
        r#"(?i)\b(not\s+)?like,\s*(?:you know(?: what i mean)?|i mean),\s*"#,
        "$1"
    ),
    regex_rule!(
        "mid-discourse-parenthetical",
        "Remove fully comma-delimited discourse parentheticals",
        Activation::Aggressive,
        r#"(?i),\s*(?:you know(?: what i mean)?|i mean|like|i don'?t know),\s*"#,
        " "
    ),
    regex_rule!(
        "mid-filler-like",
        "Remove filler like after a recognized context or comma",
        Activation::Aggressive,
        r#"(?i)(?:\b((?:as\s+in)|(?:(?:it'?s|that'?s|there'?s|here'?s|is|was|are|were|am|be|been|being|and|but|or|so)(?:\s+(?:actually|basically|literally))?))\s+|,\s*)like\s+([A-Za-z0-9][A-Za-z0-9'-]*)\b"#,
        "$1 $2"
    ),
    // High-confidence filled pauses.
    regex_rule!(
        "filled-pauses",
        "Remove standalone um/uh/erm variants and adjacent commas",
        Activation::Safe,
        r#",?\s*\b(?:(?i:u[hm]+)|er+m+)\b\s*,?"#,
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
        "repeated-prefix-stutter",
        "Strip repeated one-letter or consonant-digraph starts before a matching word",
        Activation::Safe,
        r#"(?i)\b([a-z]|sh|th|ch)(?:[- ]+\1){1,3}[- ]+(\1[a-z][a-z'-]*)\b"#,
        "$2"
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
    Rule {
        name: "spoken-numbers",
        description: "Convert recognized English numbers, retaining readable large-magnitude words",
        activation: Activation::Safe,
        kind: RuleKind::SpokenNumbers,
    },
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
    // The `[B-HJ-Z]` class gap excludes standalone "A" and "I" for the same
    // A/I-ambiguity reason as the `short_prefix` check in
    // `passes::normalize_numeric_identifier_groups`; keep both in agreement.
    regex_rule!(
        "compact-short-identifier",
        "Join one unambiguous uppercase letter or any two-letter prefix to a number",
        Activation::Safe,
        r#"\b([B-HJ-Z]|[A-Z]{2})\s+(\d+(?:\.\d+)*)\b"#,
        "$1$2"
    ),
    // High-confidence casual-form normalization.
    regex_rule!(
        "cause-to-because",
        "Normalize standalone or attached apostrophe-cause to because",
        Activation::Safe,
        r#"(?i)([A-Za-z]?)['\u{2019}]cause\b"#,
        "$1 because"
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
    regex_rule!(
        "drop-right-tag-question",
        "Replace the complete ', right?' tag question with a period",
        Activation::Safe,
        r#"(?i),[ \t]*right\?"#,
        "."
    ),
    procedural_rule!(
        "drop-dangling-connective",
        "Remove a trailing connective left dangling at the end of the transcript",
        Activation::Safe,
        drop_dangling_connective
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
