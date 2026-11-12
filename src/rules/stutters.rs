//! Whole-word restarts, with exceptions for deliberate repetitions.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::borrow::Cow;
use text2num::{text2digits, Language};

use super::engine::TransformResult;

// Grammar-dependent doubles remain the aggressive rule's responsibility.
const KEEP_REPEATED: &[&str] = &[
    "that",
    "had",
    "do",
    "can",
    "no",
    "plus",
    "dash",
    "equals",
    "slash",
    "dot",
    "star",
    "underscore",
    "colon",
    "pipe",
    "blah",
    "yada",
    "ha",
    "heh",
    "la",
    "bye",
    "yeah",
    "very",
    "really",
    "much",
    "many",
    "far",
];

/// Collapse identical adjacent words separated only by ASCII spaces or tabs.
///
/// Contractions stay whole, the first word's spelling is retained, and
/// punctuation or a line break ends a run. Number words, digits, spelling
/// letters other than I/a, and deliberate doubles are protected.
///
/// # Arguments
/// * `input` - Transcript before number and acronym formatting.
/// * `exceptions` - Compiled literal word/phrase exceptions configured by the user.
///
/// # Returns
/// The transformed text and the number of collapsed runs; no-op text is borrowed.
///
/// # Errors
/// This transform is infallible; it returns text and a match count directly.
///
/// # Panics
/// Panics only if the built-in word regex cannot compile.
pub(crate) fn collapse_repeated_words<'a>(
    input: &'a str,
    exceptions: &[Regex],
) -> TransformResult<'a> {
    let words = regex_static!(
        r"[\p{L}\p{M}\p{N}_]+(?:['’][\p{L}\p{M}\p{N}_]+)*",
        "stutter word regex must compile"
    );
    let language = Language::english();
    let word_spans: Vec<_> = words.find_iter(input).collect();
    let protected_spans: Vec<_> = exceptions
        .iter()
        .flat_map(|pattern| pattern.find_iter(input))
        .filter(|found| {
            word_spans
                .binary_search_by_key(&found.start(), |word| word.start())
                .is_ok()
                && word_spans
                    .binary_search_by_key(&found.end(), |word| word.end())
                    .is_ok()
        })
        .map(|found| found.range())
        .collect();
    let mut previous: Option<(usize, String)> = None;
    let mut removals: Vec<(usize, usize)> = Vec::new();
    for found in word_spans {
        let lowercase = found.as_str().to_lowercase();
        if let Some((end, prior)) = &previous {
            let gap = &input[*end..found.start()];
            if prior == &lowercase
                && !gap.is_empty()
                && gap.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
                && !protected(&lowercase, &language)
                && !protected_spans
                    .iter()
                    .any(|span| span.start <= found.start() && found.end() <= span.end)
                && !distinct_grammatical_roles(&input[..*end], &input[found.end()..], &lowercase)
            {
                if let Some((_, last_end)) = removals.last_mut().filter(|(_, last)| last == end) {
                    *last_end = found.end();
                } else {
                    removals.push((*end, found.end()));
                }
            }
        }
        previous = Some((found.end(), lowercase));
    }
    if removals.is_empty() {
        return TransformResult {
            text: Cow::Borrowed(input),
            matches: 0,
        };
    }
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    for &(start, end) in &removals {
        output.push_str(&input[cursor..start]);
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    TransformResult {
        text: Cow::Owned(output),
        matches: removals.len(),
    }
}

/// Compile additional literal words/phrases, canonicalized for stable rule-set IDs.
///
/// # Returns
/// Case-insensitive whole-word matchers in canonical order.
///
/// # Errors
/// Returns an error for an empty entry, non-word text, or an invalid matcher.
///
/// # Panics
/// Panics only if the built-in word regex cannot compile.
pub(super) fn compile_repetition_exceptions(entries: &[String]) -> Result<Vec<Regex>> {
    let word = regex_static!(
        r"^[\p{L}\p{M}\p{N}_]+(?:['’][\p{L}\p{M}\p{N}_]+)*$",
        "exception word regex must compile"
    );
    let mut phrases = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let words: Vec<_> = entry.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty()
            || words.iter().any(|part| !word.is_match(part))
            || entry
                .chars()
                .any(|ch| ch.is_whitespace() && !matches!(ch, ' ' | '\t'))
        {
            bail!("repetition exception {} must contain words separated by spaces/tabs, without other whitespace, punctuation, or regex syntax", index + 1);
        }
        phrases.push(words);
    }
    phrases.sort();
    phrases.dedup();
    phrases
        .into_iter()
        .map(|words| {
            let literal = words
                .iter()
                .map(|word| regex::escape(word))
                .collect::<Vec<_>>()
                .join("[ \\t]+");
            Regex::new(&format!(r"(?i)\b{literal}\b"))
                .context("could not compile repetition exception")
        })
        .collect()
}

// Identical words can finish an object clause and supply its predicate label,
// or act as both a verb and the Control modifier in a keyboard shortcut.
fn distinct_grammatical_roles(prefix: &str, suffix: &str, word: &str) -> bool {
    let naming = regex_static!(
        concat!(
            r"(?i)\b(?:call(?:s|ed|ing)?|label(?:s|ed|led|ing|ling)?|nam(?:e|es|ed|ing)|consider(?:s|ed|ing)?)\b[^.!?;\n]*",
            r"\b(?:that|which|who)[ \t]+(?:is|are|was|were|isn['’]t|aren['’]t|wasn['’]t|weren['’]t)",
            r"(?:[ \t]+not)?(?:[ \t]+(?:a|an))?[ \t]+[\p{L}\p{M}\p{N}_]+(?:['’][\p{L}\p{M}\p{N}_]+)*$"
        ),
        "object-label repetition regex must compile"
    );
    if !matches!(
        word,
        "is" | "are"
            | "was"
            | "were"
            | "not"
            | "a"
            | "an"
            | "the"
            | "it"
            | "its"
            | "he"
            | "she"
            | "we"
            | "you"
            | "they"
            | "and"
            | "or"
            | "to"
            | "of"
            | "in"
    ) && naming.is_match(prefix)
    {
        return true;
    }
    if word != "control" {
        return false;
    }
    let governing_verb = regex_static!(
        r"(?i)\b(?:to|can|could|cannot|can['’]t|may|might|must|should|will|would|won['’]t|don['’]t|didn['’]t|doesn['’]t)[ \t]+control$",
        "governing control verb regex must compile"
    );
    let key = regex_static!(
        r"(?i)^[ \t]+(?:control[ \t]+)*(?:[a-z0-9]|f(?:[1-9]|1[0-9]|2[0-4])|alt|shift|tab|enter|esc(?:ape)?|space|backspace|delete|home|end|page[ \t]+(?:up|down))\b",
        "keyboard shortcut tail regex must compile"
    );
    governing_verb.is_match(prefix) && key.is_match(suffix)
}

fn protected(word: &str, language: &Language) -> bool {
    if matches!(word, "i" | "a") {
        return false;
    }
    word.chars().count() == 1
        || word.chars().any(char::is_numeric)
        || KEEP_REPEATED.contains(&word)
        || text2digits(word, language).is_ok()
        // Spoken numeric forms outside text2num's English vocabulary.
        || matches!(word, "oh" | "nil" | "zeroth" | "trillion" | "trillions")
}
