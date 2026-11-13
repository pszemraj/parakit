//! Shared vocabulary for numeric boundaries and contextual labels.

/// # Returns
/// Whether a word names a time, measurement, or currency unit.
///
/// # Panics
/// Panics only if the built-in unit regex cannot compile.
pub(super) fn is_unit(word: &str) -> bool {
    let unit = regex_static!(
        concat!(
            r"(?i)^(?:ms|ns|gb|mb|kb|tb|gib|mib|kib|hz|khz|mhz|ghz|kg|km|cm|mm|fps|kw|mw|gw|px|",
            r"bit|mol|amp|ton|bar|rad|lux|db|lb|lbs|oz|ft|yd|usd|eur|gbp|jpy|cad|aud|chf|cny|",
            r"(?:nano|micro|milli|centi|deci|kilo|mega|giga|tera)?(?:meter|metre|gram|second)s?|",
            r"seconds?|secs?|minutes?|mins?|hours?|degrees?|percent|cents?|dollars?|euros?|pounds?|watts?|(?:kilo|mega|giga|tera)?(?:bytes?|bits?))$"
        ),
        "numeric unit regex must compile"
    );
    unit.is_match(word) || is_calendar_unit(&word.to_ascii_lowercase())
}

/// # Returns
/// Whether a lowercase word names a calendar division that can take an ordinal.
pub(super) fn is_calendar_unit(word: &str) -> bool {
    matches!(
        word,
        "day"
            | "days"
            | "week"
            | "weeks"
            | "month"
            | "months"
            | "year"
            | "years"
            | "decade"
            | "decades"
            | "century"
            | "centuries"
            | "millennium"
            | "millennia"
    )
}

/// # Returns
/// Whether a lowercase word is an exact English number-parser token.
pub(super) fn number_word(word: &str) -> bool {
    matches!(
        word,
        "zero"
            | "nought"
            | "one"
            | "two"
            | "three"
            | "four"
            | "five"
            | "six"
            | "seven"
            | "eight"
            | "nine"
            | "ten"
            | "eleven"
            | "twelve"
            | "thirteen"
            | "fourteen"
            | "fifteen"
            | "sixteen"
            | "seventeen"
            | "eighteen"
            | "nineteen"
            | "twenty"
            | "thirty"
            | "forty"
            | "fifty"
            | "sixty"
            | "seventy"
            | "eighty"
            | "ninety"
            | "hundred"
            | "thousand"
            | "million"
            | "billion"
            | "trillion"
            | "hundreds"
            | "thousands"
            | "millions"
            | "billions"
            | "trillions"
            | "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "seventh"
            | "eighth"
            | "ninth"
            | "tenth"
            | "eleventh"
            | "twelfth"
            | "thirteenth"
            | "fourteenth"
            | "fifteenth"
            | "sixteenth"
            | "seventeenth"
            | "eighteenth"
            | "nineteenth"
            | "twentieth"
            | "thirtieth"
            | "fortieth"
            | "fiftieth"
            | "sixtieth"
            | "seventieth"
            | "eightieth"
            | "ninetieth"
            | "hundredth"
            | "thousandth"
            | "millionth"
            | "billionth"
            | "trillionth"
            | "and"
            | "point"
            | "-"
    )
}

/// # Returns
/// Whether a word may mean zero inside an explicitly recognized numeric structure.
pub(super) fn zero_alias(word: &str) -> bool {
    matches!(word, "oh" | "o" | "nought")
}

/// # Arguments
/// * `tokens` - Original numeric tokens and prose boundaries.
/// * `end` - Exclusive end of the run to inspect.
///
/// # Returns
/// Start of the contiguous numeric vocabulary ending at `end`.
///
/// # Panics
/// Panics if `end` exceeds the token count.
pub(super) fn run_start(tokens: &[super::numbers::NumberToken<'_>], end: usize) -> usize {
    tokens[..end]
        .iter()
        .rposition(|token| {
            let word = token.lowercase.as_str();
            !number_word(word)
                && !word.strip_suffix('s').is_some_and(number_word)
                && !matches!(word, "oh" | "o")
                && token.text.parse::<f64>().is_err()
        })
        .map_or(0, |index| index + 1)
}

/// # Returns
/// The value of an English magnitude word, if present.
pub(super) fn scale(word: &str) -> Option<f64> {
    match word {
        "hundred" => Some(100.0),
        "thousand" => Some(1_000.0),
        "million" => Some(1_000_000.0),
        "billion" => Some(1_000_000_000.0),
        "trillion" => Some(1_000_000_000_000.0),
        _ => None,
    }
}
