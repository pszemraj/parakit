//! Calendar years, model numbers, and identifiers outside the prose threshold.

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

/// Join spoken two-digit groups when a local hardware/model reference identifies them.
///
/// # Arguments
/// * `input` - A complete transcript line, including following fractions and units.
/// * `language` - English number parser shared with the ordinary number pass.
///
/// # Returns
/// Model numbers rendered as digits; unchanged input is borrowed.
///
/// # Panics
/// Panics only if the built-in token regex cannot compile.
pub(super) fn normalize_model_numbers<'a>(input: &'a str, language: &Language) -> Cow<'a, str> {
    let tokens = number_tokens(input);
    let numbers = context_numbers(&tokens, language);
    let mut replacements = Vec::new();
    for (index, first) in numbers.iter().enumerate() {
        if first.text.len() != 2
            || !first.text.bytes().all(|byte| byte.is_ascii_digit())
            || !(10.0..100.0).contains(&first.value)
        {
            continue;
        }
        let Some((last, tail)) = year_tail(&tokens, &numbers, index) else {
            continue;
        };
        // A one-digit tail needs an explicit zero: "fifty oh five", not
        // an arbitrary grouping of a count with a following single digit.
        if (last.text.len() != 2
            && !matches!(
                tokens[first.end].lowercase.as_str(),
                "oh" | "o" | "zero" | "0"
            ))
            || index > 0 && numbers[index - 1].end == first.start
            || numbers.iter().any(|next| next.start == last.end)
            || number_group_continues(&input[tokens[last.end - 1].end..])
            || !model_reference_tail(&tokens, last.end)
        {
            continue;
        }
        let label_at = model_variant_end(&tokens, last.end).unwrap_or(last.end);
        let direct = first.start > 0 && model_label(&tokens, first.start - 1)
            || tokens
                .get(label_at)
                .is_some_and(|_| model_label(&tokens, label_at));
        if !direct && !model_reference(input, &tokens, first.start) {
            continue;
        }
        replacements.push((
            tokens[first.start].start,
            tokens[last.end - 1].end,
            format!("{}{:02}", first.text, tail),
        ));
    }
    apply_replacements(input, replacements)
}

// A label can introduce the number or follow it as the hardware noun.
fn model_label(tokens: &[NumberToken<'_>], at: usize) -> bool {
    match tokens[at].lowercase.as_str() {
        "gpu" | "gpus" | "cpu" | "cpus" | "processor" | "processors" | "model" | "models" => true,
        "card" | "cards" => {
            at > 0 && matches!(tokens[at - 1].lowercase.as_str(), "graphics" | "video")
        }
        "graphics" | "video" => tokens
            .get(at + 1)
            .is_some_and(|next| matches!(next.lowercase.as_str(), "card" | "cards")),
        "number" => at > 0 && matches!(tokens[at - 1].lowercase.as_str(), "model" | "part"),
        _ => false,
    }
}

fn model_reference(input: &str, tokens: &[NumberToken<'_>], start: usize) -> bool {
    let Some(determiner) = (start.saturating_sub(3)..start).rev().find(|&at| {
        matches!(
            tokens[at].lowercase.as_str(),
            "a" | "an" | "the" | "my" | "your" | "our" | "their" | "his" | "her" | "this" | "that"
        )
    }) else {
        return false;
    };
    if tokens[determiner + 1..start].iter().any(|token| {
        !token.text.bytes().all(|byte| byte.is_ascii_alphabetic())
            || matches!(
                token.lowercase.as_str(),
                "score"
                    | "time"
                    | "price"
                    | "cost"
                    | "rate"
                    | "count"
                    | "range"
                    | "budget"
                    | "resolution"
                    | "memory"
                    | "context"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "has"
                    | "have"
                    | "for"
                    | "at"
                    | "in"
                    | "of"
                    | "to"
                    | "and"
            )
    }) {
        return false;
    }
    (start.saturating_sub(8)..determiner).rev().any(|at| {
        model_label(tokens, at)
            && input[tokens[at].end..tokens[start].start]
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b' ' | b'\t' | b','))
    })
}

