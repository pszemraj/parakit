//! Shared vocabulary for numeric boundaries and contextual labels.

/// # Returns
/// Whether a word names a time, measurement, or currency unit.
///
/// # Panics
/// Panics only if the built-in unit regex cannot compile.
pub(super) fn is_unit(word: &str) -> bool {
    let unit = regex_static!(
        concat!(
            r"(?i)^(?:[kmunpgtpe]?(?:g|m|s|a|w|v|b)|[kmgtp]?ib|[kmgt]?hz|[a-z]p[smh]|[kmg]?px|",
            r"bit|mol|amp|ton|bar|rad|lux|db|lb|lbs|oz|ft|yd|in|usd|eur|gbp|jpy|cad|aud|chf|cny|",
            r"(?:nano|micro|milli|centi|deci|kilo|mega|giga|tera)?(?:meter|metre|gram|second)s?|",
            r"seconds?|secs?|minutes?|mins?|hours?|days?|weeks?|months?|years?|degrees?|percent|cents?|dollars?|euros?|pounds?|watts?|bytes?|bits?)$"
        ),
        "numeric unit regex must compile"
    );
    unit.is_match(word)
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
