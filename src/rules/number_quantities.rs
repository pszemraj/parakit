//! Quantity renderers and connected-list preservation over complete source spans.

use std::ops::Range;

use super::number_words::{horizontal_gap, list_joiner, scale};
use super::numbers::{identifier_stem, NumberKind, NumberSpan, NumberToken, Replacement};

/// A parsed coefficient and its complete scale/suffix extent in original tokens.
struct Quantity<'a> {
    span: &'a NumberSpan,
    scale_end: usize,
    end: usize,
    value: f64,
    tail: bool,
    has_scale: bool,
}

fn quantities<'a>(tokens: &[NumberToken<'_>], spans: &'a [NumberSpan]) -> Vec<Quantity<'a>> {
    spans
        .iter()
        .map(|span| {
            let mut scale_end = span.end;
            let mut value = span.value;
            if !span.ordinal {
                for magnitude in tokens[span.end..].iter().map_while(|t| scale(&t.lowercase)) {
                    value *= magnitude;
                    scale_end += 1;
                }
            }
            let end = scale_end
                + usize::from(
                    !span.ordinal
                        && tokens
                            .get(scale_end)
                            .is_some_and(|token| matches!(token.text, "K" | "M" | "B")),
                );
            let tail = scale(&tokens[span.start].lowercase).is_some()
                && spans
                    .iter()
                    .any(|previous| previous.end == span.start && !previous.ordinal);
            let has_scale = end > span.end
                || tokens[span.start..span.end]
                    .iter()
                    .any(|token| scale(&token.lowercase).is_some());
            Quantity {
                span,
                scale_end,
                end,
                value,
                tail,
                has_scale,
            }
        })
        .collect()
}

