//! Whole-word restarts, with exceptions for deliberate repetitions.

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
///
/// # Returns
/// The transformed text and the number of collapsed runs; no-op text is borrowed.
///
/// # Errors
/// This transform is infallible; it returns text and a match count directly.
///
/// # Panics
/// Panics only if the built-in word regex cannot compile.
pub(crate) fn collapse_repeated_words(input: &str) -> TransformResult<'_> {
    let words = regex_static!(
        r"[\p{L}\p{M}\p{N}_]+(?:['’][\p{L}\p{M}\p{N}_]+)*",
        "stutter word regex must compile"
    );
    let language = Language::english();
    let mut previous: Option<(usize, String)> = None;
    let mut removals: Vec<(usize, usize)> = Vec::new();
    for found in words.find_iter(input) {
        let lowercase = found.as_str().to_lowercase();
        if let Some((end, prior)) = &previous {
            let gap = &input[*end..found.start()];
            if prior == &lowercase
                && !gap.is_empty()
                && gap.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
                && !protected(&lowercase, &language)
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
