//! Spoken-number formatting layered around the `text2num` English parser.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;
use text2num::{find_numbers, replace_numbers_in_text, text2digits, Language, Token};

use super::engine::TransformResult;

#[derive(Debug)]
struct NumberToken<'a> {
    text: &'a str,
    lowercase: String,
    start: usize,
    end: usize,
}

impl Token for &NumberToken<'_> {
    fn text(&self) -> Cow<'_, str> {
        self.text.into()
    }

    fn text_lowercase(&self) -> Cow<'_, str> {
        self.lowercase.as_str().into()
    }
}

/// Convert English number expressions to digits by delegating to `text2num`.
///
/// `threshold` is the minimum isolated numeric value rendered as digits.
/// Expressions ending in one explicit `million` or `billion` scale retain
/// that word with a numeric coefficient for readability (e.g. `"three
/// billion"` becomes `"3 billion"`). Multi-scale expressions retain
/// `text2num`'s full-digit rendering. Contexts where `"second"` is a time
/// unit rather than an ordinal are protected from conversion.
///
/// # Arguments
///
/// * `input` - Transcript text to normalize.
/// * `threshold` - Minimum isolated numeric value rendered as digits.
///
/// # Returns
///
/// A [`TransformResult`] with `matches` set to `1` if the text changed and
/// `0` otherwise.
///
/// # Errors
///
/// This function is infallible: it returns [`TransformResult`], not
/// `Result`, and never returns an `Err`.
pub(crate) fn normalize_spoken_numbers(input: &str, threshold: f64) -> TransformResult {
    let language = Language::english();
    let text = replace_numbers_preserving_time_units(input, &language, threshold);
    let text = render_signed_numbers(text);
    TransformResult {
        matches: usize::from(text != input),
        text,
    }
}

/// Replace a spoken sign immediately before a number rendered by `text2num`.
fn render_signed_numbers(input: String) -> String {
    static SIGNED_NUMBER: OnceLock<Regex> = OnceLock::new();
    let signed_number = SIGNED_NUMBER.get_or_init(|| {
        Regex::new(
            r"(?i)(^|[^[:alnum:]_-])(?:negative|minus)[ \t]+(\d+(?:\.\d+)?(?:[ \t]+(?:million|billion))?)\b",
        )
            .expect("signed-number regex must compile")
    });
    if !signed_number.is_match(&input) {
        return input;
    }
    signed_number.replace_all(&input, "${1}-${2}").into_owned()
}

fn replace_numbers_preserving_time_units(
    input: &str,
    language: &Language,
    threshold: f64,
) -> String {
    static SECOND: OnceLock<Regex> = OnceLock::new();
    static PREVIOUS_TOKEN: OnceLock<Regex> = OnceLock::new();
    let second_re = SECOND
        .get_or_init(|| Regex::new(r"(?i)\bsecond\b").expect("second-word regex must compile"));
    let previous_re = PREVIOUS_TOKEN.get_or_init(|| {
        Regex::new(r"(?i)([a-z0-9]+)([-\s]+)$").expect("previous-token regex must compile")
    });

    let protected: Vec<_> = second_re
        .find_iter(input)
        .filter(|found| second_is_time_unit(&input[..found.start()], previous_re, language))
        .collect();
    if protected.is_empty() {
        return replace_numbers_with_hybrid_magnitudes(input, language, threshold);
    }

    // Run text2num independently around protected units so their preceding
    // quantity can still convert without turning the unit into the ordinal 2nd.
    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    for found in protected {
        output.push_str(&replace_numbers_with_hybrid_magnitudes(
            &input[last_end..found.start()],
            language,
            threshold,
        ));
        output.push_str(found.as_str());
        last_end = found.end();
    }
    output.push_str(&replace_numbers_with_hybrid_magnitudes(
        &input[last_end..],
        language,
        threshold,
    ));
    output
}