/// Propose quantity renderings and preserve connected counts as complete phrases.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Original tokens and byte boundaries.
/// * `spans` - Parsed quantities, with enabled decimals separated from surrounding counts.
/// * `threshold` - Minimum spoken quantity to render.
/// * `enabled` - Enabled numeric renderers.
/// * `proposals` - Existing typed proposals, extended with quantity renderings.
///
/// # Returns
/// Connected ranges whose original wording must be retained.
///
/// # Panics
/// Panics if supplied span boundaries do not index the supplied tokens.
pub(super) fn classify(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    threshold: f64,
    enabled: &[NumberKind],
    proposals: &mut Vec<Replacement>,
) -> Vec<Range<usize>> {
    let quantities = quantities(tokens, spans);
    for quantity in &quantities {
        let span = quantity.span;
        let words = &tokens[span.start..span.end];
        let bare_scale = scale(&words[0].lowercase).is_some();
        let tail = quantity.tail;
        let article = span.start > 0 && tokens[span.start - 1].lowercase == "a";
        let mut start = span.start;
        let end = quantity.scale_end;
        let mut text = span.text.clone();
        for token in &tokens[span.end..end] {
            text.push(' ');
            text.push_str(&token.lowercase);
        }
        let value = quantity.value;
        let scaled = end > span.end;
        let scales: Vec<_> = words
            .iter()
            .filter_map(|word| scale(&word.lowercase).map(|value| (word.lowercase.as_str(), value)))
            .collect();
        let large: Vec<_> = scales
            .iter()
            .filter(|(name, _)| matches!(*name, "million" | "billion"))
            .collect();
        if !scaled
            && large.len() == 1
            && words
                .last()
                .is_some_and(|word| matches!(word.lowercase.as_str(), "million" | "billion"))
            && (!bare_scale || end > span.start + 1)
        {
            text = format!("{} {}", span.value / large[0].1, large[0].0);
        }
        let eligible = !span.literal
            && !tail
            && !words.iter().any(|word| word.lowercase == "point")
            && (!bare_scale || end > span.start + 1)
            && span.value >= threshold;
        let rendered = span.literal || eligible && enabled.contains(&NumberKind::Spoken);
        // A cardinal takes its article as the coefficient ("a thousand five
        // hundred"); an ordinal keeps it as a determiner ("a 101st time").
        if eligible
            && article
            && bare_scale
            && !span.ordinal
            && tokens
                .get(end)
                .is_none_or(|token| !matches!(token.lowercase.as_str(), "second" | "seconds"))
        {
            start -= 1;
        }
        // The later bound's own scale words sit between it and "parameters".
        // A trillion bound keeps its wording, so its range stays as spoken.
        let parameter_range = quantities.iter().any(|next| {
            next.span.start > end
                && !tokens[next.span.start..next.scale_end]
                    .iter()
                    .any(|token| token.lowercase == "trillion")
                && tokens.get(next.scale_end).is_some_and(|token| {
                    matches!(token.lowercase.as_str(), "parameter" | "parameters")
                })
                && tokens[end..next.span.start].iter().all(|token| {
                    matches!(
                        token.lowercase.as_str(),
                        "up" | "to" | "through" | "and" | "maybe" | "about" | "roughly"
                    )
                })
        });
        if rendered
            && !span.ordinal
            && !tokens[span.start..end]
                .iter()
                .any(|token| token.lowercase == "trillion")
            && !(span.literal
                && tokens
                    .get(span.end)
                    .is_some_and(|token| token.lowercase == "hundred"))
            && (parameter_range
                || tokens.get(end).is_some_and(|token| {
                    matches!(token.lowercase.as_str(), "parameter" | "parameters")
                }))
        {
            // A decimal coefficient keeps its stated precision and takes its
            // one scale's suffix, as before an explicit suffix: 1.55B, not 1550M.
            let stated = SIZE_SUFFIXES
                .iter()
                .find(|(magnitude, _)| {
                    span.text.contains('.')
                        && end == span.end + 1
                        && scale(&tokens[span.end].lowercase) == Some(*magnitude)
                })
                .and_then(|&(magnitude, suffix)| {
                    size_suffix_coefficient(span, tokens, end, magnitude as u128)
                        .map(|coefficient| format!("{coefficient}{suffix}"))
                });
            if let Some(compact) = stated.or_else(|| compact_size(value)) {
                proposals.push(Replacement {
                    start,
                    end,
                    text: compact,
                    kind: NumberKind::Size,
                });
            }
        }
        if quantity.end > quantity.scale_end
            && !span.ordinal
            && !tail
            && (rendered
                || bare_scale && span.value >= threshold && enabled.contains(&NumberKind::Spoken))
        {
            let suffix = tokens[end].text;
            let suffix_scale = match suffix {
                "K" => 1_000,
                "M" => 1_000_000,
                "B" => 1_000_000_000,
                _ => unreachable!("size suffix checked above"),
            };
            if let Some(coefficient) = size_suffix_coefficient(span, tokens, end, suffix_scale) {
                proposals.push(Replacement {
                    start: span.start - usize::from(article && bare_scale),
                    end: quantity.end,
                    text: format!("{coefficient}{suffix}"),
                    kind: NumberKind::Size,
                });
            }
        }
        if rendered
            && identifier_stem(input, tokens, span.start)
            && !span.ordinal
            && !quantities.iter().any(|next| {
                next.span.start == span.end
                    && !next.span.ordinal
                    && !proposals.iter().any(|proposal| {
                        enabled.contains(&NumberKind::Year)
                            && proposal.kind == NumberKind::Year
                            && proposal.start == next.span.start
                    })
            })
        {
            let stem = &tokens[span.start - 1];
            if span
                .text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
            {
                proposals.push(Replacement {
                    start: span.start - 1,
                    end: span.end,
                    text: format!("{}{}", stem.text, span.text),
                    kind: NumberKind::Identifier,
                });
            }
        }
        if eligible
            || span.literal
                && span.start > 0
                && matches!(
                    tokens[span.start - 1].lowercase.as_str(),
                    "minus" | "negative"
                )
        {
            proposals.push(Replacement {
                start,
                end,
                text,
                kind: NumberKind::Spoken,
            });
        }
    }
    connected(input, tokens, &quantities, threshold, enabled, proposals)
}

