//! Century-pair years over the original numeric spans.

use text2num::Language;

use super::number_words::is_unit;
use super::numbers::{NumberKind, NumberSpan, NumberToken, Replacement};

/// Propose century-pair years independently of surrounding prose or thresholds.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Original tokens with byte positions.
/// * `spans` - Parsed numbers with token-index boundaries.
/// * `_language` - Shared number-parser language.
///
/// # Returns
/// Year proposals in original-token coordinates.
///
/// # Panics
/// Panics if supplied span boundaries do not index the supplied tokens.
pub(super) fn classify(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    _language: &Language,
) -> Vec<Replacement> {
    spans
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| {
            let (century, tail) = (&pair[0], &pair[1]);
            if !integer(century)
                || century.text.len() != 2
                || !matches!(century.value as u32, 18..=20)
                || !integer(tail)
                || century.literal && tail.literal && century.end == tail.start
                || index > 0 && spans[index - 1].end == century.start
                || spans
                    .get(index + 2)
                    .is_some_and(|next| next.start == tail.end)
            {
                return None;
            }
            let between = &tokens[century.end..tail.start];
            let plain = between.is_empty()
                && tail.text.len() == 2
                && (10.0..100.0).contains(&tail.value)
                && horizontal_gap(input, tokens[century.end - 1].end, tokens[tail.start].start);
            let zero = between.len() == 1
                && matches!(between[0].lowercase.as_str(), "oh" | "o")
                && tail.value < 10.0
                && tail.text.len() == 1
                && horizontal_gap(input, tokens[century.end - 1].end, between[0].start)
                && horizontal_gap(input, between[0].end, tokens[tail.start].start);
            if (!plain && !zero) || unit_after(tokens, tail.end) {
                return None;
            }
            Some(Replacement {
                start: century.start,
                end: tail.end,
                text: format!("{}{:02}", century.text, tail.value as u32),
                kind: NumberKind::Year,
            })
        })
        .collect()
}

fn integer(number: &NumberSpan) -> bool {
    !number.ordinal
        && number.value >= 0.0
        && number.value.fract() == 0.0
        && number.text.bytes().all(|byte| byte.is_ascii_digit())
}

fn unit_after(tokens: &[NumberToken<'_>], end: usize) -> bool {
    let end = end + usize::from(tokens.get(end).is_some_and(|token| token.text == "-"));
    tokens.get(end).is_some_and(|next| {
        is_unit(&next.lowercase)
            || matches!(next.text, "K" | "M" | "B")
            || matches!(next.lowercase.as_str(), "am" | "pm")
            || matches!(next.lowercase.as_str(), "a" | "p")
                && tokens.get(end + 1).is_some_and(|token| token.text == ".")
                && tokens
                    .get(end + 2)
                    .is_some_and(|token| token.lowercase == "m")
    })
}

fn horizontal_gap(input: &str, start: usize, end: usize) -> bool {
    input[start..end]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
}
