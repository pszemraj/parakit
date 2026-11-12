//! Dotted versions and decimal components over the shared number tokens.

use text2num::{text2digits, Language};

use super::number_words::scale;
use super::numbers::{NumberKind, NumberSpan, NumberToken, Replacement};

/// Propose dotted numeric expressions without changing the original tokens.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Shared token text and byte boundaries.
/// * `spans` - Number expressions already identified in the shared tokens.
/// * `language` - Number parser used for every spoken component.
///
/// # Returns
/// Version replacements expressed in token indexes.
///
/// # Panics
/// Panics if supplied span boundaries do not index the supplied tokens.
pub(super) fn classify(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    language: &Language,
) -> Vec<Replacement> {
    let mut replacements = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        let token = &tokens[cursor];
        let starts_number =
            (token.text != "-" && token.lowercase != "and" && number_word(token, language))
                || literal(token.text)
                || matches!(token.lowercase.as_str(), "oh" | "o")
                || spans
                    .iter()
                    .any(|span| span.start == cursor && !span.ordinal);
        let candidate = starts_number
            .then(|| candidate(input, tokens, spans, cursor, language))
            .flatten();
        let Some((end, mut text)) = candidate else {
            cursor += 1;
            continue;
        };
        let mut start = cursor;
        if cursor > 0
            && tokens[cursor - 1].lowercase == "v"
            && spaced(input, &tokens[cursor - 1], token)
        {
            start -= 1;
            text.insert(0, 'v');
        }
        replacements.push(Replacement {
            start,
            end,
            text,
            kind: NumberKind::Version,
        });
        cursor = end;
    }
    replacements
}

fn candidate(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    start: usize,
    language: &Language,
) -> Option<(usize, String)> {
    let mut literal_end = start;
    if literal(tokens[start].text) {
        literal_end += 1;
        while literal_end + 1 < tokens.len()
            && tokens[literal_end].text == "."
            && integer(tokens[literal_end + 1].text)
            && tokens[literal_end - 1].end == tokens[literal_end].start
            && tokens[literal_end].end == tokens[literal_end + 1].start
        {
            literal_end += 2;
        }
    }
    let run_end = word_run_end(input, tokens, literal_end.max(start + 1));
    let point = if literal_end > start {
        literal_end
    } else {
        (start + 1..run_end).find(|&index| tokens[index].lowercase == "point")?
    };
    if tokens
        .get(point)
        .is_none_or(|token| token.lowercase != "point")
    {
        let text = &input[tokens[start].start..tokens[literal_end.checked_sub(1)?].end];
        return (text.contains('.')
            && start > 0
            && tokens[start - 1].lowercase == "v"
            && spaced(input, &tokens[start - 1], &tokens[start]))
        .then(|| (literal_end, text.to_string()));
    }
    if point == start
        || !spaced(input, &tokens[point - 1], &tokens[point])
        || !tokens
            .get(point + 1)
            .is_some_and(|next| spaced(input, &tokens[point], next))
    {
        return None;
    }
    // The shared parser has already separated independent counts, including
    // "fifty and two point five". A version base cannot join them again.
    if spans
        .iter()
        .any(|span| start < span.start && span.start < point)
    {
        return None;
    }
    let base = if literal_end > start {
        input[tokens[start].start..tokens[literal_end - 1].end].to_string()
    } else {
        component(&tokens[start..point], language)?
    };
    let points: Vec<_> = (point..run_end)
        .filter(|&index| tokens[index].lowercase == "point")
        .collect();
    if points.len() > 1 {
        if let Some(version) = dotted(input, tokens, &points, run_end, &base, language) {
            return Some(version);
        }
    }
    let (end, fraction) = component_prefix(input, tokens, point + 1, run_end, false, language)?;
    Some((end, format!("{base}.{fraction}")))
}

