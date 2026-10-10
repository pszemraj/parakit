//! Spoken-number formatting layered around the `text2num` English parser.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;
use text2num::{find_numbers, replace_numbers_in_text, text2digits, Language, Occurence, Token};

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
/// `threshold` gates only isolated single-digit cardinals and ordinals: those below it keep their wording, and cardinals of 10 or more always convert.
/// Expressions ending in one explicit `million` or `billion` scale retain
/// that word with a numeric coefficient for readability (e.g. `"three
/// billion"` becomes `"3 billion"`). Multi-scale expressions retain
/// `text2num`'s full-digit rendering. Contexts where `"second"` is a time
/// unit rather than an ordinal, indefinite magnitudes such as `"hundreds"`
/// and `"a few million"`, and fractional quantities such as `"half a
/// million"`, are protected from conversion. A line break ends every
/// expression: each line is converted independently and every break is kept.
///
/// # Arguments
///
/// * `input` - Transcript text to normalize.
/// * `threshold` - Value at or above which isolated single-digit cardinals and ordinals become digits.
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
    // text2num tokenizes across all whitespace, so a multi-line input would
    // join "twenty\nfive" into "25" and drop the break. Guards would likewise
    // absorb a count from the previous paragraph.
    let text = input
        .split('\n')
        .map(|line| render_signed_numbers(replace_numbers_in_segments(line, &language, threshold)))
        .collect::<Vec<_>>()
        .join("\n");
    TransformResult {
        matches: usize::from(text != input),
        text,
    }
}

/// Convert a line in segments separated by `and` between equal scales.
///
/// text2num reads "five thousand and ten thousand" as 5010 and 1000, but a
/// quantity whose largest scale equals the one before `and` cannot continue
/// that number. Other scales still join: "one hundred and twenty thousand five
/// hundred" is 120500, and "two thousand and five hundred" is 2500.
fn replace_numbers_in_segments(line: &str, language: &Language, threshold: f64) -> String {
    let tokens = number_tokens(line);
    let boundary = tokens.windows(2).enumerate().find_map(|(index, pair)| {
        let [scale, conjunction] = pair else {
            return None;
        };
        let rank = scale_rank(&scale.lowercase)?;
        if conjunction.lowercase != "and" {
            return None;
        }
        let following = &tokens[index + 2..];
        let next = find_numbers(following.iter(), language, 0.0)
            .into_iter()
            .next()?;
        if next.start != 0 {
            return None;
        }
        // Within one number, "and" follows a scale ("two hundred and fifty
        // thousand"). Greedy parsing can also carry the next quantity across an
        // "and" after a plain count ("fifty and two hundred"); stop there.
        let lead = (1..next.end)
            .find(|&at| {
                following[at].lowercase == "and"
                    && scale_rank(&following[at - 1].lowercase).is_none()
            })
            .unwrap_or(next.end);
        // text2num ends a decimal before its scale: "three point six" then "million".
        let decimal = following[..lead]
            .iter()
            .any(|token| token.lowercase == "point");
        let end = if decimal && lead == next.end {
            (lead + 1).min(following.len())
        } else {
            lead
        };
        let largest = following[..end]
            .iter()
            .filter_map(|token| scale_rank(&token.lowercase))
            .max();
        (largest == Some(rank)).then_some(conjunction)
    });
    let Some(conjunction) = boundary else {
        return replace_numbers_preserving_literals(line, language, threshold);
    };
    format!(
        "{}{}{}",
        replace_numbers_preserving_literals(&line[..conjunction.start], language, threshold),
        &line[conjunction.start..conjunction.end],
        replace_numbers_in_segments(&line[conjunction.end..], language, threshold),
    )
}

