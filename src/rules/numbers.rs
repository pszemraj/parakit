//! Parse original numeric spans, classify them, then render without re-reading output.

use super::number_words::{number_word, scale};
use std::borrow::Cow;
use std::ops::Range;
use text2num::{find_numbers, Language, Token};

/// Independently configurable numeric renderers sharing one parsed transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NumberKind {
    Spoken,
    Version,
    Year,
    Identifier,
    Size,
}
impl NumberKind {
    /// # Returns
    /// The configuration and reporting name for this numeric type.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Spoken => "spoken-numbers",
            Self::Version => "spoken-versions",
            Self::Year => "calendar-years",
            Self::Identifier => "numeric-identifiers",
            Self::Size => "size-suffixes",
        }
    }
}

/// A token and its byte boundaries in the original transcript.
#[derive(Debug)]
pub(super) struct NumberToken<'a> {
    pub(super) text: &'a str,
    pub(super) lowercase: String,
    pub(super) start: usize,
    pub(super) end: usize,
}
impl Token for &NumberToken<'_> {
    fn text(&self) -> Cow<'_, str> {
        self.text.into()
    }
    fn text_lowercase(&self) -> Cow<'_, str> {
        // text2num strips a trailing s even from nouns ("ones", "OSS").
        // Zero aliases belong only inside an explicitly recognized structure.
        if number_word(&self.lowercase) || self.text.parse::<f64>().is_ok() {
            self.lowercase.as_str().into()
        } else {
            "~".into()
        }
    }
}

/// One parsed value with start/end indexing the original token array.
#[derive(Clone, Debug)]
pub(super) struct NumberSpan {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) text: String,
    pub(super) value: f64,
    pub(super) ordinal: bool,
    pub(super) literal: bool,
}

/// A typed rendering proposal whose range indexes original tokens.
#[derive(Debug)]
pub(super) struct Replacement {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) text: String,
    pub(super) kind: NumberKind,
}