fn replace_numbers_with_hybrid_magnitudes(
    input: &str,
    language: &Language,
    threshold: f64,
) -> String {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    static NUMERIC_MAGNITUDE: OnceLock<Regex> = OnceLock::new();
    let token_re =
        TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9]+|[^\s]").expect("number token regex"));
    let numeric_re = NUMERIC_MAGNITUDE.get_or_init(|| {
        Regex::new(r"(?i)\b\d+(?:\.\d+)?[ \t]+(?:million|billion)\b")
            .expect("numeric magnitude regex")
    });

    let tokens: Vec<_> = token_re
        .find_iter(input)
        .map(|found| NumberToken {
            text: found.as_str(),
            lowercase: found.as_str().to_ascii_lowercase(),
            start: found.start(),
            end: found.end(),
        })
        .collect();
    let occurrences = find_numbers(tokens.iter(), language, threshold);
    let mut replacements: Vec<_> = numeric_re
        .find_iter(input)
        .map(|found| (found.start(), found.end(), found.as_str().to_string()))
        .collect();

    for (scale_index, scale_token) in tokens.iter().enumerate() {
        let Some((scale_word, scale_value)) = magnitude_scale(&scale_token.lowercase) else {
            continue;
        };
        if replacements
            .iter()
            .any(|(start, end, _)| *start <= scale_token.start && scale_token.end <= *end)
        {
            continue;
        }

        let Some(scale_occurrence) = occurrences.iter().find(|occurrence| {
            occurrence.start <= scale_index && occurrence.end == scale_index + 1
        }) else {
            continue;
        };
        if scale_occurrence.is_ordinal {
            continue;
        }

        let (mut start_index, coefficient) = if scale_occurrence.start < scale_index {
            (scale_occurrence.start, scale_occurrence.value / scale_value)
        } else {
            match occurrences.iter().rev().find(|occurrence| {
                occurrence.end == scale_index
                    && !occurrence.is_ordinal
                    && input[tokens[occurrence.end - 1].end..scale_token.start]
                        .chars()
                        .all(char::is_whitespace)
            }) {
                Some(previous) => (previous.start, previous.value),
                None if preceding_article_is_adjacent(input, &tokens, scale_index) => {
                    (scale_index - 1, 1.0)
                }
                None => continue,
            }
        };

        if preceding_article_is_adjacent(input, &tokens, start_index) {
            start_index -= 1;
        }

        if tokens[start_index..=scale_index]
            .iter()
            .filter(|token| magnitude_scale(&token.lowercase).is_some())
            .count()
            != 1
        {
            continue;
        }

        let rendered = coefficient.to_string();
        if !coefficient.is_finite() || rendered.contains(['e', 'E']) {
            continue;
        }
        replacements.push((
            tokens[start_index].start,
            scale_token.end,
            format!("{rendered} {scale_word}"),
        ));
    }

    if replacements.is_empty() {
        return replace_numbers_in_text(input, language, threshold);
    }
    replacements.sort_by_key(|(start, end, _)| (*start, *end));

    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    for (start, end, replacement) in replacements {
        if start < last_end {
            continue;
        }
        output.push_str(&replace_numbers_in_text(
            &input[last_end..start],
            language,
            threshold,
        ));
        output.push_str(&replacement);
        last_end = end;
    }
    output.push_str(&replace_numbers_in_text(
        &input[last_end..],
        language,
        threshold,
    ));
    output
}

fn preceding_article_is_adjacent(input: &str, tokens: &[NumberToken<'_>], index: usize) -> bool {
    index > 0
        && tokens[index - 1].lowercase == "a"
        && input[tokens[index - 1].end..tokens[index].start]
            .chars()
            .all(char::is_whitespace)
}

fn magnitude_scale(token: &str) -> Option<(&'static str, f64)> {
    match token {
        "million" => Some(("million", 1_000_000.0)),
        "billion" => Some(("billion", 1_000_000_000.0)),
        _ => None,
    }
}

fn second_is_time_unit(prefix: &str, previous_re: &Regex, language: &Language) -> bool {
    let Some(captures) = previous_re.captures(prefix) else {
        return false;
    };
    let previous = captures.get(1).expect("capture 1 is required").as_str();
    let separator = captures.get(2).expect("capture 2 is required").as_str();

    if separator.contains('-') {
        let candidate = format!("{previous}-second");
        // Recognized compounds such as "twenty-second" are genuine ordinals.
        return text2digits(&candidate, language).is_err();
    }

    matches!(
        previous.to_ascii_lowercase().as_str(),
        "a" | "per" | "every" | "each"
    ) || previous.chars().all(|character| character.is_ascii_digit())
        || text2digits(previous, language).is_ok_and(|rendered| {
            !rendered.ends_with("st")
                && !rendered.ends_with("nd")
                && !rendered.ends_with("rd")
                && !rendered.ends_with("th")
        })
}