// Keep the referent as a noun: an unknown following noun can instead be a
// quantity/unit ("fifty ninety meter cable"). Accept sentence boundaries,
// hardware nouns, and the predicates/connectors that continue a model reference.
fn model_reference_tail(tokens: &[NumberToken<'_>], end: usize) -> bool {
    model_predicate_tail(tokens, end)
        || model_variant_end(tokens, end).is_some_and(|after| model_predicate_tail(tokens, after))
}

fn model_variant_end(tokens: &[NumberToken<'_>], end: usize) -> Option<usize> {
    if tokens.get(end).is_some_and(|_| model_label(tokens, end)) {
        return None;
    }
    let at = end + usize::from(tokens.get(end).is_some_and(|token| token.text == "-"));
    tokens
        .get(at)
        .filter(|token| model_variant(token.text))
        .map(|_| at + 1)
}

fn model_variant(word: &str) -> bool {
    let code = regex_static!(r"^[A-Z][A-Za-z]{1,2}$", "model variant regex must compile");
    code.is_match(word) && !model_measurement_unit(word)
}

fn model_measurement_unit(word: &str) -> bool {
    let unit = regex_static!(
        concat!(
            r"(?i)^(?:[kmunpgtpe]?(?:g|m|s|a|w|v|b)|[kmgtp]?ib|[kmgt]?hz|[a-z]p[smh]|[kmg]?px|",
            r"bit|mol|amp|ton|bar|rad|lux|db|lb|lbs|oz|ft|yd|in|usd|eur|gbp|jpy|cad|aud|chf|cny|",
            r"(?:nano|micro|milli|centi|deci|kilo|mega|giga|tera)?(?:meter|metre|gram|second)s?|",
            r"minutes?|hours?|days?|years?|degrees?|percent|dollars?|euros?|pounds?|watts?|bytes?|bits?)$"
        ),
        "model suffix unit regex must compile"
    );
    unit.is_match(word)
}

fn model_predicate_tail(tokens: &[NumberToken<'_>], end: usize) -> bool {
    let Some(next) = tokens.get(end) else {
        return true;
    };
    let boundary = |token: &NumberToken<'_>| {
        matches!(
            token.text,
            "." | "," | ";" | ":" | "!" | "?" | "(" | ")" | "[" | "]"
        )
    };
    let word = next.lowercase.as_str();
    boundary(next)
        || matches!(word, "gpu" | "cpu" | "processor" | "model")
        || (matches!(word, "graphics" | "video")
            && tokens
                .get(end + 1)
                .is_some_and(|next| next.lowercase == "card"))
        || reference_tail_word(word)
        || (word.ends_with("ing")
            && tokens
                .get(end + 1)
                .is_none_or(|next| boundary(next) || reference_tail_word(&next.lowercase)))
}

fn reference_tail_word(word: &str) -> bool {
    matches!(
        word,
        "here"
            | "there"
            | "instead"
            | "too"
            | "also"
            | "is"
            | "are"
            | "was"
            | "were"
            | "has"
            | "had"
            | "does"
            | "did"
            | "can"
            | "could"
            | "will"
            | "would"
            | "should"
            | "works"
            | "runs"
            | "sits"
            | "and"
            | "or"
            | "but"
            | "with"
            | "without"
            | "for"
            | "from"
            | "to"
            | "on"
            | "in"
            | "at"
            | "by"
    )
}

/// Recognize an additional count or decimal after a numeric group.
///
/// # Returns
/// Whether joining would consume only the prefix of a longer numeric run.
///
/// # Panics
/// Panics only if the built-in token regex cannot compile.
pub(super) fn number_group_continues(suffix: &str) -> bool {
    if suffix
        .strip_prefix('.')
        .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()))
    {
        return true;
    }
    let tokens = number_tokens(suffix);
    let Some(first) = tokens.first() else {
        return false;
    };
    suffix[..first.start]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
        && (first.text.bytes().all(|byte| byte.is_ascii_digit())
            || matches!(
                first.lowercase.as_str(),
                "point" | "oh" | "o" | "half" | "halves" | "quarter" | "quarters"
            )
            || text2digits(first.text, &Language::english()).is_ok()
            || first
                .lowercase
                .strip_suffix('s')
                .is_some_and(|singular| text2digits(singular, &Language::english()).is_ok()))
}

/// Preserve a numeric continuation or a measurement attached to a long stem.
///
/// # Arguments
/// * `prefix` - Existing uppercase identifier stem.
/// * `suffix` - Text immediately following its numeric groups.
///
/// # Returns
/// Whether this group must remain separate from its stem.
///
/// # Panics
/// Panics only if a built-in regex cannot compile.
pub(super) fn identifier_group_continues(prefix: &str, suffix: &str) -> bool {
    let stem = number_tokens(prefix);
    number_group_continues(suffix)
        || model_label(&stem, 0) && !model_reference_tail(&number_tokens(suffix), 0)
        || prefix.len() >= 3
            && suffix.strip_prefix('-').is_some_and(|tail| {
                let word = tail
                    .split(|ch: char| !ch.is_ascii_alphabetic())
                    .next()
                    .unwrap_or("");
                model_measurement_unit(word)
            })
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