fn connected(
    input: &str,
    tokens: &[NumberToken<'_>],
    quantities: &[Quantity<'_>],
    threshold: f64,
    enabled: &[NumberKind],
    proposals: &[Replacement],
) -> Vec<Range<usize>> {
    // Decimal/version and year renderers keep their independent controls.
    let typed = |quantity: &Quantity<'_>| {
        proposals.iter().any(|proposal| {
            matches!(proposal.kind, NumberKind::Version | NumberKind::Year)
                && enabled.contains(&proposal.kind)
                && proposal.start <= quantity.span.start
                && quantity.span.start < proposal.end
        })
    };
    let members: Vec<_> = quantities
        .iter()
        .filter(|quantity| !quantity.tail)
        .collect();
    let joined = |left: &Quantity<'_>, right: &Quantity<'_>| {
        left.span.ordinal == right.span.ordinal
            && left.end <= right.span.start
            && (if left.end == right.span.start {
                horizontal_gap(
                    input,
                    tokens[left.end - 1].end,
                    tokens[right.span.start].start,
                )
            } else {
                list_joiner(tokens, left.end, right.span.start)
            })
            && !tokens[left.span.start..left.end]
                .iter()
                .any(|t| t.lowercase == "point")
            && !tokens[right.span.start..right.end]
                .iter()
                .any(|t| t.lowercase == "point")
            && !typed(left)
            && !typed(right)
    };
    let mut protected = Vec::new();
    let mut at = 0;
    while at < members.len() {
        let mut end = at + 1;
        while end < members.len() && joined(members[end - 1], members[end]) {
            end += 1;
        }
        let run = &members[at..end];
        // Two comma-separated values may belong to separate prose clauses.
        let comma_pair = run.len() == 2
            && matches!(&tokens[run[0].end..run[1].span.start], [comma] if comma.text == ",");
        // A following scale can apply to both bounds. Inspect the complete
        // quantity, including scales outside the parser span and K/M/B.
        let shared_scale = run.windows(2).any(|pair| {
            !pair[0].span.ordinal
                && (!pair[0].span.literal || !pair[1].span.literal)
                && pair[0].end < pair[1].span.start
                && scale(&tokens[pair[1].span.start].lowercase).is_none()
                && !pair[0].has_scale
                && pair[1].has_scale
        });
        if run.len() > 1
            && !comma_pair
            && (shared_scale
                || run
                    .iter()
                    .any(|quantity| !quantity.span.literal && quantity.span.value < threshold))
        {
            protected.push(run[0].span.start..run[run.len() - 1].end);
        }
        at = end;
    }
    protected
}

fn size_suffix_coefficient(
    span: &NumberSpan,
    tokens: &[NumberToken<'_>],
    end: usize,
    suffix_scale: u128,
) -> Option<String> {
    let final_scale = scale(&tokens[end - 1].lowercase);
    let repeated = final_scale == Some(suffix_scale as f64);
    // Without a final scale the rendered number is complete: scales earlier
    // in it ("one thousand twenty four", "one hundred point five") are
    // already folded into its digits.
    if final_scale.is_none() {
        return Some(span.text.clone());
    }
    // A matching suffix abbreviates the final scale; a larger final scale
    // keeps its wording. A decimal before another scale fails the parse
    // below and stays spaced.
    if final_scale.is_some_and(|value| value > suffix_scale as f64) {
        return None;
    }
    let appended_end = end - usize::from(repeated && span.end < end);
    if repeated && span.end == appended_end && span.end < end {
        return Some(span.text.clone());
    }
    // Render from exact parsed digits, never from the floating-point value
    // used for threshold comparisons and general magnitude classification.
    let mut coefficient = span.text.parse::<u128>().ok()?;
    for token in &tokens[span.end..appended_end] {
        coefficient = coefficient.checked_mul(scale(&token.lowercase)? as u128)?;
    }
    if repeated && span.end == end {
        if coefficient % suffix_scale != 0 {
            return None;
        }
        coefficient /= suffix_scale;
    }
    Some(coefficient.to_string())
}

/// Compact size magnitudes and their suffixes, largest first.
const SIZE_SUFFIXES: [(f64, char); 3] = [(1e9, 'B'), (1e6, 'M'), (1e3, 'K')];

fn compact_size(value: f64) -> Option<String> {
    for (scale, suffix) in SIZE_SUFFIXES {
        if value >= scale && value % (scale / 10.0) == 0.0 {
            return Some(format!("{}{suffix}", value / scale));
        }
    }
    None
}