fn dotted(
    input: &str,
    tokens: &[NumberToken<'_>],
    points: &[usize],
    run_end: usize,
    base: &str,
    language: &Language,
) -> Option<(usize, String)> {
    let mut text = base.to_string();
    for (index, &point) in points.iter().enumerate() {
        if !tokens
            .get(point + 1)
            .is_some_and(|next| spaced(input, &tokens[point], next))
        {
            return None;
        }
        let (end, value) = if let Some(&next) = points.get(index + 1) {
            if !spaced(input, &tokens[next - 1], &tokens[next]) {
                return None;
            }
            (next, component(&tokens[point + 1..next], language)?)
        } else {
            component_prefix(input, tokens, point + 1, run_end, true, language)?
        };
        text.push('.');
        text.push_str(&value);
        if index + 1 == points.len() {
            return Some((end, text));
        }
    }
    None
}

fn component_prefix(
    input: &str,
    tokens: &[NumberToken<'_>],
    start: usize,
    limit: usize,
    scales: bool,
    language: &Language,
) -> Option<(usize, String)> {
    let mut end = start;
    while end < limit {
        let word = tokens[end].lowercase.as_str();
        if word == "point"
            || scale(word).is_some_and(|value| value >= 1_000_000.0)
            || !scales && scale(word).is_some()
            || !number_word(&tokens[end], language)
        {
            break;
        }
        end += 1;
    }
    // A scale followed by "and" starts another quantity, not a build component.
    if tokens[start..end]
        .iter()
        .any(|token| token.lowercase == "and")
    {
        if let Some(index) =
            (start..end).find(|&index| scale(&tokens[index].lowercase) == Some(1_000.0))
        {
            end = index;
        }
    }
    let mut rendered = None;
    for candidate_end in start + 1..=end {
        if let Some(value) = component(&tokens[start..candidate_end], language) {
            rendered = Some((candidate_end, value));
        }
    }
    let (mut end, mut value) = rendered?;
    // In "five two thousand", the final digit belongs to the scaled count.
    if !scales
        && tokens
            .get(end)
            .is_some_and(|token| scale(&token.lowercase).is_some())
    {
        let digits = tokens[start..end]
            .iter()
            .filter(|token| token.text != "-")
            .all(|token| digit(token, language).is_some());
        if digits {
            if let Some(split) =
                (start + 1..end).find(|&index| spaced(input, &tokens[index - 1], &tokens[index]))
            {
                end = split;
                value = component(&tokens[start..end], language)?;
            }
        }
    }
    Some((end, value))
}

/// Keep digit-by-digit components, including leading zeros, in their spoken order.
fn component(tokens: &[NumberToken<'_>], language: &Language) -> Option<String> {
    let words: Vec<_> = tokens.iter().filter(|token| token.text != "-").collect();
    let first = *words.first()?;
    if words.len() == 1 && integer(first.text) {
        return Some(first.text.to_string());
    }
    if words
        .iter()
        .any(|token| scale(&token.lowercase).is_some_and(|value| value >= 1_000_000.0))
        || words
            .iter()
            .any(|token| scale(&token.lowercase) == Some(1_000.0))
            && words.iter().any(|token| token.lowercase == "and")
    {
        return None;
    }
    let digits: Option<String> = words.iter().map(|token| digit(token, language)).collect();
    if digits.is_some() || digit(first, language).as_deref() == Some("0") {
        return digits;
    }
    let phrase = words
        .iter()
        .map(|token| alias(&token.lowercase))
        .collect::<Vec<_>>()
        .join(" ");
    text2digits(&phrase, language)
        .ok()
        .filter(|value| integer(value))
}

fn digit(token: &NumberToken<'_>, language: &Language) -> Option<String> {
    let text = alias(&token.lowercase);
    if text.len() == 1 && integer(text) {
        return Some(text.to_string());
    }
    text2digits(text, language)
        .ok()
        .filter(|value| value.len() == 1 && integer(value))
}

fn alias(word: &str) -> &str {
    match word {
        "oh" | "o" => "zero",
        _ => word,
    }
}

fn number_word(token: &NumberToken<'_>, language: &Language) -> bool {
    matches!(token.text, "-")
        || token.lowercase == "and"
        || integer(token.text)
        || token.text.bytes().all(|byte| byte.is_ascii_alphabetic())
            && text2digits(alias(&token.lowercase), language).is_ok_and(|value| integer(&value))
}

fn word_run_end(input: &str, tokens: &[NumberToken<'_>], start: usize) -> usize {
    let mut end = start;
    while end < tokens.len()
        && (end == 0
            || input[tokens[end - 1].end..tokens[end].start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t')))
        && (inline_hyphen(tokens, end)
            || tokens[end]
                .text
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic())
            || literal(tokens[end].text))
    {
        end += 1;
    }
    end
}

fn spaced(input: &str, left: &NumberToken<'_>, right: &NumberToken<'_>) -> bool {
    let gap = &input[left.end..right.start];
    !gap.is_empty() && gap.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn integer(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn literal(text: &str) -> bool {
    text.split('.').all(integer)
}

fn inline_hyphen(tokens: &[NumberToken<'_>], index: usize) -> bool {
    index > 0
        && tokens[index].text == "-"
        && tokens[index - 1].end == tokens[index].start
        && tokens.get(index + 1).is_some_and(|next| {
            tokens[index].end == next.start
                && next.text.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
        && tokens[index - 1]
            .text
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
}
