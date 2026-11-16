//! Clock protections and century-pair years over the original numeric spans.

use super::number_words::{horizontal_gap, is_unit, list_joiner, zero_alias};
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
    // Classify the whole connected expression before protecting it. A cue or
    // meridiem can belong to either end, and an endpoint can be a whole hour.
    let mut candidates = Vec::new();
    let mut index = 0;
    while index < spans.len() {
        let hour = &spans[index];
        if !integer(hour) || hour.literal || hour.value > 23.0 {
            index += 1;
            continue;
        }
        let cue = hour.value <= 12.0 && clock_cue(input, tokens, hour.start);
        let minute = spans.get(index + 1).filter(|minute| {
            integer(minute)
                && !minute.literal
                && minute.value <= 59.0
                // "At seven thirty hours" is a clock; a following measurement
                // such as "about five thirty-dollar cards" is a quantity.
                && (!unit_after(tokens, minute.end)
                    || tokens.get(minute.end).is_some_and(|t| t.lowercase == "hours"))
                && hour.end == minute.start
                && !(index > 0 && spans[index - 1].end == hour.start && !spans[index - 1].ordinal)
                && !spans
                    .get(index + 2)
                    .is_some_and(|next| next.start == minute.end && !next.ordinal)
                && horizontal_gap(input, tokens[hour.end - 1].end, tokens[minute.start].start)
        });
        let end = minute.map_or(hour.end, |minute| minute.end);
        let marked_end = clock_marker_end(tokens, end, hour.value <= 12.0);
        let marked = marked_end > end;
        // "Hours" belongs to a clock's extent but needs a cue from the phrase;
        // it must not turn ordinary quantities into clocks on its own.
        let extent_end =
            if minute.is_some() && tokens.get(end).is_some_and(|t| t.lowercase == "hours") {
                end + 1
            } else {
                marked_end
            };
        // Whole hours only participate as endpoints, not as counts before a
        // noun ("at seven thirty and eight files").
        if minute.is_some()
            || hour.value <= 12.0
                && (marked
                    || tokens.get(end).is_none_or(|t| {
                        matches!(
                            t.lowercase.as_str(),
                            "and"
                                | "or"
                                | "to"
                                | "until"
                                | "till"
                                | "through"
                                | "today"
                                | "tomorrow"
                                | "tonight"
                                | "yesterday"
                                | "on"
                                | "in"
                                | "at"
                                | "for"
                                | "every"
                                | "each"
                                | "this"
                                | "next"
                                | "last"
                                | "daily"
                                | "nightly"
                                | "sharp"
                                | "."
                                | ","
                                | "!"
                                | "?"
                                | ";"
                        )
                    }))
        {
            candidates.push((hour.start..extent_end, cue || marked, minute.is_some()));
        }
        index += if minute.is_some() { 2 } else { 1 };
    }
    let mut protected = Vec::new();
    let mut at = 0;
    while at < candidates.len() {
        let mut end = at + 1;
        while end < candidates.len()
            && clock_joiner(tokens, candidates[end - 1].0.end, candidates[end].0.start)
        {
            end += 1;
        }
        let run = &candidates[at..end];
        if run.iter().any(|(_, cue, _)| *cue) && run.iter().any(|(_, _, pair)| *pair) {
            protected.push(run[0].0.start..run[run.len() - 1].0.end);
        }
        at = end;
    }
    protected
}

fn clock_cue(input: &str, tokens: &[NumberToken<'_>], start: usize) -> bool {
    if start == 0 || !horizontal_gap(input, tokens[start - 1].end, tokens[start].start) {
        return false;
    }
    matches!(
        tokens[start - 1].lowercase.as_str(),
        "at" | "from"
            | "to"
            | "until"
            | "between"
            | "by"
            | "before"
            | "after"
            | "around"
            | "about"
            | "till"
            | "past"
            | "since"
    ) || start >= 2 && tokens[start - 2].lowercase == "it" && tokens[start - 1].lowercase == "is"
        || start >= 3
            && tokens[start - 3].lowercase == "it"
            && matches!(tokens[start - 2].text, "'" | "’")
            && tokens[start - 1].lowercase == "s"
}

fn clock_joiner(tokens: &[NumberToken<'_>], start: usize, end: usize) -> bool {
    list_joiner(tokens, start, end)
        || matches!(&tokens[start..end], [joiner] if matches!(joiner.lowercase.as_str(), "until" | "till"))
}

fn clock_marker_end(tokens: &[NumberToken<'_>], end: usize, twelve_hour: bool) -> usize {
    meridiem_end(tokens, end, twelve_hour).unwrap_or_else(|| {
        if twelve_hour
            && matches!(tokens.get(end..end + 3), Some([o, apostrophe, clock])
                if o.lowercase == "o" && matches!(apostrophe.text, "'" | "’") && clock.lowercase == "clock")
        {
            end + 3
        } else {
            end
        }
    })
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
                || !matches!(century.value as u32, 13..=20)
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
            let suffix =
                tail.end + usize::from(tokens.get(tail.end).is_some_and(|token| token.text == "-"));
            if (!plain && !zero)
                || unit_after(tokens, tail.end)
                || meridiem_end(tokens, suffix, false).is_some()
            {
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
    tokens
        .get(end)
        .is_some_and(|next| is_unit(next.text) || matches!(next.text, "K" | "M" | "B"))
}

fn meridiem_end(tokens: &[NumberToken<'_>], end: usize, lower_am: bool) -> Option<usize> {
    let next = tokens.get(end)?;
    if next.text == "AM" || next.lowercase == "pm" || lower_am && next.lowercase == "am" {
        Some(end + 1)
    } else if matches!(next.lowercase.as_str(), "a" | "p")
        && tokens.get(end + 1).is_some_and(|token| token.text == ".")
        && tokens
            .get(end + 2)
            .is_some_and(|token| token.lowercase == "m")
    {
        Some(end + 3 + usize::from(tokens.get(end + 3).is_some_and(|t| t.text == ".")))
    } else {
        None
    }
}