/// Classify and render enabled numeric types without re-reading rendered text.
///
/// # Arguments
/// * `input` - Original transcript text.
/// * `threshold` - Minimum value rendered by the general spoken-number rule.
/// * `enabled` - Numeric categories enabled in the compiled cleaner.
///
/// # Returns
/// Rendered text and per-type counts, retaining text outside accepted spans.
///
/// # Panics
/// Panics only if the built-in token regex cannot compile.
pub(super) fn normalize<'a>(
    input: &'a str,
    threshold: f64,
    enabled: &[NumberKind],
) -> (Cow<'a, str>, Vec<(NumberKind, usize)>) {
    let re = regex_static!(
        r"[A-Za-z]+|[0-9]+(?:\.[0-9]+)*|[^\t ]",
        "numeric token regex"
    );
    let tokens: Vec<_> = re
        .find_iter(input)
        .map(|found| NumberToken {
            text: found.as_str(),
            lowercase: found.as_str().to_ascii_lowercase(),
            start: found.start(),
            end: found.end(),
        })
        .collect();
    if tokens.is_empty() {
        return (Cow::Borrowed(input), Vec::new());
    }
    let language = Language::english();
    let (initial, mut protected) = parse_spans(&tokens, &language);
    protected.extend(super::number_guards::protected(
        &tokens, &initial, &language,
    ));
    let mut spans = Vec::new();
    let mut start = 0;
    // Reparse around protected ranges on the same tokens, so a fractional
    // numerator cannot absorb an adjacent independent exact quantity.
    let mut ambiguous = Vec::new();
    for end in 0..=tokens.len() {
        if end == tokens.len() || protected.iter().any(|range| range.contains(&end)) {
            if start < end {
                let (mut parsed, ranges) = parse_spans(&tokens[start..end], &language);
                for span in &mut parsed {
                    span.start += start;
                    span.end += start;
                }
                spans.extend(parsed);
                ambiguous.extend(ranges.into_iter().map(|r| r.start + start..r.end + start));
            }
            start = end + 1;
        }
    }
    protected.extend(ambiguous);
    let mut proposals = super::number_versions::classify(input, &tokens, &spans, &language);
    proposals.extend(super::number_labels::classify(
        input, &tokens, &spans, &language,
    ));
    // Version boundaries split parser spans before quantities are classified.
    // This operates on original tokens, including a remaining scale coefficient.
    for proposal in proposals
        .iter_mut()
        .filter(|p| p.kind == NumberKind::Version)
    {
        apply_sign(input, &tokens, &spans, proposal);
    }
    let versions: Vec<_> = proposals
        .iter()
        .filter(|p| p.kind == NumberKind::Version && enabled.contains(&p.kind))
        .collect();
    let mut quantities = if versions.is_empty() {
        spans.clone()
    } else {
        Vec::new()
    };
    let mut position = 0;
    for version in &versions {
        if position < version.start {
            quantities.extend(parsed_slice(&tokens, position..version.start, &language));
        }
        position = version.end;
        if let Ok(value) = version.text.parse::<f64>() {
            let mut end = version.end;
            while tokens
                .get(end)
                .is_some_and(|token| scale(&token.lowercase).is_some())
            {
                end += 1;
            }
            quantities.push(NumberSpan {
                start: version.start,
                end: version.end,
                text: version.text.clone(),
                value,
                ordinal: false,
                literal: true,
            });
            position = end;
        }
    }
    if !versions.is_empty() && position < tokens.len() {
        quantities.extend(parsed_slice(&tokens, position..tokens.len(), &language));
    }
    quantities.retain(|span| {
        !protected
            .iter()
            .any(|r| r.start < span.end && span.start < r.end)
    });
    quantities.sort_by_key(|span| span.start);
    for span in &quantities {
        let words = &tokens[span.start..span.end];
        let bare_scale = scale(&words[0].lowercase).is_some();
        let article = span.start > 0 && tokens[span.start - 1].lowercase == "a";
        let mut start = span.start;
        let mut end = span.end;
        let mut text = span.text.clone();
        let mut value = span.value;
        let mut scaled = false;
        while tokens
            .get(end)
            .is_some_and(|token| scale(&token.lowercase).is_some())
        {
            let magnitude = &tokens[end].lowercase;
            text.push(' ');
            text.push_str(magnitude);
            value *= scale(magnitude).unwrap();
            end += 1;
            scaled = true;
        }
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
            && !words.iter().any(|word| word.lowercase == "point")
            && (!bare_scale || end > span.start + 1)
            && span.value >= threshold;
        let rendered = span.literal || eligible && enabled.contains(&NumberKind::Spoken);
        if eligible
            && article
            && bare_scale
            && tokens
                .get(end)
                .is_none_or(|token| !matches!(token.lowercase.as_str(), "second" | "seconds"))
        {
            start -= 1;
        }
        let parameter_range = quantities.iter().any(|next| {
            next.start > end
                && tokens.get(next.end).is_some_and(|token| {
                    matches!(token.lowercase.as_str(), "parameter" | "parameters")
                })
                && tokens[end..next.start].iter().all(|token| {
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
            if let Some(compact) = compact_size(value) {
                proposals.push(Replacement {
                    start,
                    end,
                    text: compact,
                    kind: NumberKind::Size,
                });
            }
        }
        if tokens
            .get(end)
            .is_some_and(|token| matches!(token.text, "K" | "M" | "B"))
            && !span.ordinal
            && (rendered
                || bare_scale && span.value >= threshold && enabled.contains(&NumberKind::Spoken))
        {
            proposals.push(Replacement {
                start: span.start - usize::from(article && bare_scale),
                end: end + 1,
                text: format!(
                    "{}{}",
                    text.split_whitespace().next().unwrap(),
                    tokens[end].text
                ),
                kind: NumberKind::Size,
            });
        }
        if rendered
            && identifier_stem(input, &tokens, span.start)
            && !span.ordinal
            && !quantities.iter().any(|next| {
                next.start == span.end
                    && !next.ordinal
                    && !proposals.iter().any(|proposal| {
                        enabled.contains(&NumberKind::Year)
                            && proposal.kind == NumberKind::Year
                            && proposal.start == next.start
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
    // A low spoken member makes an adjacent run ambiguous as a whole.
    // A decimal/version or a year remains its own explicitly classified type.
    let mut mixed = Vec::new();
    let mut at = 0;
    while at < spans.len() {
        let mut end = at + 1;
        while end < spans.len()
            && !spans[end - 1].ordinal
            && !spans[end].ordinal
            && spans[end - 1].end == spans[end].start
            && input[tokens[spans[end - 1].end - 1].end..tokens[spans[end].start].start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t'))
            && !tokens[spans[end - 1].start..spans[end - 1].end]
                .iter()
                .any(|t| t.lowercase == "point")
            && !tokens[spans[end].start..spans[end].end]
                .iter()
                .any(|t| t.lowercase == "point")
        {
            end += 1;
        }
        if end > at + 1
            && spans[at..end]
                .iter()
                .any(|span| !span.literal && span.value < threshold)
        {
            mixed.push(spans[at].start..spans[end - 1].end);
        }
        at = end;
    }
    // Unknown zero aliases stay as a complete original run. Only an explicit
    // version or a recognized century pair may interpret the alias as zero.
    for (index, pair) in spans.windows(2).enumerate() {
        if pair[0].end >= pair[1].start
            || !tokens[pair[0].end..pair[1].start]
                .iter()
                .all(|token| matches!(token.lowercase.as_str(), "oh" | "o"))
        {
            continue;
        }
        let mut first = index;
        let mut last = index + 1;
        while first > 0 && spans[first - 1].end == spans[first].start {
            first -= 1;
        }
        while last + 1 < spans.len() && spans[last].end == spans[last + 1].start {
            last += 1;
        }
        mixed.push(spans[first].start..spans[last].end);
    }
    // Include leading aliases too ("double oh seven", "oh oh seven").
    for span in &spans {
        let mut start = span.start;
        while start > 0 && matches!(tokens[start - 1].lowercase.as_str(), "oh" | "o") {
            start -= 1;
        }
        if start < span.start {
            mixed.push(start..span.end);
        }
    }
    // A bare point is prose, not permission to convert only its trailing digits.
    for (at, token) in tokens.iter().enumerate() {
        if token.lowercase == "point"
            && !proposals
                .iter()
                .any(|p| p.kind == NumberKind::Version && p.start < at && at < p.end)
        {
            let mut end = at + 1;
            while end < tokens.len() && number_word(&tokens[end].lowercase) {
                end += 1;
            }
            protected.push(at..end);
        }
        // Scientific notation is not a supported numeric structure. Keep the
        // entire expression readable instead of converting only its exponent.
        if token.lowercase == "e" && at > 0 {
            let exponent = at
                + 1
                + usize::from(tokens.get(at + 1).is_some_and(|t| {
                    matches!(t.lowercase.as_str(), "minus" | "negative" | "plus")
                }));
            if let (Some(base), Some(power)) = (
                spans.iter().find(|s| s.end == at),
                spans.iter().find(|s| s.start == exponent),
            ) {
                protected.push(base.start..power.end);
            }
        }
    }
    // Spoken decimal versions participate in the same identifier join as
    // literal decimals, while keeping their attribution to spoken-versions.
    for proposal in &mut proposals {
        if enabled.contains(&NumberKind::Identifier)
            && proposal.kind == NumberKind::Version
            && identifier_stem(input, &tokens, proposal.start)
            && !spans
                .iter()
                .any(|next| next.start == proposal.end && !next.ordinal)
        {
            proposal.start -= 1;
            proposal.text.insert_str(0, tokens[proposal.start].text);
        }
    }
    // Literal dotted versions need no f64 value to compact a prefix.
    for (at, token) in tokens.iter().enumerate().skip(1) {
        let stem = &tokens[at - 1];
        if token.text.starts_with(|ch: char| ch.is_ascii_digit())
            && token.text.contains('.')
            && !spans
                .iter()
                .any(|next| next.start == at + 1 && !next.ordinal)
            && identifier_stem(input, &tokens, at)
        {
            proposals.push(Replacement {
                start: at - 1,
                end: at + 1,
                text: format!("{}{}", stem.text, token.text),
                kind: NumberKind::Identifier,
            });
        }
    }
    for proposal in proposals
        .iter_mut()
        .filter(|proposal| matches!(proposal.kind, NumberKind::Size | NumberKind::Spoken))
    {
        apply_sign(input, &tokens, &spans, proposal);
    }
    let priority = |kind| match kind {
        NumberKind::Size => 5,
        NumberKind::Version => 4,
        NumberKind::Year => 3,
        NumberKind::Identifier => 2,
        NumberKind::Spoken => 1,
    };
    proposals.sort_by_key(|p| {
        (
            std::cmp::Reverse(priority(p.kind)),
            p.start,
            std::cmp::Reverse(p.end),
        )
    });
    let mut accepted: Vec<Replacement> = Vec::new();
    for proposal in proposals {
        if !enabled.contains(&proposal.kind)
            || !matches!(proposal.kind, NumberKind::Version | NumberKind::Year)
                && mixed
                    .iter()
                    .any(|range| range.start < proposal.end && proposal.start < range.end)
            || protected
                .iter()
                .any(|r| r.start < proposal.end && proposal.start < r.end)
            || accepted
                .iter()
                .any(|p| p.start < proposal.end && proposal.start < p.end)
        {
            continue;
        }
        accepted.push(proposal);
    }
    accepted.sort_by_key(|p| p.start);
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut counts: Vec<(NumberKind, usize)> = Vec::new();
    for proposal in accepted {
        let start = tokens[proposal.start].start;
        let end = tokens[proposal.end - 1].end;
        output.push_str(&input[cursor..start]);
        output.push_str(&proposal.text);
        cursor = end;
        if input[start..end] != proposal.text {
            if let Some((_, count)) = counts.iter_mut().find(|(kind, _)| *kind == proposal.kind) {
                *count += 1;
            } else {
                counts.push((proposal.kind, 1));
            }
        }
    }
    output.push_str(&input[cursor..]);
    (
        if output == input {
            Cow::Borrowed(input)
        } else {
            Cow::Owned(output)
        },
        counts,
    )
}

fn apply_sign(
    input: &str,
    tokens: &[NumberToken<'_>],
    spans: &[NumberSpan],
    proposal: &mut Replacement,
) {
    let start = proposal.start;
    if start == 0
        || proposal.text.starts_with('v')
        || !matches!(tokens[start - 1].lowercase.as_str(), "minus" | "negative")
        || start >= 2
            && (tokens[start - 2].text == "-" || spans.iter().any(|span| span.end == start - 1))
        || !input[tokens[start - 1].end..tokens[start].start]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return;
    }
    proposal.start -= 1;
    proposal.text.insert(0, '-');
}

/// # Arguments
/// * `input` - Original transcript text.
/// * `tokens` - Original tokens and byte boundaries.
/// * `start` - Index of the numeric token after the possible stem.
///
/// # Returns
/// Whether a token has an uppercase identifier stem immediately before it.
///
/// # Panics
/// Panics if `start` or token boundaries do not index the supplied input.
pub(super) fn identifier_stem(input: &str, tokens: &[NumberToken<'_>], start: usize) -> bool {
    start > 0 && {
        let stem = &tokens[start - 1];
        (stem.text == "SM"
            || stem.text.len() == 1
                && stem.text.bytes().all(|byte| byte.is_ascii_uppercase())
                && !matches!(stem.text, "A" | "I"))
            && stem.end <= tokens[start].start
            && input[stem.end..tokens[start].start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t'))
    }
}

fn compact_size(value: f64) -> Option<String> {
    for (scale, suffix) in [(1e9, 'B'), (1e6, 'M'), (1e3, 'K')] {
        if value >= scale && value % (scale / 10.0) == 0.0 {
            return Some(format!("{}{suffix}", value / scale));
        }
    }
    None
}

fn parsed_slice(
    tokens: &[NumberToken<'_>],
    range: Range<usize>,
    language: &Language,
) -> Vec<NumberSpan> {
    let (mut spans, _) = parse_spans(&tokens[range.clone()], language);
    for span in &mut spans {
        span.start += range.start;
        span.end += range.start;
    }
    spans
}

fn parse_range(
    tokens: &[NumberToken<'_>],
    range: Range<usize>,
    language: &Language,
) -> Vec<NumberSpan> {
    let mut spans: Vec<_> = find_numbers(tokens[range.clone()].iter(), language, 0.0)
        .into_iter()
        .map(|n| {
            let zero_scale = tokens[range.start + n.start].lowercase == "zero"
                && tokens
                    .get(range.start + n.start + 1)
                    .is_some_and(|t| scale(&t.lowercase).is_some())
                && tokens[range.start + n.start + 1..range.start + n.end]
                    .iter()
                    .all(|t| scale(&t.lowercase).is_some());
            NumberSpan {
                start: range.start + n.start,
                end: range.start + n.end,
                text: if zero_scale { "0".to_string() } else { n.text },
                value: if zero_scale { 0.0 } else { n.value },
                ordinal: n.is_ordinal,
                literal: false,
            }
        })
        .collect();
    for (at, token) in tokens.iter().enumerate().take(range.end).skip(range.start) {
        if token.text.starts_with(|ch: char| ch.is_ascii_digit())
            && !spans.iter().any(|n| n.start <= at && at < n.end)
        {
            if let Ok(value) = token.text.parse::<f64>() {
                spans.push(NumberSpan {
                    start: at,
                    end: at + 1,
                    text: token.text.into(),
                    value,
                    ordinal: false,
                    literal: true,
                });
            }
        }
    }
    spans.sort_by_key(|span| span.start);
    spans
}

fn parse_spans(
    tokens: &[NumberToken<'_>],
    language: &Language,
) -> (Vec<NumberSpan>, Vec<Range<usize>>) {
    let mut boundaries = vec![0, tokens.len()];
    for (at, token) in tokens.iter().enumerate() {
        if matches!(token.text, "\n" | "\r") {
            boundaries.extend([at, at + 1]);
        }
    }
    // An equal scale after "and" starts a separate quantity. Establish
    // these boundaries before a greedy parse can absorb the next coefficient.
    for (at, pair) in tokens.windows(2).enumerate() {
        let Some(rank) = scale(&pair[0].lowercase) else {
            continue;
        };
        if pair[1].lowercase != "and" {
            continue;
        }
        let limit = tokens[at + 2..]
            .iter()
            .position(|token| !number_word(&token.lowercase) && token.text.parse::<f64>().is_err())
            .map_or(tokens.len(), |end| at + 2 + end);
        let following = &tokens[at + 2..limit];
        let Some(next) = find_numbers(following.iter(), language, 0.0)
            .into_iter()
            .next()
        else {
            continue;
        };
        if next.start != 0 {
            continue;
        }
        let lead = (1..next.end)
            .find(|&index| {
                following[index].lowercase == "and"
                    && scale(&following[index - 1].lowercase).is_none()
            })
            .unwrap_or(next.end);
        let decimal = following[..lead]
            .iter()
            .any(|token| token.lowercase == "point");
        let end = if decimal && lead == next.end {
            (lead + 1).min(following.len())
        } else {
            lead
        };
        if following[..end]
            .iter()
            .filter_map(|token| scale(&token.lowercase))
            .fold(0.0, f64::max)
            == rank
        {
            boundaries.push(at + 2);
        }
    }
    for _ in 0..4 {
        let mut protected = Vec::new();
        boundaries.sort_unstable();
        boundaries.dedup();
        let spans: Vec<_> = boundaries
            .windows(2)
            .flat_map(|w| parse_range(tokens, w[0]..w[1], language))
            .collect();
        let mut added = Vec::new();
        for span in &spans {
            for at in span.start + 1..span.end.saturating_sub(1) {
                if tokens[at].lowercase == "and" && scale(&tokens[at - 1].lowercase).is_none() {
                    added.push(at + 1);
                }
            }
        }
        for pair in spans.windows(2) {
            let (count, next) = (&pair[0], &pair[1]);
            if count.end + 1 == next.start && tokens[count.end].lowercase == "and" {
                let before = tokens[count.start..count.end]
                    .iter()
                    .filter_map(|t| scale(&t.lowercase))
                    .fold(0.0, f64::max);
                let after = tokens[next.start..next.end]
                    .iter()
                    .filter_map(|t| scale(&t.lowercase))
                    .fold(0.0, f64::max);
                if before > 0.0 && before == after {
                    added.push(next.start);
                }
            }
            if count.end != next.start
                || count.ordinal
                || next.ordinal
                || count.literal
                || scale(&tokens[next.start].lowercase).is_none()
            {
                continue;
            }
            if tokens[count.start..count.end]
                .iter()
                .any(|t| t.lowercase == "point")
            {
                continue;
            }
            if scale(&tokens[count.end - 1].lowercase).is_some()
                && tokens[count.start..count.end]
                    .iter()
                    .filter_map(|token| scale(&token.lowercase))
                    .fold(0.0, f64::max)
                    < scale(&tokens[next.start].lowercase).unwrap()
            {
                protected.push(count.start..next.end);
                continue;
            }
            let splits: Vec<_> = (count.start + 1..count.end)
                .filter(|&at| {
                    let head = at - usize::from(tokens[at - 1].lowercase == "and");
                    scale(&tokens[at].lowercase).is_none()
                        && !matches!(tokens[at].text, "-" | "and")
                        && tokens[at - 1].text != "-"
                        && one_cardinal(&tokens[count.start..head], language)
                        && one_cardinal(&tokens[at..next.end], language)
                })
                .collect();
            if let [at] = splits.as_slice() {
                added.push(*at);
            } else {
                protected.push(count.start..next.end);
            }
        }
        added.retain(|at| !boundaries.contains(at));
        if added.is_empty() {
            return (spans, protected);
        }
        boundaries.extend(added);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    (
        boundaries
            .windows(2)
            .flat_map(|w| parse_range(tokens, w[0]..w[1], language))
            .collect(),
        Vec::new(),
    )
}

fn one_cardinal(tokens: &[NumberToken<'_>], language: &Language) -> bool {
    matches!(find_numbers(tokens.iter(), language, 0.0).as_slice(), [number] if number.start == 0 && number.end == tokens.len() && !number.is_ordinal)
}
