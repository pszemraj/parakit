//! Calendar years and structural identifiers that bypass the prose threshold.

use std::borrow::Cow;
use text2num::{find_numbers, text2digits, Language};

use super::numbers::{number_tokens, NumberToken};

struct ContextNumber {
    start: usize,
    end: usize,
    value: f64,
    text: String,
}

fn context_numbers(tokens: &[NumberToken<'_>], language: &Language) -> Vec<ContextNumber> {
    let mut numbers: Vec<_> = find_numbers(tokens.iter(), language, 0.0)
        .into_iter()
        .map(|number| ContextNumber {
            start: number.start,
            end: number.end,
            value: number.value,
            text: number.text,
        })
        .collect();
    // The speech parser skips existing digits. Calendar components may mix
    // written digits with words, so include literal integers without reparsing.
    for (index, token) in tokens.iter().enumerate() {
        if token.text.bytes().all(|byte| byte.is_ascii_digit())
            && !numbers
                .iter()
                .any(|number| number.start <= index && index < number.end)
        {
            if let Ok(value) = token.text.parse::<u32>() {
                numbers.push(ContextNumber {
                    start: index,
                    end: index + 1,
                    value: f64::from(value),
                    text: token.text.to_string(),
                });
            }
        }
    }
    numbers.sort_by_key(|number| number.start);
    numbers
}

/// Resolve calendar years before the ordinary threshold/preservation pass.
///
/// # Arguments
/// * `input` - One transcript line.
/// * `language` - English number parser used by the main number pass.
///
/// # Returns
/// Text with recognized calendar years rendered as digits.
/// Unchanged text is borrowed.
///
/// # Panics
/// Panics only if the built-in token regex cannot compile.
pub(super) fn normalize_calendar<'a>(input: &'a str, language: &Language) -> Cow<'a, str> {
    let tokens = number_tokens(input);
    let numbers = context_numbers(&tokens, language);
    let mut replacements = Vec::new();
    for (index, century) in numbers.iter().enumerate() {
        if century.text.len() != 2
            || !century.text.bytes().all(|byte| byte.is_ascii_digit())
            || !(10.0..100.0).contains(&century.value)
            || century.value.fract() != 0.0
        {
            continue;
        }
        let Some((last, tail)) = year_tail(&tokens, &numbers, index) else {
            continue;
        };
        if !year_context(&tokens, &numbers, index, century.value, language)
            || clock_suffix(&tokens, last.end)
            || input[tokens[last.end - 1].end..].starts_with('-')
            || numbers.iter().any(|next| next.start == last.end)
        {
            continue;
        }
        replacements.push((
            tokens[century.start].start,
            tokens[last.end - 1].end,
            format!("{}{:02}", century.text, tail),
        ));
        // A day explicitly followed by a recognized month/year is structural,
        // including first/second/third below the ordinary prose threshold.
        if let Some(day) = month_day(&tokens, &numbers, index) {
            replacements.push((
                tokens[day.start].start,
                tokens[day.end - 1].end,
                day.text.clone(),
            ));
        }
    }

    apply_replacements(input, replacements)
}

/// Render a short identifier's spoken digit after fraction spans are protected.
///
/// # Arguments
/// * `input` - A segment outside protected fractions and time units.
/// * `language` - English number parser used by the main number pass.
///
/// # Returns
/// Text with short-label digits rendered independently of the prose threshold.
/// Unchanged text is borrowed.
///
/// # Panics
/// Panics only if the built-in token regex cannot compile.
pub(super) fn normalize_identifiers<'a>(input: &'a str, language: &Language) -> Cow<'a, str> {
    let tokens = number_tokens(input);
    let numbers = find_numbers(tokens.iter(), language, 0.0);
    let mut replacements = Vec::new();
    for (index, number) in numbers.iter().enumerate() {
        if number.is_ordinal
            || !(0.0..10.0).contains(&number.value)
            || number.value.fract() != 0.0
            || number.start == 0
        {
            continue;
        }
        let stem = &tokens[number.start - 1];
        if !short_stem(stem.text)
            || !horizontal_gap(input, stem.end, tokens[number.start].start)
            || input[tokens[number.end - 1].end..].starts_with('-')
        {
            continue;
        }
        let continued = numbers
            .get(index + 1)
            .is_some_and(|next| next.start == number.end);
        if continued {
            continue;
        }
        replacements.push((
            tokens[number.start].start,
            tokens[number.end - 1].end,
            number.text.clone(),
        ));
    }
    apply_replacements(input, replacements)
}

fn apply_replacements(input: &str, mut replacements: Vec<(usize, usize, String)>) -> Cow<'_, str> {
    if replacements.is_empty() {
        return Cow::Borrowed(input);
    }
    replacements.sort_by_key(|(start, end, _)| (*start, *end));
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    for (start, end, replacement) in replacements {
        if start < cursor {
            continue;
        }
        output.push_str(&input[cursor..start]);
        output.push_str(&replacement);
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    Cow::Owned(output)
}

