//! Literal token ranges for fractions, indefinite quantities, and time units.

use std::borrow::Cow;
use std::ops::Range;
use text2num::{find_numbers, Language, Token};

use super::number_words::scale;
use super::numbers::{NumberSpan, NumberToken};

/// Find phrases whose number words must retain their original spelling.
///
/// # Arguments
/// * `tokens` - Original transcript tokens, including punctuation and line breaks.
/// * `spans` - Parsed spoken numbers and literal numeric tokens.
/// * `language` - Shared English number parser.
///
/// # Returns
/// Sorted, disjoint token ranges to exclude from numeric rendering.
pub(super) fn protected(
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    language: &Language,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let word = token.lowercase.as_str();
        if matches!(word, "second" | "seconds") && second_is_time_unit(tokens, index, language) {
            ranges.push(index..index + 1);
            continue;
        }
        if plural_magnitude(word) {
            let start = cardinal_chain(tokens, spans, index, language)
                .map_or(index, |(range, _, _)| range.start);
            ranges.push(start..following_scales(tokens, index + 1));
            continue;
        }
        if matches!(word, "few" | "several" | "couple") {
            let mut end = index + 1;
            if word == "couple" && at(tokens, end, "of") {
                end += 1;
            }
            let extended = following_scales(tokens, end);
            if extended > end {
                let start = if index > 0 && at(tokens, index - 1, "a") {
                    index - 1
                } else {
                    index
                };
                ranges.push(start..extended);
            }
            continue;
        }
        if matches!(word, "half" | "halves" | "quarter" | "quarters") {
            if let Some(range) = fraction(tokens, spans, index, None, language) {
                ranges.push(range);
            }
            continue;
        }
        if at(tokens, index + 1, "of") {
            let plural = word.ends_with('s');
            if let Some(denominator) = ordinal_ending(tokens, index + 1, plural, language) {
                if let Some(range) = fraction(tokens, spans, index, Some(denominator), language) {
                    ranges.push(range);
                }
            }
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut().filter(|last| range.start <= last.end) {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn at(tokens: &[NumberToken<'_>], index: usize, word: &str) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| token.lowercase == word)
}

fn line_start(tokens: &[NumberToken<'_>], end: usize) -> usize {
    tokens[..end]
        .iter()
        .rposition(|token| matches!(token.text, "\r" | "\n"))
        .map_or(0, |index| index + 1)
}

fn plural_magnitude(word: &str) -> bool {
    matches!(
        word,
        "tens" | "hundreds" | "thousands" | "millions" | "billions" | "trillions"
    )
}

/// Extend only across adjacent scale words and scale-to-scale hyphens.
fn following_scales(tokens: &[NumberToken<'_>], mut end: usize) -> usize {
    loop {
        let next = if at(tokens, end, "-") { end + 1 } else { end };
        if tokens
            .get(next)
            .is_none_or(|token| scale(&token.lowercase).is_none())
        {
            return end;
        }
        end = next + 1;
    }
}

fn fraction(
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    denominator: usize,
    ordinal: Option<Range<usize>>,
    language: &Language,
) -> Option<Range<usize>> {
    let word = tokens[denominator].lowercase.as_str();
    let plural = matches!(word, "halves" | "quarters") || ordinal.is_some() && word.ends_with('s');
    let mut start = ordinal.as_ref().map_or(denominator, |range| range.start);
    let mut end = denominator + 1;
    let of = at(tokens, end, "of");
    if of {
        end += 1;
    }
    let article_after = at(tokens, end, "a");
    let scales_start = end + usize::from(article_after);
    let scales_end = following_scales(tokens, scales_start);
    let has_scale = scales_end > scales_start;
    if has_scale {
        end = scales_end;
    }
    let article_before = start > 0 && at(tokens, start - 1, "a");
    if article_before {
        start -= 1;
    }
    let joined = start > 0 && at(tokens, start - 1, "and");
    if joined {
        start -= 1;
    }
    let scale_first = joined && start > 0 && scale(&tokens[start - 1].lowercase).is_some();
    if !of && !has_scale && !scale_first {
        return None;
    }
    if ordinal.is_some() && !plural && !has_scale {
        // "The fifth of May" remains an ordinal; "a fifth" and "one
        // hundredth" supply an explicit singular fraction numerator.
        let numerator =
            article_before || at(tokens, start, "one") || start > 0 && at(tokens, start - 1, "one");
        if !numerator {
            return None;
        }
    }
    let prefix_end = if start > 0 && at(tokens, start - 1, "-") {
        start - 1
    } else {
        start
    };
    if let Some((previous, value, mixed)) = cardinal_chain(tokens, spans, prefix_end, language) {
        let has_article = article_before || article_after;
        let singular = previous.len() == 1 && (!has_article || value == 1.0);
        // An article before a scale-first phrase supplies its own quantity:
        // "year two thousand a million and a half" has an independent year.
        if plural || joined || singular || mixed {
            start = previous.start;
        }
    }
    if scale_first && start > 0 && at(tokens, start - 1, "a") {
        start -= 1;
    }
    Some(start..end)
}

/// Find the trailing adjacent cardinal expressions, allowing a mixed-count
/// conjunction. Parsing a slice ends greedy spans at the protected boundary.
fn cardinal_chain(
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    end: usize,
    language: &Language,
) -> Option<(Range<usize>, f64, bool)> {
    let offset = line_start(tokens, end);
    let parsed = find_numbers(tokens[offset..end].iter(), language, 0.0);
    let mut numbers: Vec<_> = parsed
        .into_iter()
        .map(|span| {
            (
                span.start + offset..span.end + offset,
                span.value,
                span.is_ordinal,
            )
        })
        .collect();
    numbers.extend(
        spans
            .iter()
            .filter(|span| span.literal && span.start >= offset && span.end <= end)
            .map(|span| (span.start..span.end, span.value, span.ordinal)),
    );
    numbers.sort_by_key(|(range, _, _)| range.start);
    let (last, value, ordinal) = numbers.last()?;
    if *ordinal || last.end != end {
        return None;
    }
    let mut start = last.start;
    let mut mixed = false;
    for (previous, _, ordinal) in numbers.iter().rev().skip(1) {
        if *ordinal {
            break;
        }
        let adjacent = previous.end == start;
        let joined = previous.end + 1 == start && at(tokens, previous.end, "and");
        if !adjacent && !joined {
            break;
        }
        mixed |= joined;
        start = previous.start;
    }
    Some((start..end, *value, mixed))
}

/// A parser view changes only the final lowercase word for plural ordinals;
/// source text and byte boundaries remain owned by the original tokens.
struct OrdinalToken<'t, 's> {
    token: &'t NumberToken<'s>,
    singular: bool,
}

impl Token for OrdinalToken<'_, '_> {
    fn text(&self) -> Cow<'_, str> {
        self.token.text.into()
    }

    fn text_lowercase(&self) -> Cow<'_, str> {
        let word = self.token.lowercase.as_str();
        if self.singular {
            word.strip_suffix('s').unwrap_or(word).into()
        } else {
            word.into()
        }
    }
}

fn ordinal_ending(
    tokens: &[NumberToken<'_>],
    end: usize,
    plural: bool,
    language: &Language,
) -> Option<Range<usize>> {
    let offset = line_start(tokens, end);
    let view = tokens[offset..end]
        .iter()
        .enumerate()
        .map(|(index, token)| OrdinalToken {
            token,
            singular: plural && index + offset + 1 == end,
        });
    find_numbers(view, language, 0.0)
        .into_iter()
        .last()
        .filter(|span| span.is_ordinal && span.end + offset == end)
        .map(|span| span.start + offset..span.end + offset)
}

fn second_is_time_unit(tokens: &[NumberToken<'_>], index: usize, language: &Language) -> bool {
    if index == 0 {
        return false;
    }
    if at(tokens, index - 1, "-") {
        // A valid compound is an ordinal ("twenty-second"); an unrelated
        // hyphenated word leaves the unit word literal ("split-second").
        return ordinal_ending(tokens, index + 1, false, language)
            .is_none_or(|range| range.start == index);
    }
    if matches!(
        tokens[index - 1].lowercase.as_str(),
        "a" | "per" | "every" | "each"
    ) || tokens[index - 1].text.parse::<f64>().is_ok()
    {
        return true;
    }
    if ordinal_ending(tokens, index + 1, false, language).is_some_and(|range| range.start < index) {
        return false;
    }
    let offset = line_start(tokens, index);
    let previous = find_numbers(tokens[offset..index].iter(), language, 0.0);
    let Some(previous) = previous.last().filter(|span| span.end + offset == index) else {
        return false;
    };
    if !previous.is_ordinal {
        return true;
    }
    let next =
        (index + 1..tokens.len()).find(|&at| !matches!(tokens[at].lowercase.as_str(), "," | "and"));
    if next.is_some_and(|at| {
        find_numbers(tokens[at..at + 1].iter(), language, 0.0)
            .last()
            .is_some_and(|span| span.is_ordinal)
    }) {
        return false;
    }
    previous.start > 0 && at(tokens, previous.start + offset - 1, "the")
        || at(tokens, index + 1, "of")
}