/// Order of magnitude for the scale words that can close a spoken quantity.
fn scale_rank(word: &str) -> Option<u8> {
    match word {
        "hundred" => Some(2),
        "thousand" => Some(3),
        "million" => Some(6),
        "billion" => Some(9),
        _ => None,
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

fn replace_numbers_preserving_literals(input: &str, language: &Language, threshold: f64) -> String {
    static PROTECTED_WORD: OnceLock<Regex> = OnceLock::new();
    static PREVIOUS_TOKEN: OnceLock<Regex> = OnceLock::new();
    static FOLLOWING_SCALE: OnceLock<Regex> = OnceLock::new();
    let protected_re = PROTECTED_WORD.get_or_init(|| {
        Regex::new(
            concat!(
                r"(?i)\b(?:",
                r"(?P<scale_first>(?:a[ \t]+)?(?:hundred|thousand|million|billion|trillion)[ \t]+and[ \t]+a[ \t]+(?:half|quarter))|",
                r"(?:and[ \t]+)?(?:a[ \t]+)?(?:half|quarter)(?:[ \t]+of)?(?:[ \t]+a)?[ \t]+(?:hundred|thousand|million|billion|trillion)|",
                r"(?:halves|quarters)(?:[ \t]+of)?(?:[ \t]+a)?[ \t]+(?:hundred|thousand|million|billion|trillion)|",
                r"(?:(?:a[ \t]+)?few|several|(?:a[ \t]+)?couple(?:[ \t]+of)?)[ \t]+(?:hundred|thousand|million|billion|trillion)|",
                r"(?P<denominator>[a-z]+(?:-[a-z]+)*)[ \t]+of(?:[ \t]+a)?[ \t]+(?:hundred|thousand|million|billion|trillion)|",
                r"second|tens|hundreds|thousands|millions|billions|trillions)\b",
            ),
        )
        .expect("protected number-word regex must compile")
    });
    let previous_re = PREVIOUS_TOKEN.get_or_init(|| {
        Regex::new(r"(?i)([a-z0-9]+)([-\s]+)$").expect("previous-token regex must compile")
    });
    let following_scale_re = FOLLOWING_SCALE.get_or_init(|| {
        Regex::new(r"(?i)^(?:[-\s]+(?:hundred|thousand|million|billion|trillion)\b)+")
            .expect("following-scale regex must compile")
    });

    let protected_words: Vec<_> = protected_re
        .captures_iter(input)
        .filter_map(|captures| {
            let found = captures.get(0).expect("whole match is required");
            let mut ordinal_start = None;
            if let Some(denominator) = captures.name("denominator") {
                // Use the parser's full ordinal boundary, including spaced
                // compounds such as "twenty first" or "one hundredth".
                let mut tokens = number_tokens(&input[..denominator.end()]);
                // text2num recognizes singular ordinals. Classify a plural
                // denominator through its singular form while preserving the
                // original text and byte positions for the literal span.
                if let Some(last) = tokens.last_mut() {
                    if is_plural_ordinal(
                        &last.lowercase,
                        &input[..last.start],
                        previous_re,
                        language,
                    ) {
                        last.lowercase.pop();
                    }
                }
                let occurrences = find_numbers(tokens.iter(), language, 0.0);
                let ordinal = occurrences
                    .last()
                    .filter(|number| number.end == tokens.len() && number.is_ordinal);
                let Some(ordinal) = ordinal else {
                    // The broad denominator candidate starts at the numerator
                    // of "three-quarters" or "one-half". Retry its last word
                    // so that it cannot hide an ordinary fraction guard.
                    if let Some((_, suffix)) = denominator.as_str().rsplit_once('-') {
                        let start = denominator.end() - suffix.len();
                        if protected_re.captures_at(input, start).is_some_and(|retry| {
                            retry.name("denominator").is_none()
                                && retry.get(0).is_some_and(|fraction| {
                                    fraction.start() == start && fraction.end() == found.end()
                                })
                        }) {
                            return Some((found, false, Some(found.start())));
                        }
                    }
                    // A rejected fraction candidate can contain a literal
                    // plural magnitude. Preserve that original guard instead
                    // of letting this broader candidate hide it.
                    return plural_magnitude(denominator.as_str()).then_some((
                        denominator,
                        false,
                        None,
                    ));
                };
                let mut start = ordinal.start;
                // Include only the adjacent fraction connectors; text2num
                // remains responsible for every word inside the denominator.
                for connector in ["a", "and"] {
                    if start > 0
                        && tokens[start - 1].lowercase == connector
                        && input[tokens[start - 1].end..tokens[start].start]
                            .bytes()
                            .all(|byte| matches!(byte, b' ' | b'\t'))
                    {
                        start -= 1;
                    }
                }
                ordinal_start = Some(tokens[start].start.min(found.start()));
            }
            if found.as_str().eq_ignore_ascii_case("second")
                && !second_is_time_unit(&input[..found.start()], previous_re, language)
            {
                return None;
            }
            Some((found, captures.name("scale_first").is_some(), ordinal_start))
        })
        .collect();
    if protected_words.is_empty() {
        return replace_numbers_with_hybrid_magnitudes(input, language, threshold);
    }

    // Parse independently around literal spans. This preserves plural grammar
    // and time units while exact quantities elsewhere still convert normally.
    let mut output = String::with_capacity(input.len());
    let mut last_end = 0;
    for (index, (found, scale_first, ordinal_start)) in protected_words.iter().enumerate() {
        let phrase_start = ordinal_start.unwrap_or(found.start()).max(last_end);
        let prefix = &input[last_end..phrase_start];
        let phrase = &input[phrase_start..found.end()];
        let is_plural_magnitude = plural_magnitude(phrase);
        let is_fraction = ordinal_start.is_some()
            || phrase.split_ascii_whitespace().any(|word| {
                matches!(
                    word.to_ascii_lowercase().as_str(),
                    "half" | "quarter" | "halves" | "quarters"
                )
            });
        let protected_start = if is_plural_magnitude || is_fraction {
            let preceding = if *scale_first
                && phrase
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|word| word.eq_ignore_ascii_case("a"))
            {
                // The article already starts this quantity; a preceding year
                // or chapter count is independent and must remain convertible.
                None
            } else if is_fraction && !scale_first {
                preceding_fraction_start(prefix, phrase, previous_re, language)
            } else {
                preceding_number_start(prefix, language)
            };
            last_end + preceding.unwrap_or(prefix.len())
        } else {
            phrase_start
        };
        // Adjacent singular scales belong to the same indefinite quantity:
        // parsing a later scale separately invents an exact count.
        let protected_end = if !phrase.eq_ignore_ascii_case("second") {
            found.end()
                + following_scale_re
                    .find(&input[found.end()..])
                    .map_or(0, |following| following.end())
        } else {
            found.end()
        };
        // A following-scale extension must not swallow the beginning of the
        // next protected phrase (e.g. "hundreds million and a half").
        let protected_end =
            protected_words
                .get(index + 1)
                .map_or(protected_end, |(next, _, ordinal_start)| {
                    protected_end.min(ordinal_start.unwrap_or(next.start()).max(found.end()))
                });
        output.push_str(&replace_numbers_with_hybrid_magnitudes(
            &input[last_end..protected_start],
            language,
            threshold,
        ));
        output.push_str(&input[protected_start..protected_end]);
        last_end = protected_end;
    }
    output.push_str(&replace_numbers_with_hybrid_magnitudes(
        &input[last_end..],
        language,
        threshold,
    ));
    output
}

/// Whether a word is one of the literal plural magnitude guards.
fn plural_magnitude(phrase: &str) -> bool {
    matches!(
        phrase.to_ascii_lowercase().as_str(),
        "tens" | "hundreds" | "thousands" | "millions" | "billions" | "trillions"
    )
}

/// Return the start of the contiguous trailing cardinal phrases in a text slice,
/// when followed only by whitespace. The caller keeps that raw span
/// with the adjacent plural magnitude instead of converting it separately.
fn preceding_number_start(input: &str, language: &Language) -> Option<usize> {
    let tokens = number_tokens(input);
    let occurrences = find_numbers(tokens.iter(), language, 0.0);
    let last = occurrences.last()?;
    if last.start == last.end
        || last.is_ordinal
        || last.end != tokens.len()
        || !input[tokens[last.end - 1].end..]
            .chars()
            .all(char::is_whitespace)
    {
        return None;
    }
    let mut start = last.start;
    for previous in occurrences.iter().rev().skip(1) {
        // A preceding date/chapter ordinal is an independent exact quantity,
        // even beside a plural magnitude or a phrase such as "quarter million".
        if previous.is_ordinal {
            break;
        }
        let adjacent = previous.end == start
            && input[tokens[previous.end - 1].end..tokens[start].start]
                .chars()
                .all(char::is_whitespace);
        let joined_fraction = previous.end + 1 == start
            && tokens[previous.end].lowercase == "and"
            && input[tokens[previous.end - 1].end..tokens[previous.end].start]
                .chars()
                .all(char::is_whitespace)
            && input[tokens[previous.end].end..tokens[start].start]
                .chars()
                .all(char::is_whitespace);
        if !adjacent && !joined_fraction {
            break;
        }
        start = previous.start;
    }
    Some(tokens[start].start)
}

/// Keep a fraction's numerator or mixed count, but leave independent exact
/// counts available for conversion before a singular fraction.
fn preceding_fraction_start(
    input: &str,
    phrase: &str,
    previous_re: &Regex,
    language: &Language,
) -> Option<usize> {
    let input = input.strip_suffix('-').unwrap_or(input);
    let start = preceding_number_start(input, language)?;
    let joined = phrase
        .split_ascii_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("and"));
    let plural = number_tokens(phrase).iter().any(|token| {
        matches!(token.lowercase.as_str(), "halves" | "quarters")
            || is_plural_ordinal(
                &token.lowercase,
                &format!("{input}{}", &phrase[..token.start]),
                previous_re,
                language,
            )
    });
    // A conjunction can introduce either a mixed fraction or independent
    // quantities. Preserve it with the count instead of guessing from nearby
    // words such as "both" or "between".
    if joined || plural {
        return Some(start);
    }
    let numerator = number_tokens(&input[start..]);
    let occurrences = find_numbers(numerator.iter(), language, 0.0);
    let has_article = phrase
        .split_ascii_whitespace()
        .any(|word| word.eq_ignore_ascii_case("a"));
    // An article already supplies the singular fraction's numerator. Preserve
    // an explicit "one", but do not absorb a nearby chapter or year count.
    let singular_numerator = numerator.len() == 1
        && (!has_article || occurrences.last().is_some_and(|number| number.value == 1.0));
    // `text2num` recognizes a compound count such as "two thousand and five"
    // as one expression. Separate counts joined by "and" form a mixed fraction.
    let mixed_numerator =
        occurrences.len() > 1 && numerator.iter().any(|token| token.lowercase == "and");
    (singular_numerator || mixed_numerator).then_some(start)
}