fn short_stem(stem: &str) -> bool {
    stem == "SM"
        || (stem.len() == 1
            && stem.as_bytes()[0].is_ascii_uppercase()
            && !matches!(stem, "A" | "I"))
}

fn horizontal_gap(input: &str, start: usize, end: usize) -> bool {
    start < end
        && input[start..end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
}

fn year_tail<'a>(
    tokens: &[NumberToken<'_>],
    numbers: &'a [ContextNumber],
    index: usize,
) -> Option<(&'a ContextNumber, u32)> {
    let century = &numbers[index];
    let next = numbers.get(index + 1)?;
    if next.text.len() > 2
        || !next.text.bytes().all(|byte| byte.is_ascii_digit())
        || !(0.0..100.0).contains(&next.value)
    {
        return None;
    }
    if century.end == next.start {
        if next.text == "0" {
            if let Some(unit) = numbers.get(index + 2).filter(|unit| {
                unit.text.bytes().all(|byte| byte.is_ascii_digit())
                    && unit.start == next.end
                    && (0.0..10.0).contains(&unit.value)
            }) {
                return Some((unit, unit.value as u32));
            }
        }
        return Some((next, next.value as u32));
    }
    // ASR commonly spells the zero in a year as "oh" or "o".
    (next.start == century.end + 1
        && next.value < 10.0
        && matches!(tokens[century.end].lowercase.as_str(), "oh" | "o"))
    .then_some((next, next.value as u32))
}

fn year_context(
    tokens: &[NumberToken<'_>],
    numbers: &[ContextNumber],
    index: usize,
    century: f64,
    language: &Language,
) -> bool {
    let first = &numbers[index];
    let Some(previous) = first.start.checked_sub(1).map(|at| &tokens[at]) else {
        return false;
    };
    if matches!(previous.lowercase.as_str(), "year" | "years" | "fy")
        || is_month(&previous.lowercase)
        || month_day(tokens, numbers, index).is_some()
    {
        return true;
    }
    // Keep bare clock ranges such as "from nineteen thirty until twenty
    // fifteen" protected. "Is from" describes an origin rather than a range.
    if (18.0..=20.0).contains(&century)
        && matches!(previous.lowercase.as_str(), "in" | "during" | "since")
    {
        return true;
    }
    if (18.0..=20.0).contains(&century)
        && previous.lowercase == "from"
        && first.start >= 2
        && matches!(
            tokens[first.start - 2].lowercase.as_str(),
            "is" | "was" | "are" | "were"
        )
    {
        return true;
    }
    let end = if previous.lowercase == "of" {
        first.start - 1
    } else {
        first.start
    };
    quarter_before(tokens, end, language)
}

fn quarter_before(tokens: &[NumberToken<'_>], end: usize, language: &Language) -> bool {
    let Some(last) = end.checked_sub(1).map(|at| &tokens[at]) else {
        return false;
    };
    if matches!(last.lowercase.as_str(), "q1" | "q2" | "q3" | "q4") {
        return true;
    }
    if end >= 2 && tokens[end - 2].lowercase == "q" {
        let value = text2digits(last.text, language)
            .ok()
            .or_else(|| last.text.parse::<u32>().ok().map(|value| value.to_string()));
        return value.is_some_and(|value| matches!(value.as_str(), "1" | "2" | "3" | "4"));
    }
    if last.lowercase == "quarter" && end >= 2 {
        return matches!(
            tokens[end - 2].lowercase.as_str(),
            "first" | "second" | "third" | "fourth" | "1st" | "2nd" | "3rd" | "4th"
        );
    }
    false
}

fn month_day<'a>(
    tokens: &[NumberToken<'_>],
    numbers: &'a [ContextNumber],
    index: usize,
) -> Option<&'a ContextNumber> {
    let year = &numbers[index];
    let day = numbers.get(index.checked_sub(1)?)?;
    if !(1.0..=31.0).contains(&day.value)
        || day.value.fract() != 0.0
        || day.start == 0
        || !is_month(&tokens[day.start - 1].lowercase)
        || tokens[day.end..year.start]
            .iter()
            .any(|token| token.text != ",")
    {
        return None;
    }
    Some(day)
}

fn is_month(word: &str) -> bool {
    matches!(
        word,
        "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
    )
}

fn clock_suffix(tokens: &[NumberToken<'_>], end: usize) -> bool {
    tokens.get(end).is_some_and(|token| {
        matches!(token.text, "K" | "M" | "B")
            || matches!(token.lowercase.as_str(), "am" | "pm" | "hours")
            || (matches!(token.lowercase.as_str(), "a" | "p")
                && tokens.get(end + 1).is_some_and(|token| token.text == ".")
                && tokens
                    .get(end + 2)
                    .is_some_and(|token| token.lowercase == "m"))
    })
}
