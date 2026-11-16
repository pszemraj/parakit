//! Clock protections and century-pair years over the original numeric spans.

use super::number_words::{is_unit, zero_alias};
use super::numbers::{NumberKind, NumberSpan, NumberToken, Replacement};
use std::ops::Range;

/// Preserve hour/minute pairs rather than rendering them as spaced digits.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Original tokens with byte positions.
/// * `spans` - Parsed numbers with token-index boundaries.
///
/// # Returns
/// Original-token ranges with a meridiem marker or a twelve-hour clock cue.
///
/// # Panics
/// Panics if supplied span boundaries do not index the supplied tokens.
pub(super) fn clock_times(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
) -> Vec<Range<usize>> {
    let mut previous_clock_end = None;
    spans
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| {
            let (hour, minute) = (&pair[0], &pair[1]);
            if !integer(hour)
                || !integer(minute)
                || hour.value > 23.0
                || minute.value > 59.0
                || hour.end != minute.start
                || index > 0 && spans[index - 1].end == hour.start && !spans[index - 1].ordinal
                || spans
                    .get(index + 2)
                    .is_some_and(|next| next.start == minute.end && !next.ordinal)
                || !horizontal_gap(input, tokens[hour.end - 1].end, tokens[minute.start].start)
            {
                return None;
            }
            // Without a clock cue, nineteen thirty remains the year 1930.
            let cue = hour.value <= 12.0
                && hour.start > 0
                && (matches!(
                    tokens[hour.start - 1].lowercase.as_str(),
                    "at" | "from" | "to" | "until" | "between"
                ) || tokens[hour.start - 1].lowercase == "and"
                    && previous_clock_end == Some(hour.start - 1))
                && horizontal_gap(input, tokens[hour.start - 1].end, tokens[hour.start].start);
            let am = hour.value <= 12.0
                && tokens
                    .get(minute.end)
                    .is_some_and(|next| next.lowercase == "am");
            if cue || am || meridiem_after(tokens, minute.end) {
                previous_clock_end = Some(minute.end);
                Some(hour.start..minute.end)
            } else {
                None
            }
        })
        .collect()
}

/// Propose century-pair years independently of surrounding prose or thresholds.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Original tokens with byte positions.
/// * `spans` - Parsed numbers with token-index boundaries.
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
                || index > 0
                    && spans[index - 1].end == century.start
                    && !spans[index - 1].ordinal
                    && !super::numbers::identifier_stem(input, tokens, spans[index - 1].start)
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
                && zero_alias(&between[0].lowercase)
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
        is_unit(next.text) || matches!(next.text, "K" | "M" | "B") || meridiem_after(tokens, end)
    })
}

fn meridiem_after(tokens: &[NumberToken<'_>], end: usize) -> bool {
    tokens.get(end).is_some_and(|next| {
        matches!(next.text, "AM" | "PM")
            || next.lowercase == "pm"
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