/// Classify plural denominators without mistaking elapsed seconds for fractions.
fn is_plural_ordinal(word: &str, prefix: &str, previous_re: &Regex, language: &Language) -> bool {
    let Some(singular) = word.strip_suffix('s') else {
        return false;
    };
    if singular == "second" && second_is_time_unit(prefix, previous_re, language) {
        return false;
    }
    let tokens = number_tokens(singular);
    find_numbers(tokens.iter(), language, 0.0)
        .last()
        .is_some_and(|number| number.is_ordinal && number.end == tokens.len())
}

fn number_tokens(input: &str) -> Vec<NumberToken<'_>> {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let token_re =
        TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9]+|[^\s]").expect("number token regex"));

    token_re
        .find_iter(input)
        .map(|found| NumberToken {
            text: found.as_str(),
            lowercase: found.as_str().to_ascii_lowercase(),
            start: found.start(),
            end: found.end(),
        })
        .collect()
}

/// Private-use character that makes text2num end a number before the next
/// word. Like punctuation it ends the number without breaking a listing, so the
/// next value keeps its threshold context. It never leaves this module.
const NUMBER_BREAK: char = '\u{E000}';

/// Convert a span after moving the ends that text2num's greedy parse misplaces.
///
/// text2num extends a number with every word that still fits. It reads "two
/// hundred three hundred" as 203 and a bare 100, and "fifty and three" as 53.
/// Each round marks where a new number starts and parses again, because a
/// break after "and" can expose a count that the following scale then takes.
fn replace_numbers_with_hybrid_magnitudes(
    input: &str,
    language: &Language,
    threshold: f64,
) -> String {
    // Text that already holds the marker cannot be marked; a stranded scale
    // there keeps its wording instead.
    if input.contains(NUMBER_BREAK) {
        return render_hybrid_magnitudes(input, language, threshold);
    }
    let mut marked: Option<String> = None;
    // "and" breaks come first and stranded scales second, so two rounds
    // suffice; the bound only guards against a cycle.
    for _ in 0..4 {
        let text = marked.as_deref().unwrap_or(input);
        let breaks = number_breaks(text, language);
        if breaks.is_empty() {
            break;
        }
        let mut next = String::with_capacity(text.len() + breaks.len() * NUMBER_BREAK.len_utf8());
        let mut last = 0;
        for at in breaks {
            next.push_str(&text[last..at]);
            next.push(NUMBER_BREAK);
            last = at;
        }
        next.push_str(&text[last..]);
        marked = Some(next);
    }
    match marked {
        Some(text) => {
            render_hybrid_magnitudes(&text, language, threshold).replace(NUMBER_BREAK, "")
        }
        None => render_hybrid_magnitudes(input, language, threshold),
    }
}

/// Byte offsets, ascending, where a new number starts inside a text2num match.
fn number_breaks(input: &str, language: &Language) -> Vec<usize> {
    let tokens = number_tokens(input);
    let occurrences = find_numbers(tokens.iter(), language, 0.0);
    // "and" continues a number after a scale ("two hundred and fifty"), never
    // after a count: "fifty and three" is two numbers, not 53.
    let mut breaks: Vec<usize> = occurrences
        .iter()
        .filter(|number| !number.is_ordinal)
        .flat_map(|number| number.start + 1..number.end.saturating_sub(1))
        .filter(|&at| {
            tokens[at].lowercase == "and" && scale_rank(&tokens[at - 1].lowercase).is_none()
        })
        .map(|at| tokens[at + 1].start)
        .collect();
    if breaks.is_empty() {
        breaks = occurrences
            .windows(2)
            .filter_map(|pair| {
                let [count, scale] = pair else {
                    return None;
                };
                match stranded_scale(input, &tokens, count, scale, language)? {
                    StrandedScale::Coefficient(at) => Some(tokens[at].start),
                    StrandedScale::Decimal(end) if end < scale.end => Some(tokens[end].start),
                    StrandedScale::Decimal(_) | StrandedScale::Ambiguous => None,
                }
            })
            .collect();
    }
    breaks
}

/// How to read a scale word that text2num could not apply to the number before it.
enum StrandedScale {
    /// The count starting at this token is the scale's coefficient.
    Coefficient(usize),
    /// A decimal coefficient takes the scale words before this token, which
    /// starts the next number.
    Decimal(usize),
    /// More than one split is grammatical, or the scales may stack.
    Ambiguous,
}

/// Classify a number that starts with a bare scale word right after another.
///
/// text2num cannot apply "hundred" to 203 or "thousand" to 1502, so it ends
/// the first number and gives the scale an implicit coefficient of 1. The
/// trailing count of the first number is the coefficient that was spoken. A
/// split is accepted only when exactly one count position leaves two complete
/// numbers.
fn stranded_scale(
    input: &str,
    tokens: &[NumberToken<'_>],
    count: &Occurence,
    scale: &Occurence,
    language: &Language,
) -> Option<StrandedScale> {
    let rank = scale_rank(&tokens[scale.start].lowercase)?;
    if count.is_ordinal
        || scale.is_ordinal
        || count.end != scale.start
        || !input[tokens[count.end - 1].end..tokens[scale.start].start]
            .chars()
            .all(char::is_whitespace)
    {
        return None;
    }
    let words = &tokens[count.start..count.end];
    // text2num cannot scale a decimal at all; its scale words end the quantity.
    if words.iter().any(|token| token.lowercase == "point") {
        let end = (scale.start..scale.end)
            .find(|&at| tokens[at].lowercase != "-" && scale_rank(&tokens[at].lowercase).is_none())
            .unwrap_or(scale.end);
        return Some(StrandedScale::Decimal(end));
    }
    // "fifteen hundred thousand" can stack its scales into 1500000.
    let largest = words
        .iter()
        .filter_map(|token| scale_rank(&token.lowercase))
        .max();
    if scale_rank(&words.last()?.lowercase).is_some()
        && largest.is_some_and(|largest| largest < rank)
    {
        return Some(StrandedScale::Ambiguous);
    }
    let mut splits = (count.start + 1..count.end).filter(|&at| {
        let previous = &tokens[at - 1].lowercase;
        let head_end = if previous == "and" { at - 1 } else { at };
        tokens[at].lowercase != "and"
            && scale_rank(&tokens[at].lowercase).is_none()
            && previous != "-"
            && is_one_number(&tokens[count.start..head_end], language)
            && is_one_number(&tokens[at..scale.end], language)
    });
    let first = splits.next()?;
    Some(if splits.next().is_some() {
        StrandedScale::Ambiguous
    } else {
        StrandedScale::Coefficient(first)
    })
}

/// Whether text2num reads all of `tokens` as exactly one cardinal.
fn is_one_number(tokens: &[NumberToken<'_>], language: &Language) -> bool {
    matches!(
        find_numbers(tokens.iter(), language, 0.0).as_slice(),
        [number] if number.start == 0 && number.end == tokens.len() && !number.is_ordinal
    )
}

fn render_hybrid_magnitudes(input: &str, language: &Language, threshold: f64) -> String {
    static NUMERIC_MAGNITUDE: OnceLock<Regex> = OnceLock::new();
    let numeric_re = NUMERIC_MAGNITUDE.get_or_init(|| {
        Regex::new(r"(?i)\b\d+(?:\.\d+)?[ \t]+(?:million|billion)\b")
            .expect("numeric magnitude regex")
    });

    let tokens = number_tokens(input);
    let occurrences = find_numbers(tokens.iter(), language, 0.0);
    let mut replacements: Vec<_> = numeric_re
        .find_iter(input)
        .map(|found| (found.start(), found.end(), found.as_str().to_string()))
        .collect();

    // Adjacent spoken small integers can name a year, time, phone number, or
    // separate values. Preserve the entire run rather than interpreting it.
    // Punctuation, explicit scales, decimals, and existing digits are boundaries.
    let mut index = 0;
    while index + 1 < occurrences.len() {
        let first = &occurrences[index];
        let mut end = index;
        while let Some(next) = occurrences.get(end + 1) {
            let previous = &occurrences[end];
            if [previous, next].iter().any(|number| {
                number.is_ordinal
                    || !(0.0..100.0).contains(&number.value)
                    || number.value.fract() != 0.0
                    || tokens[number.start..number.end]
                        .iter()
                        .any(|token| token.lowercase == "point")
                    || input[tokens[number.end - 1].end..].starts_with('-')
            }) || previous.end != next.start
                || !input[tokens[previous.start].start..tokens[next.end - 1].end]
                    .chars()
                    .all(|ch| ch.is_ascii_alphabetic() || matches!(ch, ' ' | '\t' | '-'))
            {
                break;
            }
            end += 1;
        }
        if end > index {
            // A suffix such as "K" or "B" does not disambiguate the run either:
            // "Llama three seventy B" names version 3 at 70B, not 370B.
            let start = tokens[first.start].start;
            let end = tokens[occurrences[end].end - 1].end;
            replacements.push((start, end, input[start..end].to_owned()));
        }
        index = end + 1;
    }

    // A decimal coefficient keeps its scale words, as text2num cannot scale it:
    // "two point five thousand" is 2.5 thousand. Breaks already gave every
    // other unambiguous stranded scale its coefficient. One that remains would
    // render as a bare 100 or 1000, so keep the words.
    for pair in occurrences.windows(2) {
        let [count, scale] = pair else {
            continue;
        };
        let Some(stranded) = stranded_scale(input, &tokens, count, scale, language) else {
            continue;
        };
        let start = tokens[count.start].start;
        let end = tokens[scale.end - 1].end;
        let replacement = match stranded {
            StrandedScale::Decimal(scale_end) if scale_end == scale.end => {
                let scales: Vec<_> = tokens[scale.start..scale.end]
                    .iter()
                    .filter(|token| scale_rank(&token.lowercase).is_some())
                    .map(|token| token.lowercase.as_str())
                    .collect();
                format!("{} {}", count.text, scales.join(" "))
            }
            _ => input[start..end].to_owned(),
        };
        replacements.push((start, end, replacement));
    }

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
